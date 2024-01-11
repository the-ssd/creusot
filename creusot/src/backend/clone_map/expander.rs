use std::collections::VecDeque;

use crate::{backend::ty_inv, translation::traits};
use petgraph::Direction;
use rustc_hir::def_id::DefId;
use rustc_middle::{
    mir::Mutability,
    ty::{subst::SubstsRef, AliasKind, ParamEnv, Ty, TyKind},
};

use super::*;

/// The `Expander` takes a list of 'root' dependencies (items explicitly requested by user code),
/// and expands this into a complete dependency graph.
pub(super) struct Expander<'a, 'tcx> {
    pub clone_graph: DepGraph<'tcx>,
    pub namer: &'a mut CloneNames<'tcx>,
    self_id: TransId,
    param_env: ParamEnv<'tcx>,
    expansion_queue: VecDeque<DepNode<'tcx>>,
}

impl<'a, 'tcx> Expander<'a, 'tcx> {
    pub fn new(
        namer: &'a mut CloneNames<'tcx>,
        self_id: TransId,
        param_env: ParamEnv<'tcx>,
    ) -> Self {
        Self {
            clone_graph: Default::default(),
            namer,
            self_id,
            param_env,
            expansion_queue: Default::default(),
        }
    }

    fn self_did(&self) -> Option<DefId> {
        match self.self_id {
            TransId::Item(did) | TransId::TyInv(TyInvKind::Adt(did)) => Some(did),
            _ => None,
        }
    }

    pub fn add_root(&mut self, key: DepNode<'tcx>, level: CloneLevel) {
        self.clone_graph.add_root(key, self.namer.insert(key), level);
        self.expansion_queue.push_back(key);
    }

    /// Expand the graph with new entries
    pub fn update_graph(
        mut self,
        ctx: &mut Why3Generator<'tcx>,
        depth: CloneDepth,
    ) -> DepGraph<'tcx> {
        let self_key = CloneNode::from_trans_id(ctx.tcx, self.self_id);

        for key in &self.expansion_queue {
            if *key != self_key {
                self.clone_graph.add_graph_edge(self_key, *key, CloneLevel::Root);
            }
        }

        while let Some(key) = self.expansion_queue.pop_front() {
            trace!("update graph with {:?} (public={:?})", key, self.clone_graph.info(key).level);

            if depth == CloneDepth::Shallow && !self.clone_graph.is_root(key) {
                // If there is a Signature level edge from a pre-existing root node, mark this one as root as well as it must be an associated type in
                // a root signature
                if self.clone_graph.graph.edges_directed(key, Direction::Incoming).any(
                    |(src, _, (lvl, _))| {
                        self.clone_graph.is_root(src) && *lvl == CloneLevel::Signature
                    },
                ) {
                    self.add_root(key, self.clone_graph.info(key).level)
                } else {
                    continue;
                }
            }

            if self.clone_graph.graph.edges_directed(key, Direction::Incoming).count() == 0 {
                self.clone_graph.add_graph_edge(self_key, key, CloneLevel::Root);
            }

            if let Some((did, subst)) = key.did() {
                if traits::still_specializable(ctx.tcx, self.param_env, did, subst) {
                    self.clone_graph.info_mut(key).opaque();
                }

                if self_key
                    .did()
                    .is_some_and(|(self_did, _)| !ctx.is_transparent_from(did, self_did))
                {
                    self.clone_graph.info_mut(key).opaque();
                }

                ctx.translate(did);

                if util::is_inv_internal(ctx.tcx, did) && depth == CloneDepth::Deep {
                    let ty = subst.type_at(0);
                    let ty = ctx.try_normalize_erasing_regions(self.param_env, ty).unwrap_or(ty);
                    self.expand_ty_inv(ctx, self.param_env, ty);
                }

                self.expand_laws(ctx, did, subst, depth);
            }

            self.expand_subst(ctx, key);
            self.expand_projections(ctx, key);
            self.expand_dependencies(ctx, key);
        }

