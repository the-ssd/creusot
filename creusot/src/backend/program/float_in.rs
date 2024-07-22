use std::collections::HashSet;

use why3::{
    coma::{self, Arg, Param, Term, Var},
    ty::Type,
    Ident, QName,
};

pub enum ExprWFVKind {
    Lambda(Vec<Param>, Box<ExprWFV>),

    Defn(Box<ExprWFV>, bool, Vec<DefnWFV>),

    Let(Box<ExprWFV>, Vec<Var>),

    Symbol(QName),

    App(Box<ExprWFV>, Box<ArgWFV>),

    Assign(Box<ExprWFV>, Vec<(Ident, Term)>),

    Assert(Box<Term>, Box<ExprWFV>),

    Assume(Box<Term>, Box<ExprWFV>),

    BlackBox(Box<ExprWFV>),

    WhiteBox(Box<ExprWFV>),

    Any,
}

struct ExprWFV {
    fvs: HashSet<QName>,
    kind: ExprWFVKind,
}

struct ArgWFV {
    fvs: HashSet<QName>,
    kind: ArgKind,
}

pub enum ArgKind {
    Ty(Type),
    Term(Term),
    Ref(Ident),
    Cont(ExprWFV),
}

pub struct DefnWFV {
    pub name: Ident,
    pub writes: Vec<Ident>,
    pub params: Vec<Param>,
    pub body: ExprWFV,
    pub fvs: HashSet<QName>,
}

fn annot_arg(arg: coma::Arg) -> ArgWFV {
    match arg {
        Arg::Ty(ty) => ArgWFV { fvs: Default::default(), kind: ArgKind::Ty(ty) },
        Arg::Term(t) => {
            ArgWFV { fvs: t.fvs().into_iter().map(|i| i.into()).collect(), kind: ArgKind::Term(t) }
        }
        Arg::Ref(i) => {
            ArgWFV { fvs: vec![i.clone().into()].into_iter().collect(), kind: ArgKind::Ref(i) }
        }
        Arg::Cont(e) => {
            let e = annot(e);
            ArgWFV { fvs: e.fvs.clone(), kind: ArgKind::Cont(e) }
        }
    }
}
fn annot_def(def: coma::Defn) -> DefnWFV {
    let e = annot(def.body);

    let mut fvs = e.fvs.clone();
    def.params.iter().for_each(|p| match p {
        Param::Ty(_) => (),
        Param::Term(i, _) => {
            fvs.remove(&i.clone().into());
        }
        Param::Reference(i, _) => {
            fvs.remove(&i.clone().into());
        }
        Param::Cont(i, _, _) => {
            fvs.remove(&i.clone().into());
        }
    });

    DefnWFV { name: def.name, writes: def.writes, params: def.params, body: e, fvs }
}

fn annot(expr: coma::Expr) -> ExprWFV {
    match expr {
        coma::Expr::Symbol(v) => {
            ExprWFV { fvs: [v.clone()].into_iter().collect(), kind: ExprWFVKind::Symbol(v) }
        }
        coma::Expr::App(l, r) => {
            let l = annot(*l);
            let r = annot_arg(*r);
            let mut fvs = l.fvs.clone();
            fvs.extend(r.fvs.clone());

            ExprWFV { fvs, kind: ExprWFVKind::App(Box::new(l), Box::new(r)) }
        }
        coma::Expr::Lambda(bndrs, e) => {
            let e = annot(*e);

            let mut fvs = e.fvs.clone();

            bndrs.iter().for_each(|p| match p {
                Param::Ty(_) => todo!(),
                Param::Term(i, _) => {
                    fvs.remove(&i.clone().into());
                }
                Param::Reference(i, _) => {
                    fvs.remove(&i.clone().into());
                }
                Param::Cont(i, _, _) => {
                    fvs.remove(&i.clone().into());
                }
            });

            ExprWFV { fvs, kind: ExprWFVKind::Lambda(bndrs, Box::new(e)) }
        }
        coma::Expr::Defn(e, b, defs) => {
            let e = annot(*e);
            let defs: Vec<_> = defs.into_iter().map(annot_def).collect();

            let mut fvs: HashSet<_> = Default::default();

            defs.iter().for_each(|def| fvs.extend(def.fvs.clone()));

            ExprWFV { fvs, kind: ExprWFVKind::Defn(Box::new(e), b, defs) }
        }
        coma::Expr::Assign(e, ts) => {
            let e = annot(*e);
            let mut fvs = e.fvs.clone();
            ts.iter().for_each(|(id, exp)| {
                fvs.insert(id.clone().into());
                fvs.extend(exp.fvs().into_iter().map(|i| i.into()))
            });

            ExprWFV { fvs, kind: ExprWFVKind::Assign(Box::new(e), ts) }
        }
        coma::Expr::Let(e, vars) => {
            let e = annot(*e);

            let mut fvs = e.fvs.clone();
            vars.iter().for_each(|Var(id, _, e, _)| {
                fvs.insert(id.clone().into());
                fvs.extend(e.fvs().into_iter().map(|i| i.into()))
            });

            ExprWFV { fvs, kind: ExprWFVKind::Let(Box::new(e), vars) }
        }
        coma::Expr::Assert(t, e) => {
            let e = annot(*e);
            let fvs = t.fvs().into_iter().map(|i| i.into()).chain(e.fvs.clone()).collect();

            ExprWFV { fvs, kind: ExprWFVKind::Assert(t, Box::new(e)) }
        }
        coma::Expr::Assume(t, e) => {
            let e = annot(*e);
            let fvs = t.fvs().into_iter().map(|i| i.into()).chain(e.fvs.clone()).collect();

            ExprWFV { fvs, kind: ExprWFVKind::Assume(t, Box::new(e)) }
        }
        coma::Expr::BlackBox(e) => {
            let e = annot(*e);
            ExprWFV { fvs: e.fvs.clone(), kind: ExprWFVKind::BlackBox(Box::new(e)) }
        }
        coma::Expr::WhiteBox(e) => {
            let e = annot(*e);
            ExprWFV { fvs: e.fvs.clone(), kind: ExprWFVKind::WhiteBox(Box::new(e)) }
        }
        coma::Expr::Any => ExprWFV { fvs: Default::default(), kind: ExprWFVKind::Any },
    }
}

