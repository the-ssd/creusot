use crate::*;

pub trait IndexLogic<I> {
    type Item;

    #[logic]
    fn index_logic(self, ix: I) -> Self::Item;
}

impl<T> IndexLogic<Int> for Seq<T> {
    type Item = T;

    #[logic]
    #[trusted]
    #[creusot::builtins = "seq.Seq.get"]
    fn index_logic(self, _: Int) -> Self::Item {
        absurd
    }
}

impl<T, S: ShallowModel<ShallowModelTy = Seq<T>> + ?Sized> IndexLogic<Int> for S {
    type Item = T;

    #[logic]
    #[why3::attr = "inline:trivial"]
    fn index_logic(self, ix: Int) -> Self::Item {
        pearlite! { self@.index_logic(ix) }
    }
}

impl<T, S: ShallowModel<ShallowModelTy = Seq<T>> + ?Sized> IndexLogic<usize> for S {
    type Item = T;

    #[logic]
    #[why3::attr = "inline:trivial"]
    fn index_logic(self, ix: usize) -> Self::Item {
        pearlite! { self@.index_logic(ix@) }
    }
}

impl<T> IndexLogic<Int> for Ghost<Seq<T>> {
    type Item = T;

    #[logic]
    #[trusted]
    #[creusot::builtins = "seq.Seq.get"]
    fn index_logic(self, _: Int) -> Self::Item {
        absurd
    }
}

pub trait ContainsLogic<T> {
    #[predicate]
    fn contains_log(self, element: T) -> bool;
}

impl<T> ContainsLogic<T> for Seq<T> {
    #[predicate]
    fn contains_log(self, e: T) -> bool {
        self.contains(e)
    }
}

impl<T, S: ShallowModel<ShallowModelTy = Seq<T>> + ?Sized> ContainsLogic<T> for S {
    #[predicate]
    fn contains_log(self, e: T) -> bool {
        pearlite! { self@.contains(e) }
    }
}
