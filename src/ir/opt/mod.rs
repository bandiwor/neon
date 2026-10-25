//! IR optimization passes.
//!
//! * [`fold`]: constant folding and propagation (typed, trap-preserving) and
//!   copy propagation
//! * [`simplify`]: value forwarding / common subexpressions, algebraic identities, `print(a + b)`
//!   splitting and dead element loads
//! * [`licm`]: loop-invariant code motion
//! * [`dce`]: unreachable-block removal, CFG simplification and dead code
//!   elimination
//! * [`inline`]: function inlining by remapping registers and blocks
//!
//! [`optimize`] runs them to a fixed point. Every pass preserves behavior,
//! including run-time traps: an operation that would overflow is never folded
//! away.

pub mod dce;
pub mod fold;
pub mod inline;
pub mod licm;
pub mod simplify;

use super::types::*;

#[derive(Debug, Clone, Copy)]
pub struct OptConfig {
    pub fold: bool,
    pub dce: bool,
    pub simplify: bool,
    pub licm: bool,
    pub inline: bool,
    /// Callees with at most this many instructions are inlined.
    pub inline_threshold: usize,
    /// Do not grow a function beyond this many virtual registers by inlining.
    pub max_regs: usize,
}

impl Default for OptConfig {
    fn default() -> Self {
        OptConfig { fold: true, dce: true, simplify: true, licm: true, inline: true, inline_threshold: 30, max_regs: 160 }
    }
}

impl OptConfig {
    /// No optimization at all.
    pub fn none() -> OptConfig {
        OptConfig { fold: false, dce: false, simplify: false, licm: false, inline: false, ..OptConfig::default() }
    }
}

/// Statistics about what the optimizer did, for tests and `--stats`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OptStats {
    pub folded: usize,
    pub simplified: usize,
    pub hoisted: usize,
    pub inlined_calls: usize,
    pub removed_insts: usize,
    pub removed_blocks: usize,
    pub removed_funcs: usize,
}

/// Optimizes every function of `prog` in place.
pub fn optimize(prog: &mut IrProgram, cfg: &OptConfig) -> OptStats {
    let mut stats = OptStats::default();
    let simplify = |prog: &mut IrProgram, stats: &mut OptStats| {
        for f in &mut prog.funcs {
            // loop-invariant motion comes last: it moves work out of the places the other passes
            // pattern-match on (a hoisted `len` hides the loop condition), then a cleanup round follows
            for _round in 0..2 {
                for _ in 0..8 {
                    let mut changed = false;
                    if cfg.fold {
                        let n = fold::fold_func(f);
                        stats.folded += n;
                        changed |= n > 0;
                    }
                    if cfg.simplify {
                        let n = simplify::simplify_func(f);
                        stats.simplified += n;
                        changed |= n > 0;
                    }
                    if cfg.dce {
                        let (insts, blocks) = dce::dce_func(f);
                        stats.removed_insts += insts;
                        stats.removed_blocks += blocks;
                        changed |= insts + blocks > 0;
                    }
                    if !changed {
                        break;
                    }
                }
                let n = if cfg.licm { licm::licm_func(f) } else { 0 };
                stats.hoisted += n;
                if n == 0 {
                    break;
                }
            }
        }
    };
    simplify(prog, &mut stats);
    if cfg.inline {
        for _ in 0..3 {
            let n = inline::inline_round(prog, cfg);
            stats.inlined_calls += n;
            if n == 0 {
                break;
            }
            simplify(prog, &mut stats);
        }
    }
    if cfg.dce {
        stats.removed_funcs += dce::prune_functions(prog);
    }
    stats
}
