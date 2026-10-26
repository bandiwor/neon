//! Effect analysis of IR code: what an instruction, a function and a whole call can do.
//!
//! * [`Inst::effects`] classifies one instruction. A call is as effectful as its callee.
//! * [`analyze`] summarizes every function: the union of the effects of its body and, through the
//!   call graph, of everything it calls. Recursion and loops add `DIVERGES` (they may not
//!   terminate); an unknown callee (a closure) has every effect.
//!
//! The summary is a least fixed point of a monotone system over a finite lattice (see
//! [`crate::effects`]): `effects(f) = local(f) ∪ ⋃ effects(callee)`. Instead of iterating until
//! nothing changes, the callee-before-caller order of the strongly connected components of the call
//! graph is used, so each component is solved once (and only the cycles iterate).

use super::types::*;
use crate::effects::{Effects, Purity};
use crate::runtime::numeric::ArithOp;

/// Per-function effect summaries, indexed by function id. Unknown functions have every effect.
#[derive(Debug, Clone, Default)]
pub struct FuncEffects {
    per_func: Vec<Effects>,
    /// `[function][parameter]`: may the object passed there escape (see [`super::escape`])?
    params_escape: super::escape::ParamEscapes,
}

impl FuncEffects {
    /// No knowledge about any function: every call is as bad as it gets.
    pub fn unknown() -> FuncEffects {
        FuncEffects::default()
    }

    pub fn of(&self, f: FuncId) -> Effects {
        self.per_func.get(f.index()).copied().unwrap_or(Effects::ALL)
    }

    /// May the object passed as parameter `i` of `f` outlive the call? Unknown means yes.
    pub fn param_escapes(&self, f: FuncId, i: usize) -> bool {
        self.params_escape.get(f.index()).and_then(|p| p.get(i)).copied().unwrap_or(true)
    }

    pub fn escapes(&self) -> &super::escape::ParamEscapes {
        &self.params_escape
    }

    pub fn purity(&self, f: FuncId) -> Purity {
        self.of(f).purity()
    }

    pub fn len(&self) -> usize {
        self.per_func.len()
    }

    pub fn is_empty(&self) -> bool {
        self.per_func.is_empty()
    }
}

impl Inst {
    /// What executing this instruction can do. Calls use the summaries in `fx`.
    pub fn effects(&self, fx: &FuncEffects) -> Effects {
        use Inst::*;
        match self {
            Const { .. } | Move { .. } | Cmp { .. } | BoolNot { .. } | BitNot { .. } | Cast { .. } | EnumTag { .. } => {
                Effects::NONE
            }
            Arith { op, ty, .. } => {
                if ty.is_int() && !matches!(op, ArithOp::And | ArithOp::Or | ArithOp::Xor) {
                    Effects::TRAPS
                } else {
                    Effects::NONE
                }
            }
            Neg { ty, .. } => {
                if ty.is_int() {
                    Effects::TRAPS
                } else {
                    Effects::NONE
                }
            }
            NewStruct { .. }
            | NewEnum { .. }
            | NewArray { .. }
            | NewMap { .. }
            | NewSet { .. }
            | MakeClosure { .. } => Effects::ALLOCS,
            GetField { .. } | GetUpval { .. } | GetGlobal { .. } => Effects::READS,
            // the payload of a variant that the code has not checked is the variant's business
            EnumField { .. } => Effects::FRAGILE,
            ArrayGet { .. } | MapGet { .. } => Effects::READS.union(Effects::TRAPS),
            SetField { .. } | MapSet { .. } | SetUpval { .. } | SetGlobal { .. } | CloseUpvals { .. } => {
                Effects::WRITES
            }
            ArraySet { .. } => Effects::WRITES.union(Effects::TRAPS),
            Call { func, .. } => fx.of(*func),
            CallClosure { .. } => Effects::ALL,
            Intrinsic { b, .. } => b.effects(),
        }
    }

    /// Dropping this instruction when its result is unused changes nothing.
    pub fn removable(&self, fx: &FuncEffects) -> bool {
        self.effects(fx).removable()
    }

    /// Whether it may write to memory other code can read.
    pub fn writes_memory(&self, fx: &FuncEffects) -> bool {
        self.effects(fx).writes()
    }
}

/// Whether `f` has a cycle in its control-flow graph.
fn has_loop(f: &IrFunc) -> bool {
    // iterative DFS with the usual white/grey/black colouring
    let n = f.blocks.len();
    let mut colour = vec![0u8; n];
    let mut stack = vec![(0usize, 0usize)];
    if n == 0 {
        return false;
    }
    colour[0] = 1;
    while let Some((b, i)) = stack.pop() {
        let succ = f.blocks[b].term.successors();
        if i < succ.len() {
            stack.push((b, i + 1));
            let s = succ[i].index();
            match colour[s] {
                0 => {
                    colour[s] = 1;
                    stack.push((s, 0));
                }
                1 => return true,
                _ => {}
            }
        } else {
            colour[b] = 2;
        }
    }
    false
}

