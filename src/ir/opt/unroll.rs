//! Full unrolling of small loops with a known trip count.
//!
//! A loop is copied [`COPIES`] times in a row (one more than the iterations it may have, for the
//! test that ends the last one): the back edges of each copy lead to the header of the next, the
//! last copy leads back to the original loop, and the edges that entered the loop now enter the first
//! copy. Registers are shared by all copies (the code runs one copy after the other, so they behave
//! exactly as in successive iterations). The result is only a candidate: constant propagation runs
//! over it, and if the exit tests of the copies all come out decided, so that the original loop can
//! no longer be reached, the loop ran at most [`MAX_TRIPS`] times and the straight-line version is
//! kept. Otherwise the candidate is discarded and the function is left as it was.
//!
//! Only loops with a handful of instructions are tried, in functions that are not already big.

use super::licm::{dominators, find_loops, reverse_post_order};
use super::sccp::sccp_func;
use super::simplify::predecessors;
use crate::ir::types::*;

/// How many iterations may be unrolled.
const MAX_TRIPS: usize = 8;
/// Each copy holds the loop test of one iteration, so one more is needed to decide the last exit.
const COPIES: usize = MAX_TRIPS + 1;
/// Loops with more instructions than this are not tried.
const MAX_LOOP_INSTS: usize = 24;
/// Functions with more instructions than this are left alone.
const MAX_FUNC_INSTS: usize = 600;
/// Attempts per function (a failed one costs a copy of the function).
const MAX_ATTEMPTS: usize = 4;

fn inst_count(f: &IrFunc) -> usize {
    f.blocks.iter().map(|b| b.insts.len()).sum()
}

fn reachable(f: &IrFunc) -> Vec<bool> {
    let mut seen = vec![false; f.blocks.len()];
    for b in reverse_post_order(f) {
        seen[b] = true;
    }
    seen
}

pub fn unroll_func(f: &mut IrFunc) -> usize {
    if inst_count(f) > MAX_FUNC_INSTS {
        return 0;
    }
    let mut done = 0;
    let mut attempts = 0;
    let mut tried: Vec<usize> = Vec::new();
    // an unrolled inner loop may make the one around it small enough
    while attempts < MAX_ATTEMPTS {
        let preds = predecessors(f);
        let rpo = reverse_post_order(f);
        let idom = dominators(f, &rpo, &preds);
        let mut progress = false;
        for lp in find_loops(f, &idom, &preds) {
            if lp.header == 0 || tried.contains(&lp.header) {
                continue;
            }
            let size: usize = lp.body.iter().map(|&b| f.blocks[b].insts.len()).sum();
            if size > MAX_LOOP_INSTS {
                continue;
            }
            tried.push(lp.header);
            attempts += 1;
            if let Some(g) = try_unroll(f, &lp, &preds, &idom) {
                *f = g;
                done += 1;
                tried.clear(); // block numbers changed meaning
                progress = true;
                break;
            }
            if attempts >= MAX_ATTEMPTS {
                break;
            }
        }
        if !progress {
            break;
        }
    }
    done
}

fn try_unroll(f: &IrFunc, lp: &super::licm::Loop, preds: &[Vec<usize>], idom: &[usize]) -> Option<IrFunc> {
    let outside: Vec<usize> =
        preds[lp.header].iter().copied().filter(|p| idom[*p] != usize::MAX && !lp.body.contains(p)).collect();
    if outside.is_empty() {
        return None;
    }
    let mut body: Vec<usize> = lp.body.iter().copied().collect();
    body.sort_unstable();
    let mut g = f.clone();

    // block numbers of each copy
    let mut copies: Vec<Vec<(usize, BlockId)>> = Vec::with_capacity(COPIES);
    for _ in 0..COPIES {
        copies.push(body.iter().map(|&b| (b, g.new_block())).collect());
    }
    let lookup = |c: usize, b: usize| copies[c].iter().find(|(o, _)| *o == b).map(|(_, n)| *n);
    for (c, blocks) in copies.iter().enumerate() {
        for &(orig, new) in blocks {
            let insts = f.blocks[orig].insts.clone();
            let mut term = f.blocks[orig].term.clone();
            for s in term.successors_mut() {
                let t = s.index();
                if t == lp.header {
                    // back edge: on to the next copy, the last one back to the original loop
                    if c + 1 < COPIES {
                        *s = lookup(c + 1, t).expect("header is in the body");
                    }
                } else if lp.body.contains(&t) {
                    *s = lookup(c, t).expect("body block");
                }
            }
            g.blocks[new.index()].insts = insts;
            g.blocks[new.index()].term = term;
        }
    }
    let first = lookup(0, lp.header).expect("header is in the body");
    for &p in &outside {
        for s in g.blocks[p].term.successors_mut() {
            if s.index() == lp.header {
                *s = first;
            }
        }
    }

    for _ in 0..3 {
        if sccp_func(&mut g) == 0 {
            break;
        }
    }
    if reachable(&g)[lp.header] {
        return None;
    }
    Some(g)
}
