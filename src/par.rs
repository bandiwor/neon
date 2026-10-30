//! A tiny data-parallel helper over `std::thread::scope` (no dependencies).
//!
//! Work items are handed out one at a time from a shared queue, so a few big functions do not leave
//! the other threads idle, and the results always come back in item order: whatever runs in
//! parallel must not change what the program produces. `set_threads(1)` (`--threads 1`, `NEON_THREADS=1`) turns it off (and is what the
//! reproducibility tests compare against); on `wasm32` there are no threads and everything runs inline.

use std::sync::Mutex;

/// Below this many items the cost of starting threads is not worth it.
const MIN_ITEMS: usize = 8;
/// Stack size of a worker thread (the main thread usually has 8 MiB).
const WORKER_STACK: usize = 16 << 20;
/// More threads than this rarely help the compiler's workloads.
const MAX_THREADS: usize = 8;

use std::sync::atomic::{AtomicUsize, Ordering};

/// The thread count chosen by [`set_threads`]; 0 means "as many as the machine has".
static REQUESTED: AtomicUsize = AtomicUsize::new(0);

/// Sets how many threads the compiler may use (0 = automatic, 1 = none). The command-line tool
/// calls this from `--threads` / `NEON_THREADS`; the library itself never reads the environment.
pub fn set_threads(n: usize) {
    REQUESTED.store(n, Ordering::Relaxed);
}

/// How many threads to use for `items` work items.
pub fn workers(items: usize) -> usize {
    if cfg!(target_arch = "wasm32") || items < MIN_ITEMS {
        return 1;
    }
    let wanted = match REQUESTED.load(Ordering::Relaxed) {
        0 => available(),
        n => n.min(MAX_THREADS),
    };
    wanted.min(items).max(1)
}

#[cfg(not(target_arch = "wasm32"))]
fn available() -> usize {
    use std::sync::OnceLock;
    static N: OnceLock<usize> = OnceLock::new();
    *N.get_or_init(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(MAX_THREADS))
}

#[cfg(target_arch = "wasm32")]
fn available() -> usize {
    1
}

/// `f(i, &mut items[i])` for every item, in parallel; the results in item order.
pub fn map_mut<T: Send, R: Send>(items: &mut [T], f: impl Fn(usize, &mut T) -> R + Sync) -> Vec<R> {
    let w = workers(items.len());
    if w <= 1 {
        return items.iter_mut().enumerate().map(|(i, t)| f(i, t)).collect();
    }
    run(items.len(), w, items.iter_mut().enumerate(), f)
}

/// `f(i, &items[i])` for every item, in parallel; the results in item order.
pub fn map<T: Sync, R: Send>(items: &[T], f: impl Fn(usize, &T) -> R + Sync) -> Vec<R> {
    let w = workers(items.len());
    if w <= 1 {
        return items.iter().enumerate().map(|(i, t)| f(i, t)).collect();
    }
    run(items.len(), w, items.iter().enumerate(), f)
}

fn run<I, T, R>(n: usize, workers: usize, queue: I, f: impl Fn(usize, T) -> R + Sync) -> Vec<R>
where
    I: Iterator<Item = (usize, T)> + Send,
    R: Send,
{
    let queue = Mutex::new(queue);
    let f = &f;
    let queue = &queue;
    let mut parts: Vec<Vec<(usize, R)>> = Vec::with_capacity(workers);
    std::thread::scope(|s| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                // generous stacks: the passes recurse over expression trees
                std::thread::Builder::new()
                    .stack_size(WORKER_STACK)
                    .spawn_scoped(s, move || {
                        let mut done = Vec::new();
                        loop {
                            let next = queue.lock().unwrap_or_else(|e| e.into_inner()).next();
                            match next {
                                Some((i, t)) => done.push((i, f(i, t))),
                                None => break done,
                            }
                        }
                    })
                    .expect("cannot start a compiler thread")
            })
            .collect();
        for h in handles {
            match h.join() {
                Ok(part) => parts.push(part),
                Err(panic) => std::panic::resume_unwind(panic),
            }
        }
    });
    let mut slots: Vec<Option<R>> = (0..n).map(|_| None).collect();
    for (i, r) in parts.into_iter().flatten() {
        slots[i] = Some(r);
    }
    slots.into_iter().map(|r| r.expect("every item is processed once")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn results_keep_item_order_and_every_item_is_visited_once() {
        let mut v: Vec<u64> = (0..1000).collect();
        let out = map_mut(&mut v, |i, x| {
            *x += 1;
            (i as u64) * 2
        });
        assert_eq!(out, (0..1000u64).map(|i| i * 2).collect::<Vec<_>>());
        assert_eq!(v, (1..=1000u64).collect::<Vec<_>>());
        let sum: u64 = map(&v, |_, x| *x).iter().sum();
        assert_eq!(sum, 500500);
    }

    #[test]
    fn small_inputs_run_inline() {
        assert_eq!(workers(3), 1);
        assert_eq!(map(&[1, 2, 3], |_, x| x * 2), vec![2, 4, 6]);
    }
}