fn float_in(mut lets: Vec<Var>, mut expr: ExprWFV) -> coma::Expr {
    use coma::*;
    match expr.kind {
        ExprWFVKind::Lambda(pars, body) => {
            let fvs = expr.fvs.difference(&body.fvs).cloned().collect();
            wrap_floats(lets, fvs, |inner| Expr::Lambda(pars, Box::new(float_in(inner, *body))))
        }
        ExprWFVKind::Defn(e, b, defs) => {
            let (common, mut def_lets) = split_for_case(lets,  defs.iter().map(|d| d.fvs.clone()).collect());
            let e = float_in(common, *e);
            let defs: Vec<_> = defs.into_iter().map(|d| float_in_defn(def_lets.pop().unwrap(), d)).collect();

            Expr::Defn(Box::new(e), b, defs)
        }
        ExprWFVKind::Let(e, l) => {
            lets.extend(l);
            let e = float_in(lets, *e);
            e
        }
        ExprWFVKind::Symbol(s) => Expr::Let(Box::new(Expr::Symbol(s)), lets),
        ExprWFVKind::App(l, r) => {
            let (common, mut defs) = split_for_case(lets, vec![l.fvs.clone(), r.fvs.clone()]);

            // let (shared, lefts, rights) = todo!();

            let l = float_in(defs.remove(0), *l);
            let r = float_in_arg(defs.remove(0), *r);

            let app = l.app(vec![r]);
            Expr::Let(Box::new(app), common)
        }
        ExprWFVKind::Assign(e, asgns) => {
            let fvs = expr.fvs.difference(&e.fvs).cloned().collect();
            wrap_floats(lets, fvs, |inner| Expr::Assign(Box::new(float_in(inner, *e)), asgns))
        }
        ExprWFVKind::Assert(t, e) => {
            let fvs = expr.fvs.difference(&e.fvs).cloned().collect();
            wrap_floats(lets, fvs, |inner| Expr::Assert(t, Box::new(float_in(inner, *e))))
        }
        ExprWFVKind::Assume(t, e) => {
            let fvs = expr.fvs.difference(&e.fvs).cloned().collect();
            wrap_floats(lets, fvs, |inner| Expr::Assume(t, Box::new(float_in(inner, *e))))
        }
        ExprWFVKind::BlackBox(e) => Expr::BlackBox(Box::new(float_in(lets, *e))),
        ExprWFVKind::WhiteBox(e) => Expr::WhiteBox(Box::new(float_in(lets, *e))),
        ExprWFVKind::Any => Expr::Any,
    }
}

fn float_in_defn(lets: Vec<Var>, defn: DefnWFV) -> coma::Defn {
    let DefnWFV { name, writes, params, body, fvs } = defn;
    let fvs = fvs.difference(&body.fvs).cloned().collect();
    let body = wrap_floats(lets, fvs, |inner| float_in(inner, body));
    coma::Defn { name, writes, params, body }
}

fn split_for_case(
    lets: Vec<Var>,
    // _: HashSet<QName>,
    defs: Vec<HashSet<QName>>,
) -> (Vec<Var>, Vec<Vec<Var>>) {
    let mut common = Vec::new();
    let mut branches = vec![ Vec::new(); defs.len()];
    for v in lets {
        let occurs : Vec<_> = defs.iter().enumerate().filter(|(_, d)| d.contains(&v.0.clone().into())).map(|(ix, _)| ix).collect();
        
        let here = occurs.len() == 1; 
        if here {
            common.push(v);

        } else {
            branches[occurs[0]].push(v)

        }
    }
    (common, branches)
}

fn float_in_arg(mut lets: Vec<Var>, mut expr: ArgWFV) -> coma::Arg {
    todo!()
}

fn wrap_floats(
    lets: Vec<Var>,
    fvs: HashSet<QName>,
    f: impl FnOnce(Vec<Var>) -> coma::Expr,
) -> coma::Expr {
    let (floats, rest) = split_floats(lets, &fvs);
    let e = f(rest);
    coma::Expr::Let(Box::new(e), floats)
}

fn split_floats(lets: Vec<Var>, fvs: &HashSet<QName>) -> (Vec<Var>, Vec<Var>) {
    // let mut floats = vec![];
    // let mut rest = vec![];

    // for Var(id, ty, e, _) in lets {
    //     if e.fvs().iter().any(|i| lets.iter().any(|Var(id, _, _, _)| id == i)) {
    //         floats.push(Var(id, ty, e, false))
    //     } else {
    //         rest.push(Var(id, ty, e, false))
    //     }
    // }

    // (floats, rest)
    todo!()
}
