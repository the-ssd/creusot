use std::rc::Rc;

use crate::analysis::{
    Borrows, MaybeInitializedLocals, MaybeLiveExceptDrop, MaybeUninitializedLocals,
};
use borrowck::{borrow_set::BorrowSet, RegionInferenceContext};
use rustc_index::bit_set::BitSet;
use rustc_middle::{
    mir::{traversal, BasicBlock, Body, Local, Location},
    ty::TyCtxt,
};
use rustc_mir_dataflow::{Analysis, ResultsCursor};

use crate::extended_location::ExtendedLocation;

pub struct EagerResolver<'body, 'tcx> {
    local_live: ResultsCursor<'body, 'tcx, MaybeLiveExceptDrop>,

    // Whether a local is initialized or not at a location
    local_init: ResultsCursor<'body, 'tcx, MaybeInitializedLocals>,

    local_uninit: ResultsCursor<'body, 'tcx, MaybeUninitializedLocals>,

    borrows: ResultsCursor<'body, 'tcx, Borrows<'body, 'tcx>>,

    borrow_set: Rc<BorrowSet<'tcx>>,

    body: &'body Body<'tcx>,
}

impl<'body, 'tcx> EagerResolver<'body, 'tcx> {
    pub(crate) fn new(
        tcx: TyCtxt<'tcx>,
        body: &'body Body<'tcx>,
        borrow_set: Rc<BorrowSet<'tcx>>,
        regioncx: Rc<RegionInferenceContext<'tcx>>,
    ) -> Self {
        let local_init = MaybeInitializedLocals
            .into_engine(tcx, body)
            .iterate_to_fixpoint()
            .into_results_cursor(body);

        let local_uninit = MaybeUninitializedLocals
            .into_engine(tcx, body)
            .iterate_to_fixpoint()
            .into_results_cursor(body);

        // MaybeLiveExceptDrop ignores `drop` for the purpose of resolve liveness... unclear that this can
        // be sound.
        let local_live = MaybeLiveExceptDrop
            .into_engine(tcx, body)
            .iterate_to_fixpoint()
            .into_results_cursor(body);

        let borrows_out_of_scope = borrowck::dataflow::calculate_borrows_out_of_scope_at_location(
            body,
            &regioncx,
            &borrow_set,
        );

        let borrows = Borrows::new(tcx, body, borrow_set.clone(), borrows_out_of_scope.clone());

        let borrows =
            borrows.into_engine(tcx, body).iterate_to_fixpoint().into_results_cursor(body);

        EagerResolver { local_live, local_init, local_uninit, borrows, borrow_set, body }
    }

    fn seek_to(&mut self, loc: ExtendedLocation) {
        loc.seek_to(&mut self.local_live);
        loc.seek_to(&mut self.local_init);
        loc.seek_to(&mut self.local_uninit);
        loc.seek_to(&mut self.borrows);
    }

    fn dead_locals(&self) -> BitSet<Local> {
        let mut live: BitSet<_> = BitSet::new_empty(self.body.local_decls.len());
        live.union(self.local_live.get());

        let mut dead: BitSet<_> = BitSet::new_filled(live.domain_size());
        dead.subtract(&live);
        dead
    }

    fn frozen_locals(&self) -> BitSet<Local> {
        let mut frozen: BitSet<_> = BitSet::new_empty(self.body.local_decls.len());
        for bi in self.borrows.get().iter() {
            let l = self.borrow_set[bi].borrowed_place.local;
            frozen.insert(l);
        }
        frozen
    }

    fn resolved_locals(&self) -> BitSet<Local> {
        let dead = self.dead_locals();

        let init = self.local_init.get().clone();
        let mut uninit: BitSet<_> = BitSet::new_empty(self.body.local_decls.len());
        uninit.union(self.local_uninit.get());

        trace!("dead: {dead:?}");
        trace!("init: {init:?}");
        trace!("uninit: {uninit:?}");

        let mut def_init = init;
        def_init.subtract(&uninit);
        trace!("def_init: {def_init:?}");

        let frozen = self.frozen_locals();

        let mut resolved = dead;
        resolved.intersect(&def_init);
        resolved.subtract(&frozen);

        resolved
    }

    fn resolved_locals_at(&mut self, loc: ExtendedLocation) -> BitSet<Local> {
        trace!("location: {loc:?}");

        if loc.is_entry_loc() {
            // At function entry, no locals are resolved
            // Thus, never live, initialized locals are resolved at mid of the entry location
            return BitSet::new_empty(self.body.local_decls.len());
        }

        self.seek_to(loc);
        self.resolved_locals()
    }

