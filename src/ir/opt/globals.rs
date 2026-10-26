//! Whole-program clean-up of global variables: a global that nothing ever reads needs no stores,
//! and a global nothing touches needs no slot.

use crate::ir::types::*;

/// Drops stores to globals that are never read and renumbers the remaining slots. Returns the
/// number of stores and slots removed.
pub fn prune_globals(prog: &mut IrProgram) -> usize {
    let n = prog.globals.len();
    if n == 0 {
        return 0;
    }
    let mut read = vec![false; n];
    for f in &prog.funcs {
        for i in f.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::GetGlobal { slot, .. } = i {
                read[slot.index()] = true;
            }
        }
    }
    let mut removed = 0;
    let mut touched = read.clone();
    for f in &mut prog.funcs {
        for b in &mut f.blocks {
            let before = b.insts.len();
            b.insts.retain(|i| !matches!(i, Inst::SetGlobal { slot, .. } if !read[slot.index()]));
            removed += before - b.insts.len();
        }
    }
    // slots still mentioned after the stores are gone (none, by construction, if unread)
    for f in &prog.funcs {
        for i in f.blocks.iter().flat_map(|b| &b.insts) {
            if let Inst::SetGlobal { slot, .. } = i {
                touched[slot.index()] = true;
            }
        }
    }
    if touched.iter().all(|&t| t) {
        return removed;
    }
    let mut remap = vec![u32::MAX; n];
    let mut next = 0;
    for (i, &t) in touched.iter().enumerate() {
        if t {
            remap[i] = next;
            next += 1;
        }
    }
    removed += n - next as usize;
    let old = std::mem::take(&mut prog.globals);
    prog.globals = old.into_iter().enumerate().filter(|(i, _)| touched[*i]).map(|(_, g)| g).collect();
    for f in &mut prog.funcs {
        for b in &mut f.blocks {
            for i in &mut b.insts {
                match i {
                    Inst::GetGlobal { slot, .. } | Inst::SetGlobal { slot, .. } => slot.0 = remap[slot.index()],
                    _ => {}
                }
            }
        }
    }
    removed
}
