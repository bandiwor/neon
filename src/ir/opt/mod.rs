//! IR optimization passes.
//!
//! * [`fold`]: constant folding and propagation (typed, trap-preserving) and
//!   copy propagation
//! * [`simplify`]: value forwarding / common subexpressions, algebraic identities, `print(a + b)`
//!   splitting and dead element loads
//! * [`sroa`]: scalar replacement of private structs/enums and removal of write-only private
//!   objects, from the escape analysis
//! * [`strings`]: constant string concatenation and merging of adjacent constant prints
//! * [`tail`]: self tail calls to loops, return duplication, branch threading, boolean diamonds
//! * [`copies`]: global copy propagation (available copies across blocks and loops)
//! * [`globals`]: stores to globals nobody reads, unused global slots
//! * [`sccp`]: constant propagation over the whole CFG, ignoring branches that are never taken
//! * [`stores`]: stores overwritten before anything can read them
//! * [`unroll`]: small loops with a constant trip count become straight-line code
//! * [`devirt`]: calls of closures whose code is known become direct calls (curried closures too)
//! * [`rotate`]: `while` loops test at the bottom; the invariant part of the test runs once
//! * [`consts`]: registers holding the same constant are merged
//! * [`licm`]: loop-invariant code motion
//! * [`dce`]: unreachable-block removal, CFG simplification and dead code
//!   elimination
//! * [`inline`]: function inlining by remapping registers and blocks
//!
//! [`optimize`] runs them to a fixed point. Every pass preserves behavior,
//! including run-time traps: an operation that would overflow is never folded
//! away.

pub mod consts;
pub mod copies;
pub mod dce;
pub mod devirt;
pub mod fold;
pub mod globals;
pub mod inline;
pub mod licm;
pub mod rotate;
pub mod sccp;
pub mod simplify;
pub mod sroa;
pub mod stores;
pub mod strings;
pub mod tail;
pub mod unroll;

use super::types::*;

#[derive(Debug, Clone, Copy)]
pub struct OptConfig {
    pub fold: bool,
    pub dce: bool,
    pub simplify: bool,
    pub licm: bool,
    /// Test `while` loops at the bottom.
    pub rotate: bool,
    /// Fully unroll small loops whose trip count is known.
    pub unroll: bool,
    pub inline: bool,
    /// Callees with at most this many instructions are inlined.
    pub inline_threshold: usize,
    /// Do not grow a function beyond this many virtual registers by inlining.
    pub max_regs: usize,
}

impl Default for OptConfig {
    fn default() -> Self {
        OptConfig {
            fold: true,
            dce: true,
            simplify: true,
            licm: true,
            rotate: true,
            unroll: true,
            inline: true,
            inline_threshold: 30,
            max_regs: 160,
        }
    }
}

impl OptConfig {
    /// No optimization at all.
    pub fn none() -> OptConfig {
        OptConfig {
            fold: false,
            dce: false,
            simplify: false,
            licm: false,
            rotate: false,
            unroll: false,
            inline: false,
            ..OptConfig::default()
        }
    }
}

/// Statistics about what the optimizer did, for tests and `--stats`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct OptStats {
    pub folded: usize,
    pub simplified: usize,
    pub hoisted: usize,
    pub unrolled: usize,
    pub rotated: usize,
    pub devirtualized: usize,
    pub inlined_calls: usize,
    pub removed_insts: usize,
    pub removed_blocks: usize,
    pub removed_funcs: usize,
}

impl OptStats {
    fn absorb(&mut self, o: &OptStats) {
        self.folded += o.folded;
        self.simplified += o.simplified;
        self.hoisted += o.hoisted;
        self.unrolled += o.unrolled;
        self.rotated += o.rotated;
        self.devirtualized += o.devirtualized;
        self.inlined_calls += o.inlined_calls;
        self.removed_insts += o.removed_insts;
        self.removed_blocks += o.removed_blocks;
        self.removed_funcs += o.removed_funcs;
    }
}