    fn resolved_locals_in_range(
        &mut self,
        start: ExtendedLocation,
        end: ExtendedLocation,
    ) -> BitSet<Local> {
        let resolved_at_start = self.resolved_locals_at(start);
        let resolved_at_end = self.resolved_locals_at(end);

        let mut resolved = resolved_at_end;
        resolved.subtract(&resolved_at_start);
        resolved
    }

    pub fn resolved_locals_during(&mut self, loc: Location) -> BitSet<Local> {
        self.resolved_locals_in_range(ExtendedLocation::Start(loc), ExtendedLocation::Mid(loc))
    }

    /// Only valid if loc is not a terminator.
    pub fn dead_locals_after(&mut self, loc: Location) -> BitSet<Local> {
        let next_loc = loc.successor_within_block();
        ExtendedLocation::Start(next_loc).seek_to(&mut self.local_live);
        self.dead_locals()
    }

    pub fn live_locals_before(&mut self, loc: Location) -> BitSet<Local> {
        ExtendedLocation::Start(loc).seek_to(&mut self.local_live);
        let mut live: BitSet<_> = BitSet::new_empty(self.body.local_decls.len());
        live.union(self.local_live.get());
        live
    }

    pub fn resolved_locals_between_blocks(
        &mut self,
        from: BasicBlock,
        to: BasicBlock,
    ) -> BitSet<Local> {
        let term = self.body.terminator_loc(from);
        let start = to.start_location();
        self.resolved_locals_in_range(ExtendedLocation::Start(term), ExtendedLocation::Start(start))
    }

    pub fn frozen_locals_before(&mut self, loc: Location) -> BitSet<Local> {
        ExtendedLocation::Start(loc).seek_to(&mut self.borrows);
        self.frozen_locals()
    }

    #[allow(dead_code)]
    pub(crate) fn debug(&mut self, regioncx: Rc<RegionInferenceContext<'tcx>>) {
        let body = self.body.clone();
        for (bb, bbd) in traversal::preorder(&body) {
            if bbd.is_cleanup {
                continue;
            }
            eprintln!("{:?}", bb);
            let mut loc = bb.start_location();
            for statement in &bbd.statements {
                self.seek_to(ExtendedLocation::Start(loc));
                let live1 = self.local_live.get().iter().collect::<Vec<_>>();
                let init1 = self.local_init.get().clone();
                let uninit1 = self.local_uninit.get().iter().collect::<Vec<_>>();
                let frozen1 = self.frozen_locals();
                let resolved1 = self.resolved_locals();

                self.seek_to(ExtendedLocation::Mid(loc));
                let live2 = self.local_live.get().iter().collect::<Vec<_>>();
                let init2 = self.local_init.get().clone();
                let uninit2 = self.local_uninit.get().iter().collect::<Vec<_>>();
                let frozen2 = self.frozen_locals();
                let resolved2 = self.resolved_locals();

                eprintln!("  {statement:?} {resolved1:?} -> {resolved2:?}");
                eprintln!(
                    "    live={live1:?} -> {live2:?} frozen={frozen1:?} -> {frozen2:?} init={init1:?} -> {init2:?} uninit={uninit1:?} -> {uninit2:?}",
                );
                if let Some(borrow) = self.borrow_set.location_map.get(&loc) {
                    eprintln!(
                        "    region={:?} value={:?}",
                        borrow.region,
                        regioncx.region_value_str(borrow.region),
                    );
                }

                loc = loc.successor_within_block();
            }

            self.seek_to(ExtendedLocation::Start(loc));
            let live1 = self.local_live.get().iter().collect::<Vec<_>>();
            let init1 = self.local_init.get().clone();
            let uninit1 = self.local_uninit.get().iter().collect::<Vec<_>>();
            let frozen1 = self.frozen_locals();
            let resolved1 = self.resolved_locals();

            self.seek_to(ExtendedLocation::Mid(loc));
            let live2 = self.local_live.get().iter().collect::<Vec<_>>();
            let init2 = self.local_init.get().clone();
            let uninit2 = self.local_uninit.get().iter().collect::<Vec<_>>();
            let frozen2 = self.frozen_locals();
            let resolved2 = self.resolved_locals();

            eprintln!("  {:?} {resolved1:?} -> {resolved2:?}", bbd.terminator().kind);
            eprintln!(
                "    live={live1:?} -> {live2:?} frozen={frozen1:?} -> {frozen2:?} init={init1:?} -> {init2:?} uninit={uninit1:?} -> {uninit2:?}",
            );
        }
        eprintln!();
    }
}
