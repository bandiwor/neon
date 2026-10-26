//! Escape analysis: which heap objects can only ever be seen by the function that creates them.
//!
//! Registers connected by `move` may hold the same object, so they form one *alias class*
//! (union-find over the moves). A class is
//!
//! * **fresh** when every value it can hold was allocated in this function: all definitions are
//!   allocations or moves within the class, and no register of it is a parameter or is captured by
//!   a closure (pinned);
//! * **escaping** when some register of it is used where another piece of code could keep or
//!   later reach the object: stored into the heap, a global or an upvalue, put into another object,
//!   captured, handed to a closure, or passed to a callee that retains that argument. Builtins say
//!   which arguments they retain ([`Builtin::retains_arg`]); for user functions the per-parameter
//!   summary computed here is used;
//! * **returned** when it reaches a `return`.
//!
//! A fresh class that does not escape is **local**: reads and writes of its objects cannot be told
//! apart from reads and writes of the function's own registers. (A local object may still be
//! returned: everything done to it before is invisible to the caller, who only gets the result.)
//!
//! Parameter summaries are the least solution of a monotone system: a parameter escapes if its
//! class escapes or is returned (a returned argument is aliased by the call's result), where calls
//! consult the callees' summaries. Iterating from "nothing escapes" until nothing changes terminates
//! because summaries only grow.

use super::types::*;

/// For every function, whether the object passed as each parameter may escape.
pub type ParamEscapes = Vec<Vec<bool>>;

fn find(parent: &mut [u32], mut x: u32) -> u32 {
    while parent[x as usize] != x {
        parent[x as usize] = parent[parent[x as usize] as usize];
        x = parent[x as usize];
    }
    x
}

/// Alias classes of one function and what is known about each.
#[derive(Debug)]
pub struct Classes {
    root: Vec<u32>,
    size: Vec<u32>,
    fresh: Vec<bool>,
    escapes: Vec<bool>,
    returned: Vec<bool>,
}

impl Classes {
    fn class(&self, r: VReg) -> usize {
        self.root[r.index()] as usize
    }

    /// A number identifying the alias class of `r` (equal for registers that may hold one object).
    pub fn class_id(&self, r: VReg) -> usize {
        self.class(r)
    }

    /// Only ever holds objects this function allocated, none of which escape (they may be returned).
    pub fn is_local(&self, r: VReg) -> bool {
        let c = self.class(r);
        self.fresh[c] && !self.escapes[c]
    }

    /// May the object in `r` outlive the function, by any route?
    pub fn leaves(&self, r: VReg) -> bool {
        let c = self.class(r);
        self.escapes[c] || self.returned[c]
    }

    /// Whether `r` is the only register in its class (no moves to or from it).
    pub fn is_alone(&self, r: VReg) -> bool {
        self.size[self.class(r)] == 1
    }

    /// Fresh, non-escaping and never returned: nothing outside can reach it.
    pub fn is_private(&self, r: VReg) -> bool {
        self.is_local(r) && !self.returned[self.class(r)]
    }
}

