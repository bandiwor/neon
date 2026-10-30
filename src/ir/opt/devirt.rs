//! Calls of closures whose code is known.
//!
//! A closure call goes through a heap object, the upvalue cells and an indirect jump, and hides the
//! callee from every other pass (it counts as "may do anything"). When the closure at a call is known
//! from the instructions before it in the same block, the call becomes a direct call:
//!
//! * the closure was just built with `closure f [captures]`;
//! * it is a global that is assigned once, at start-up, from a closure without captures;
//! * it is the result of a call to a function that always returns `closure f [its parameters]`
//!   (the curried `|x| => |y| => x + y`).
//!
//! A closure that captures variables is called through a *lifted* copy of its function: the captured
//! values follow the ordinary parameters and `up<i>` reads become moves from them. This is only
//! done when nothing ever assigns the captured variables (neither the closure, nor any other closure
//! that shares them, nor the code between creating the closure and calling it), because the copy
//! sees the values as they were at the call.

use super::simplify::def_counts;
use crate::fx::FxHashMap as HashMap;
use crate::ir::types::*;

/// For every function, which of its upvalues are assigned by it or by a closure it passes them to.
fn mutated_upvals(prog: &IrProgram) -> Vec<Vec<bool>> {
    let mut m: Vec<Vec<bool>> = prog.funcs.iter().map(|f| vec![false; f.upvals.len()]).collect();
    loop {
        let mut changed = false;
        for (fi, f) in prog.funcs.iter().enumerate() {
            for i in f.blocks.iter().flat_map(|b| &b.insts) {
                let hit = match i {
                    Inst::SetUpval { idx, .. } => Some(*idx as usize),
                    Inst::MakeClosure { func, captures, .. } => {
                        captures.iter().enumerate().find_map(|(j, c)| match c {
                            Capture::Upvalue(u) if m[func.index()].get(j) == Some(&true) => Some(*u as usize),
                            _ => None,
                        })
                    }
                    _ => None,
                };
                if let Some(u) = hit {
                    if !m[fi][u] {
                        m[fi][u] = true;
                        changed = true;
                    }
                }
            }
        }
        if !changed {
            return m;
        }
    }
}

/// Registers of `f` that some closure created in it may assign.
fn assigned_by_closures(f: &IrFunc, mutated: &[Vec<bool>]) -> Vec<bool> {
    let mut out = vec![false; f.regs.len()];
    for i in f.blocks.iter().flat_map(|b| &b.insts) {
        if let Inst::MakeClosure { func, captures, .. } = i {
            for (j, c) in captures.iter().enumerate() {
                if let Capture::Local(q) = c {
                    if mutated[func.index()].get(j) == Some(&true) {
                        out[q.index()] = true;
                    }
                }
            }
        }
    }
    out
}

/// A function that always returns the closure `func` over some of its own (never assigned) parameters.
struct Returns {
    func: FuncId,
    params: Vec<usize>,
}

fn liftable(mutated: &[Vec<bool>], func: FuncId) -> bool {
    !mutated[func.index()].iter().any(|&m| m)
}

fn returned_closures(prog: &IrProgram, mutated: &[Vec<bool>]) -> Vec<Option<Returns>> {
    prog.funcs
        .iter()
        .map(|f| {
            let defs = def_counts(f);
            let shared = assigned_by_closures(f, mutated);
            let mut found: Option<Returns> = None;
            for b in &f.blocks {
                let crate::ir::types::Term::Return(r) = b.term else { continue };
                if r.index() < f.params as usize || defs[r.index()] != 1 {
                    return None;
                }
                let def = f.blocks.iter().flat_map(|b| &b.insts).find(|i| i.def() == Some(r))?;
                let Inst::MakeClosure { func, captures, .. } = def else { return None };
                if !liftable(mutated, *func) || prog.funcs[func.index()].upvals.len() != captures.len() {
                    return None;
                }
                let mut params = Vec::new();
                for c in captures {
                    match c {
                        Capture::Local(q)
                            if q.index() < f.params as usize && defs[q.index()] == 1 && !shared[q.index()] =>
                        {
                            params.push(q.index())
                        }
                        _ => return None,
                    }
                }
                match &found {
                    None => found = Some(Returns { func: *func, params }),
                    Some(have) if have.func == *func && have.params == params => {}
                    Some(_) => return None,
                }
            }
            found
        })
        .collect()
}

/// Globals assigned exactly once, at start-up, from a closure without captures.
fn global_closures(prog: &IrProgram) -> HashMap<GlobalSlot, FuncId> {
    let mut stores = vec![0u32; prog.globals.len()];
    for f in &prog.funcs {
        for i in f.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::SetGlobal { slot, .. } = i {
                stores[slot.index()] += 1;
            }
        }
    }
    let entry = &prog.funcs[prog.entry.index()];
    let defs = def_counts(entry);
    let mut out = HashMap::default();
    for i in entry.blocks.iter().flat_map(|b| &b.insts) {
        let Inst::SetGlobal { slot, src } = i else { continue };
        if stores[slot.index()] != 1 || defs[src.index()] != 1 {
            continue;
        }
        let def = entry.blocks.iter().flat_map(|b| &b.insts).find(|d| d.def() == Some(*src));
        if let Some(Inst::MakeClosure { func, captures, .. }) = def {
            if captures.is_empty() {
                out.insert(*slot, *func);
            }
        }
    }
    out
}

