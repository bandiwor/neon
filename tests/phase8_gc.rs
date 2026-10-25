//! Phase 8: the collector under a simulated mutator with a restricted heap.

use neon::gc::{CellState, GcConfig, Handle, Heap, Roots};
use neon::runtime::{HeapView, Trap};

fn heap(limit: Option<usize>) -> Heap {
    Heap::new(&[], GcConfig { limit, initial_threshold: 4096, stress: false, reuse_slots: true })
}

/// Allocates like a program would: collect at "safe-points" when asked to.
fn alloc_str(h: &mut Heap, live: &[Handle], s: String) -> Result<Handle, Trap> {
    if h.should_collect() {
        let mut r = Roots::new();
        live.iter().for_each(|&x| r.handle(x));
        h.collect(&r);
    }
    h.alloc_str(s)
}

#[test]
fn endless_garbage_fits_in_a_small_heap() {
    let limit = 64 * 1024;
    let mut h = heap(Some(limit));
    let mut window: Vec<Handle> = Vec::new();
    for i in 0..200_000u32 {
        let s = alloc_str(&mut h, &window, format!("string number {i} with some padding to take space")).unwrap();
        window.push(s);
        if window.len() > 10 {
            window.remove(0);
        }
        assert!(h.bytes() <= limit);
    }
    assert!(h.stats().collections > 100, "{:?}", h.stats());
    h.check_invariants().unwrap();
    // everything the mutator still holds is intact
    for (k, &s) in window.iter().enumerate() {
        assert!(h.str_of(s).starts_with("string number"), "survivor {k}");
    }
    // a final collection leaves exactly what the mutator holds
    let mut r = Roots::new();
    window.iter().for_each(|&x| r.handle(x));
    h.collect(&r);
    assert_eq!(h.live_objects(), window.len());
}

#[test]
fn live_data_beyond_the_limit_is_out_of_memory_and_recoverable() {
    let mut h = heap(Some(32 * 1024));
    let mut keep: Vec<Handle> = Vec::new();
    let err = loop {
        match alloc_str(&mut h, &keep, "x".repeat(500)) {
            Ok(s) => keep.push(s),
            Err(e) => break e,
        }
    };
    assert_eq!(err, Trap::OutOfMemory);
    assert!(keep.len() > 20, "{}", keep.len());
    // free it all and the heap is usable again
    keep.clear();
    let s = alloc_str(&mut h, &keep, "again".into()).unwrap();
    assert_eq!(h.str_of(s), "again");
}

#[test]
fn linked_structures_and_closures_survive_repeated_collections() {
    let mut h = heap(Some(2 * 1024 * 1024));
    // a chain of arrays, each pointing to the next, held through a closed upvalue
    let mut head: Handle = h.alloc_array(vec![], true).unwrap();
    let cell = h.alloc_cell(CellState::Closed(head), true).unwrap();
    let clo = h.alloc_closure(0, vec![cell]).unwrap();
    for i in 0..5_000u64 {
        let CellState::Closed(cur) = h.cell_state(cell) else { unreachable!() };
        // slot 0 holds the previous node, so this is a reference array
        let node_refs = h.alloc_array(vec![cur], true).unwrap_or_else(|_| panic!("out of memory at {i}"));
        h.set_cell_state(cell, CellState::Closed(node_refs));
        head = node_refs;
        if h.should_collect() {
            let mut r = Roots::new();
            r.handle(clo);
            h.collect(&r);
        }
    }
    let mut r = Roots::new();
    r.handle(clo);
    h.collect(&r);
    // the chain is fully alive: 5001 arrays + the closure and its cell
    assert_eq!(h.live_objects(), 5_001 + 1);
    assert_eq!(h.live_cells(), 1);
    let mut len = 0;
    let mut cur = head;
    while cur != 0 && !h.array_of(cur).is_empty() {
        cur = h.array_of(cur)[0];
        len += 1;
    }
    assert_eq!(len, 5_000);
    h.check_invariants().unwrap();
}

#[test]
fn stress_mode_collects_every_time_and_still_keeps_roots() {
    let mut h = Heap::new(&[], GcConfig { stress: true, reuse_slots: false, ..GcConfig::default() });
    let a = h.alloc_str("a".into()).unwrap();
    let mut roots = Roots::new();
    roots.handle(a);
    for _ in 0..100 {
        assert!(h.should_collect());
        h.alloc_str("garbage".into()).unwrap();
        h.collect(&roots);
    }
    assert_eq!(h.live_objects(), 1);
    assert_eq!(h.stats().collections, 100);
}