/// Calls `visit(register, escaping)` for every register read by `inst`: `escaping` is whether this
/// use may let the object outlive the instruction.
fn visit_uses(inst: &Inst, pe: &ParamEscapes, mut visit: impl FnMut(VReg, bool)) {
    use Inst::*;
    match inst {
        Const { .. } | GetUpval { .. } | GetGlobal { .. } => {}
        Move { .. } => {} // the same class
        Arith { lhs, rhs, .. } | Cmp { lhs, rhs, .. } => {
            visit(*lhs, false);
            visit(*rhs, false);
        }
        Neg { src, .. } | BitNot { src, .. } | BoolNot { src, .. } | Cast { src, .. } => visit(*src, false),
        GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => visit(*obj, false),
        SetField { obj, src, .. } => {
            visit(*obj, false);
            visit(*src, true);
        }
        ArrayGet { arr, idx, .. } => {
            visit(*arr, false);
            visit(*idx, false);
        }
        ArraySet { arr, idx, src, .. } => {
            visit(*arr, false);
            visit(*idx, false);
            visit(*src, true);
        }
        MapGet { map, key, .. } => {
            visit(*map, false);
            visit(*key, false);
        }
        MapSet { map, key, val } => {
            visit(*map, false);
            visit(*key, true);
            visit(*val, true);
        }
        NewStruct { fields, .. } | NewEnum { fields, .. } => fields.iter().for_each(|&r| visit(r, true)),
        NewArray { elems, .. } | NewSet { elems, .. } => elems.iter().for_each(|&r| visit(r, true)),
        NewMap { entries, .. } => entries.iter().for_each(|&(k, v)| {
            visit(k, true);
            visit(v, true);
        }),
        Call { func, args, .. } => {
            for (i, &a) in args.iter().enumerate() {
                let esc = pe.get(func.index()).and_then(|p| p.get(i)).copied().unwrap_or(true);
                visit(a, esc);
            }
        }
        CallClosure { callee, args, .. } => {
            visit(*callee, true);
            args.iter().for_each(|&a| visit(a, true));
        }
        MakeClosure { captures, .. } => captures.iter().for_each(|c| {
            if let Capture::Local(r) = c {
                visit(*r, true)
            }
        }),
        SetUpval { src, .. } | SetGlobal { src, .. } => visit(*src, true),
        CloseUpvals { regs } => regs.iter().for_each(|&r| visit(r, true)),
        Intrinsic { b, args, .. } => {
            for (i, &a) in args.iter().enumerate() {
                visit(a, b.retains_arg(i));
            }
        }
    }
}

/// Computes the alias classes of `f` under the parameter summaries `pe`.
pub fn classify(f: &IrFunc, pe: &ParamEscapes) -> Classes {
    let n = f.regs.len();
    let mut parent: Vec<u32> = (0..n as u32).collect();
    for b in &f.blocks {
        for i in &b.insts {
            if let Inst::Move { dst, src } = i {
                if f.reg_ty(*dst) == IrTy::Ref {
                    let (a, b) = (find(&mut parent, dst.0), find(&mut parent, src.0));
                    if a != b {
                        parent[a as usize] = b;
                    }
                }
            }
        }
    }
    let root: Vec<u32> = (0..n as u32).map(|r| find(&mut parent, r)).collect();
    let mut size = vec![0u32; n];
    for &r in &root {
        size[r as usize] += 1;
    }

    let mut fresh = vec![true; n];
    let mut escapes = vec![false; n];
    let mut returned = vec![false; n];
    for r in 0..n {
        if r < f.params as usize || f.pinned[r] || f.reg_ty(VReg(r as u32)) != IrTy::Ref {
            fresh[root[r] as usize] = false;
        }
    }
    for b in &f.blocks {
        for i in &b.insts {
            if let Some(d) = i.def() {
                let allocation = matches!(
                    i,
                    Inst::NewStruct { .. }
                        | Inst::NewEnum { .. }
                        | Inst::NewArray { .. }
                        | Inst::NewMap { .. }
                        | Inst::NewSet { .. }
                        | Inst::Move { .. }
                ) || matches!(i, Inst::Intrinsic { b, .. } if b.returns_fresh_container());
                if !allocation {
                    fresh[root[d.index()] as usize] = false;
                }
            }
            visit_uses(i, pe, |r, esc| {
                if esc {
                    escapes[root[r.index()] as usize] = true;
                }
            });
        }
        if let Term::Return(r) = b.term {
            returned[root[r.index()] as usize] = true;
        }
    }
    Classes { root, size, fresh, escapes, returned }
}

/// Per-parameter escape summaries of the whole program.
pub fn param_escapes(prog: &IrProgram) -> ParamEscapes {
    let mut pe: ParamEscapes = prog.funcs.iter().map(|f| vec![false; f.params as usize]).collect();
    loop {
        let mut changed = false;
        for (fi, f) in prog.funcs.iter().enumerate() {
            if f.params == 0 {
                continue;
            }
            let classes = classify(f, &pe);
            for (p, escapes) in pe[fi].iter_mut().enumerate() {
                if f.reg_ty(VReg(p as u32)) == IrTy::Ref && !*escapes && classes.leaves(VReg(p as u32)) {
                    *escapes = true;
                    changed = true;
                }
            }
        }
        if !changed {
            return pe;
        }
    }
}