        self.clone_graph
    }

    fn expand_projections(&mut self, ctx: &mut Why3Generator<'tcx>, key: DepNode<'tcx>) {
        let Some((id, _)) = key.did() else { return; };
        let ItemType::Type = util::item_type(ctx.tcx, id) else {return; };

        let key_public = self.clone_graph.info(key).level;

        for p in ctx.projections_in_ty(id).to_owned() {
            let node = self.resolve_dep(
                ctx,
                CloneNode::new(ctx.tcx, (p.def_id, p.substs)).subst(ctx.tcx, key),
            );

            let is_type = self
                .self_did()
                .map(|did| util::item_type(ctx.tcx, did) == ItemType::Type)
                .unwrap_or(false);

            if !is_type {
                self.add_node(node, key_public);
                self.clone_graph.add_graph_edge(key, node, CloneLevel::Signature);
            }
        }
    }

    fn expand_subst(&mut self, ctx: &mut Why3Generator<'tcx>, key: DepNode<'tcx>) {
        let Some((_, key_subst)) = key.did() else { return; };
        let key_public = self.clone_graph.info(key).level;

        // Check the substitution for node dependencies on closures
        walk_types(key_subst, |t| {
            let node = match t.kind() {
                TyKind::Alias(AliasKind::Projection, pty) => {
                    let node = CloneNode::new(ctx.tcx, (pty.def_id, pty.substs));
                    Some(self.resolve_dep(ctx, node))
                }
                TyKind::Closure(id, subst) => {
                    // Sketchy... shouldn't we need to do something to subst?
                    Some(CloneNode::new(ctx.tcx, (*id, *subst)))
                }
                TyKind::Ref(_, _, Mutability::Mut) => {
                    self.clone_graph.add_builtin(PreludeModule::Borrow);
                    None
                }
                TyKind::Adt(_, _) => Some(CloneNode::Type(t)),
                _ => None,
            };

            if let Some(node) = node {
                self.add_node(node, key_public);
                self.clone_graph.add_graph_edge(key, node, CloneLevel::Signature);
            }
        });
    }

    fn expand_dependencies(&mut self, ctx: &mut Why3Generator<'tcx>, key: DepNode<'tcx>) {
        let key_public = self.clone_graph.info(key).level;

        for (dep, info) in ctx.dependencies(key).iter().flat_map(|i| i.iter()) {
            trace!("adding dependency {:?} {:?}", dep, info.level);

            let dep = self.resolve_dep(ctx, dep.subst(ctx.tcx, key));

            trace!("inserting dependency {:?} {:?}", key, dep);
            self.add_node(dep, key_public.max(info.level));

            // Skip reflexive edges
            if dep == key {
                continue;
            }

            self.clone_graph.add_graph_edge(key, dep, info.level);
        }
    }

    fn expand_ty_inv(
        &mut self,
        ctx: &mut Why3Generator<'tcx>,
        param_env: ParamEnv<'tcx>,
        ty: Ty<'tcx>,
    ) {
        let inv_kind = if ty_inv::is_tyinv_trivial(ctx.tcx, param_env, ty, true) {
            TyInvKind::Trivial
        } else {
            TyInvKind::from_ty(ty)
        };

        if let TransId::TyInv(self_kind) = self.self_id && self_kind == inv_kind {
            return;
        }

        ctx.translate_tyinv(inv_kind);
        self.add_node(DepNode::TyInv(ty, inv_kind), CloneLevel::Body);
    }

    fn expand_laws(
        &mut self,
        ctx: &mut TranslationCtx<'tcx>,
        key_did: DefId,
        key_subst: SubstsRef<'tcx>,
        depth: CloneDepth,
    ) {
        let Some(item) = ctx.tcx.opt_associated_item(key_did) else { return };
        let Some(self_did) = self.self_did() else { return };

        if depth == CloneDepth::Shallow {
            return;
        }

        // Dont clone laws into the trait / impl which defines them.
        if let Some(self_item) = ctx.tcx.opt_associated_item(self_did)
            && self_item.container_id(ctx.tcx) == item.container_id(ctx.tcx) {
            return;
        }

        // TODO: Push out of graph expansion
        // If the function we are cloning into is `#[trusted]` there is no need for laws.
        // Similarily, if it has no body, there will be no proofs.
        if util::is_trusted(ctx.tcx, self_did) || !util::has_body(ctx, self_did) {
            return;
        }

        let tcx = ctx.tcx;
        for law in ctx.laws(item.container_id(tcx)) {
            trace!("adding law {:?} in {:?}", *law, self.self_id);
            let dep = DepNode::new(tcx, (*law, key_subst));
            self.add_node(dep, CloneLevel::Body);
        }
    }

    // Given an initial substitution, find out the substituted and resolved version of the dependency `dep`.
    // This will attempt to normalize traits and associated types if the substitution provides enough
    // information.
    fn resolve_dep(&self, ctx: &TranslationCtx<'tcx>, dep: DepNode<'tcx>) -> DepNode<'tcx> {
        let param_env = self.param_env;
        dep.resolve(ctx, param_env).unwrap_or(dep)
    }

    fn add_node(&mut self, dep: DepNode<'tcx>, level: CloneLevel) {
        if self.clone_graph.add_node(dep, self.namer.insert(dep), level) {
            self.expansion_queue.push_back(dep);
        }
    }
}