/// The copy of closure function `t` that takes its upvalues as leading parameters.
fn lift(t: &IrFunc) -> IrFunc {
    let p = t.params as usize;
    let c = t.upvals.len();
    let remap = |r: VReg| if r.index() < p { r } else { VReg(r.0 + c as u32) };
    let mut g = t.clone();
    g.regs = t.regs[..p].iter().chain(&t.upvals).chain(&t.regs[p..]).copied().collect();
    g.pinned = t.pinned[..p]
        .iter()
        .copied()
        .chain(std::iter::repeat_n(false, c))
        .chain(t.pinned[p..].iter().copied())
        .collect();
    g.params = (p + c) as u32;
    let mut newly_pinned = Vec::new();
    for b in &mut g.blocks {
        for i in &mut b.insts {
            i.for_each_use_mut(|r| *r = remap(*r));
            if let Some(d) = i.def_mut() {
                *d = remap(*d);
            }
            match i {
                Inst::GetUpval { dst, idx } => *i = Inst::Move { dst: *dst, src: VReg((p as u32) + *idx) },
                Inst::MakeClosure { captures, .. } => {
                    for cap in captures.iter_mut() {
                        if let Capture::Upvalue(u) = *cap {
                            let q = VReg(p as u32 + u);
                            *cap = Capture::Local(q);
                            newly_pinned.push(q);
                        }
                    }
                }
                _ => {}
            }
        }
        match &mut b.term {
            Term::Branch { cond: r, .. } | Term::BranchTag { obj: r, .. } | Term::Return(r) => *r = remap(*r),
            _ => {}
        }
    }
    for q in newly_pinned {
        g.pinned[q.index()] = true;
    }
    g.upvals.clear();
    g.is_closure = false;
    g.name = format!("{}$lift", t.name);
    g
}

#[derive(Clone)]
struct Known {
    func: FuncId,
    /// Registers of the current function that hold the captured values.
    caps: Vec<VReg>,
}

pub fn devirtualize(prog: &mut IrProgram) -> usize {
    // nothing to do in a program that never calls a closure
    let calls_closures =
        prog.funcs.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).any(|i| matches!(i, Inst::CallClosure { .. }));
    if !calls_closures {
        return 0;
    }
    let mutated = mutated_upvals(prog);
    let rets = returned_closures(prog, &mutated);
    let globals = global_closures(prog);
    let mut lifted: HashMap<String, FuncId> = HashMap::default();
    for (i, f) in prog.funcs.iter().enumerate() {
        if let Some(base) = f.name.strip_suffix("$lift") {
            lifted.insert(base.to_string(), FuncId(i as u32));
        }
    }
    let mut total = 0;
    for fi in 0..prog.funcs.len() {
        let shared = assigned_by_closures(&prog.funcs[fi], &mutated);
        let mut blocks = std::mem::take(&mut prog.funcs[fi].blocks);
        for b in &mut blocks {
            let mut known: HashMap<VReg, Known> = HashMap::default();
            for inst in b.insts.iter_mut() {
                // a closure call with a known target
                if let Inst::CallClosure { dst, callee, args } = inst {
                    if let Some(k) = known.get(callee).cloned() {
                        let t = k.func;
                        if t.index() != fi {
                            let target = if prog.funcs[t.index()].upvals.is_empty() {
                                t
                            } else {
                                let name = prog.funcs[t.index()].name.clone();
                                match lifted.get(&name) {
                                    Some(&id) => id,
                                    None => {
                                        let id = FuncId(prog.funcs.len() as u32);
                                        let l = lift(&prog.funcs[t.index()]);
                                        prog.funcs.push(l);
                                        lifted.insert(name, id);
                                        id
                                    }
                                }
                            };
                            let mut all = args.clone();
                            all.extend(k.caps.iter().copied());
                            *inst = Inst::Call { dst: *dst, func: target, args: all };
                            total += 1;
                        }
                    }
                }
                let def = inst.def();
                if let Some(d) = def {
                    known.remove(&d);
                    known.retain(|_, k| !k.caps.contains(&d));
                }
                let Some(d) = def else { continue };
                let fact = match &*inst {
                    Inst::MakeClosure { func, captures, .. }
                        if func.index() != fi
                            && liftable(&mutated, *func)
                            && prog.funcs[func.index()].upvals.len() == captures.len() =>
                    {
                        let caps: Option<Vec<VReg>> = captures
                            .iter()
                            .map(|c| match c {
                                Capture::Local(q) if !shared[q.index()] => Some(*q),
                                _ => None,
                            })
                            .collect();
                        caps.map(|caps| Known { func: *func, caps })
                    }
                    Inst::GetGlobal { slot, .. } => globals.get(slot).map(|&func| Known { func, caps: Vec::new() }),
                    Inst::Move { src, .. } => known.get(src).cloned(),
                    Inst::Call { func, args, .. } => match rets.get(func.index()) {
                        Some(Some(r)) => {
                            Some(Known { func: r.func, caps: r.params.iter().map(|&p| args[p]).collect() })
                        }
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(k) = fact {
                    if !k.caps.contains(&d) {
                        known.insert(d, k);
                    }
                }
            }
        }
        prog.funcs[fi].blocks = blocks;
    }
    total
}