/// Runs the per-function passes on `f` until nothing changes. It only reads the rest of the program
/// (`fx`, the string table, the type descriptors), which is what lets functions run in parallel.
fn optimize_function(
    f: &mut IrFunc,
    cfg: &OptConfig,
    fx: &crate::ir::effects::FuncEffects,
    pool: &mut strings::StringPool<'_>,
    descs: &[TypeDesc],
) -> OptStats {
    let mut stats = OptStats::default();
    // loop-invariant motion comes last: it moves work out of the places the other passes
    // pattern-match on (a hoisted `len` hides the loop condition), then a cleanup round follows
    // constant propagation is the costly pass and reaches its own fixed point: it runs on
    // entry and after the passes that reshape loops, not after every small rewrite
    let mut sccp_stale = true;
    for _round in 0..3 {
        for _ in 0..8 {
            let mut changed = false;
            if cfg.fold {
                let n = fold::fold_func(f);
                stats.folded += n;
                changed |= n > 0;
            }
            if cfg.fold && sccp_stale {
                let n = sccp::sccp_func(f);
                stats.folded += n;
                changed |= n > 0;
                sccp_stale = false;
            }
            if cfg.simplify {
                let n = simplify::simplify_func(f, fx) + strings::fold_strings(pool, descs, f, fx);
                stats.simplified += n;
                changed |= n > 0;
            }
            if cfg.dce {
                let n = stores::dead_stores(f, fx);
                stats.removed_insts += n;
                changed |= n > 0;
                let (insts, blocks) = dce::dce_func(f, fx);
                stats.removed_insts += insts;
                stats.removed_blocks += blocks;
                changed |= insts + blocks > 0;
            }
            if !changed {
                break;
            }
        }
        let r = if cfg.rotate { rotate::rotate_loops(f, fx) } else { 0 };
        stats.rotated += r;
        let n = if cfg.licm { licm::licm_func(f, fx) } else { 0 } + r;
        stats.hoisted += n;
        let merged = if cfg.fold { consts::dedupe_consts(f) } else { 0 };
        stats.folded += merged;
        sccp_stale |= n + merged > 0;
        if n + merged == 0 {
            break;
        }
    }
    if cfg.dce {
        let (insts, blocks) = dce::dce_func(f, fx);
        stats.removed_insts += insts;
        stats.removed_blocks += blocks;
    }
    stats
}

/// Optimizes every function of `prog` in place.
///
/// The passes of one function never look at another function's code, only at the summaries of what
/// functions can do, so the functions are optimized in parallel. The only thing they share that they
/// can change is the string table; each adds its strings privately and the additions are merged in
/// function order afterwards, so the result is the same whatever the number of threads.
pub fn optimize(prog: &mut IrProgram, cfg: &OptConfig) -> OptStats {
    let mut stats = OptStats::default();
    let simplify = |prog: &mut IrProgram, stats: &mut OptStats| {
        // what each function can do; the passes below only remove or move code, which can only
        // shrink the real effects, so the summaries stay sound while they run
        let fx = crate::ir::effects::analyze(prog);
        let IrProgram { funcs, strings, descs, .. } = prog;
        let base_len = strings.len();
        let table: &[String] = strings;
        let index = strings::index_strings(table);
        let results = crate::par::map_mut(funcs, |_, f| {
            let mut pool = strings::StringPool::new(table, &index);
            let st = optimize_function(f, cfg, &fx, &mut pool, descs);
            (st, pool.into_added())
        });
        let mut added = Vec::with_capacity(results.len());
        for (st, a) in results {
            stats.absorb(&st);
            added.push(a);
        }
        strings::merge_added(strings, base_len, funcs, added);
    };
    simplify(prog, &mut stats);
    if cfg.simplify {
        // after the first round the call, its result move and the return are next to each other
        let any: usize = crate::par::map_mut(&mut prog.funcs, |i, f| tail::self_tail(f, FuncId(i as u32))).iter().sum();
        stats.simplified += any;
        if any > 0 {
            simplify(prog, &mut stats);
        }
    }
    if cfg.inline {
        for _ in 0..4 {
            let d = devirt::devirtualize(prog);
            stats.devirtualized += d;
            let n = inline::inline_round(prog, cfg);
            stats.inlined_calls += n;
            if n + d == 0 {
                break;
            }
            simplify(prog, &mut stats);
        }
    }
    if cfg.unroll && cfg.fold {
        let n: usize = crate::par::map_mut(&mut prog.funcs, |_, f| unroll::unroll_func(f)).iter().sum();
        stats.unrolled += n;
        if n > 0 {
            simplify(prog, &mut stats);
        }
    }
    if cfg.dce {
        if globals::prune_globals(prog) > 0 {
            simplify(prog, &mut stats);
        }
        stats.removed_funcs += dce::prune_functions(prog);
    }
    stats
}