/// The object an instruction reads or writes, when it only touches that one object's own slots.
fn touched_object(i: &Inst) -> Option<VReg> {
    match i {
        Inst::GetField { obj, .. } | Inst::SetField { obj, .. } => Some(*obj),
        Inst::ArrayGet { arr, .. } | Inst::ArraySet { arr, .. } => Some(*arr),
        Inst::MapGet { map, .. } | Inst::MapSet { map, .. } => Some(*map),
        Inst::Intrinsic { b, args, .. } if b.is_container_op() => args.first().copied(),
        _ => None,
    }
}

/// Effects of a function's own instructions, ignoring what its callees do (but a loop may not
/// terminate). Reads and writes of objects the function allocates and keeps to itself do not count:
/// nothing outside can tell.
fn local_effects(f: &IrFunc, escapes: &super::escape::ParamEscapes) -> Effects {
    let none = FuncEffects::unknown();
    let classes = super::escape::classify(f, escapes);
    let mut e = Effects::NONE;
    for b in &f.blocks {
        for i in &b.insts {
            if matches!(i, Inst::Call { .. }) {
                continue;
            }
            let mut ie = i.effects(&none);
            if touched_object(i).is_some_and(|o| classes.is_local(o)) {
                ie = ie.without(Effects::READS.union(Effects::WRITES));
            }
            e |= ie;
        }
    }
    if has_loop(f) {
        e |= Effects::DIVERGES;
    }
    e
}

/// Direct callees of every function.
fn call_graph(prog: &IrProgram) -> Vec<Vec<usize>> {
    prog.funcs
        .iter()
        .map(|f| {
            let mut callees: Vec<usize> = f
                .blocks
                .iter()
                .flat_map(|b| &b.insts)
                .filter_map(|i| match i {
                    Inst::Call { func, .. } => Some(func.index()),
                    _ => None,
                })
                .collect();
            callees.sort_unstable();
            callees.dedup();
            callees
        })
        .collect()
}

/// Strongly connected components in callee-first order (Tarjan; iterative).
fn components(graph: &[Vec<usize>]) -> Vec<Vec<usize>> {
    let n = graph.len();
    let mut index = vec![usize::MAX; n];
    let mut low = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut stack: Vec<usize> = Vec::new();
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut next = 0;
    for root in 0..n {
        if index[root] != usize::MAX {
            continue;
        }
        let mut work: Vec<(usize, usize)> = vec![(root, 0)];
        while let Some(&(v, i)) = work.last() {
            if i == 0 {
                index[v] = next;
                low[v] = next;
                next += 1;
                stack.push(v);
                on_stack[v] = true;
            }
            if i < graph[v].len() {
                work.last_mut().expect("non-empty").1 += 1;
                let w = graph[v][i];
                if index[w] == usize::MAX {
                    work.push((w, 0));
                } else if on_stack[w] {
                    low[v] = low[v].min(index[w]);
                }
            } else {
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    let mut comp = Vec::new();
                    loop {
                        let w = stack.pop().expect("component member");
                        on_stack[w] = false;
                        comp.push(w);
                        if w == v {
                            break;
                        }
                    }
                    out.push(comp);
                }
            }
        }
    }
    out
}

/// Computes the effect summary of every function of `prog`.
pub fn analyze(prog: &IrProgram) -> FuncEffects {
    let graph = call_graph(prog);
    let params_escape = super::escape::param_escapes(prog);
    let local: Vec<Effects> = prog.funcs.iter().map(|f| local_effects(f, &params_escape)).collect();
    let mut summary = local.clone();
    // Tarjan yields components callees first, so a component only needs its callees' final values
    for comp in components(&graph) {
        let recursive = comp.len() > 1 || graph[comp[0]].contains(&comp[0]);
        // the effects every member of a cycle shares: its own plus those of everything it calls
        let mut shared = if recursive { Effects::DIVERGES } else { Effects::NONE };
        for &f in &comp {
            shared |= local[f];
            for &c in &graph[f] {
                if !comp.contains(&c) {
                    shared |= summary[c];
                }
            }
        }
        for &f in &comp {
            summary[f] = shared;
        }
    }
    FuncEffects { per_func: summary, params_escape }
}

/// One line per function: purity, effects, size. For `neon ir --effects`.
pub fn report(prog: &IrProgram, fx: &FuncEffects) -> String {
    let mut out = String::new();
    for (i, f) in prog.funcs.iter().enumerate() {
        let e = fx.of(FuncId(i as u32));
        out.push_str(&format!("f{i} {:<24} {:<6} {}\n", f.name, e.purity(), e));
    }
    out
}
