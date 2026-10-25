use crate::fx::FxHashSet as HashSet;

use super::*;
use crate::bytecode::{BytecodeProgram, Chunk};
use crate::ir::{DescId, GlobalInfo, IrTy, TypeDesc};
use crate::runtime::numeric::NumTy;
use crate::runtime::{HeapView, Trap};

// descriptor ids used by the tests
const D_INT: u32 = 0;
const D_STR: u32 = 1;
const D_NODE: u32 = 2; // struct Node { val: i64, name: string, next: Node }
const D_SHAPE: u32 = 3; // enum Shape { Dot, Pair(i64, string) }

fn descs() -> Vec<TypeDesc> {
    vec![
        TypeDesc::Num(NumTy::I64),
        TypeDesc::Str,
        TypeDesc::Struct {
            name: "Node".into(),
            fields: vec![
                ("val".into(), DescId(D_INT)),
                ("name".into(), DescId(D_STR)),
                ("next".into(), DescId(D_NODE)),
            ],
        },
        TypeDesc::Enum {
            name: "Shape".into(),
            variants: vec![("Dot".into(), vec![]), ("Pair".into(), vec![DescId(D_INT), DescId(D_STR)])],
        },
    ]
}

fn heap_with(cfg: GcConfig) -> Heap {
    Heap::new(&descs(), cfg)
}

fn heap() -> Heap {
    heap_with(GcConfig { reuse_slots: false, ..GcConfig::default() })
}

fn roots_of(hs: &[Handle]) -> Roots {
    let mut r = Roots::new();
    hs.iter().for_each(|&h| r.handle(h));
    r
}

#[test]
fn unreachable_objects_are_freed_and_reachable_ones_survive() {
    let mut h = heap();
    let keep = h.alloc_str("keep".into()).unwrap();
    let drop = h.alloc_str("drop".into()).unwrap();
    assert_eq!(h.live_objects(), 2);
    let before = h.bytes();
    h.collect(&roots_of(&[keep]));
    assert!(h.is_live(keep) && !h.is_live(drop));
    assert_eq!(h.str_of(keep), "keep");
    assert!(h.bytes() < before);
    assert_eq!(h.stats().freed_objects, 1);
    h.check_invariants().unwrap();
    h.collect(&Roots::new());
    assert_eq!(h.live_objects(), 0);
    assert_eq!(h.bytes(), 0);
}

#[test]
fn null_handles_are_ignored_as_roots() {
    let mut h = heap();
    let s = h.alloc_str("x".into()).unwrap();
    let mut r = Roots::new();
    r.handle(0);
    assert!(r.is_empty());
    h.collect(&r);
    assert!(!h.is_live(s));
}

#[test]
fn handles_are_one_based() {
    let mut h = heap();
    assert_eq!(h.alloc_str("a".into()).unwrap(), 1);
    assert_eq!(h.alloc_str("b".into()).unwrap(), 2);
    assert!(!h.is_live(0));
}

#[test]
fn freed_slots_are_reused_when_enabled() {
    let mut h = heap_with(GcConfig::default());
    let a = h.alloc_str("a".into()).unwrap();
    h.collect(&Roots::new());
    let b = h.alloc_str("b".into()).unwrap();
    assert_eq!(a, b);
    assert_eq!(h.str_of(b), "b");
}

#[test]
#[should_panic(expected = "freed heap object")]
fn stale_handles_are_detected_when_slots_are_not_reused() {
    let mut h = heap();
    let a = h.alloc_str("a".into()).unwrap();
    h.collect(&Roots::new());
    let _b = h.alloc_str("b".into()).unwrap();
    let _ = h.str_of(a);
}

#[test]
#[should_panic(expected = "missed")]
fn tracing_into_a_freed_object_is_reported() {
    let mut h = heap();
    let s = h.alloc_str("inner".into()).unwrap();
    let arr = h.alloc_array(vec![s], true).unwrap();
    // collect with only the string rooted: the array dies, then root the dead array
    h.collect(&roots_of(&[s]));
    h.collect(&roots_of(&[arr]));
}

#[test]
fn struct_fields_are_traced_by_descriptor_layout() {
    let mut h = heap();
    let name = h.alloc_str("n".into()).unwrap();
    let tail = h.alloc_struct(D_NODE, vec![7, name, 0]).unwrap();
    // `val` holds a number that happens to look like a handle: it must not be traced
    let other = h.alloc_str("decoy".into()).unwrap();
    let head = h.alloc_struct(D_NODE, vec![other, 0, tail]).unwrap();
    h.collect(&roots_of(&[head]));
    assert!(h.is_live(head) && h.is_live(tail) && h.is_live(name));
    assert!(!h.is_live(other), "an integer field is not a reference");
    assert_eq!(h.struct_fields(tail)[0], 7);
}

#[test]
fn enum_payloads_depend_on_the_variant() {
    let mut h = heap();
    let s = h.alloc_str("payload".into()).unwrap();
    let pair = h.alloc_enum(D_SHAPE, 1, vec![5, s]).unwrap();
    let dot = h.alloc_enum(D_SHAPE, 0, vec![]).unwrap();
    h.collect(&roots_of(&[pair, dot]));
    assert!(h.is_live(s));
    assert_eq!(h.enum_parts(pair), (1, &[5, s][..]));
    h.collect(&roots_of(&[dot]));
    assert!(!h.is_live(pair) && !h.is_live(s) && h.is_live(dot));
}

#[test]
fn arrays_of_numbers_are_not_traced_but_arrays_of_references_are() {
    let mut h = heap();
    let s = h.alloc_str("s".into()).unwrap();
    let nums = h.alloc_array(vec![s, s], false).unwrap();
    h.collect(&roots_of(&[nums]));
    assert!(!h.is_live(s), "number arrays hold no references even if the bits look like one");
    let s2 = h.alloc_str("t".into()).unwrap();
    let strs = h.alloc_array(vec![s2], true).unwrap();
    let nested = h.alloc_array(vec![strs, strs], true).unwrap();
    h.collect(&roots_of(&[nested]));
    assert!(h.is_live(s2) && h.is_live(strs));
}

#[test]
fn cycles_are_collected() {
    let mut h = heap();
    let a = h.alloc_array(vec![], true).unwrap();
    let b = h.alloc_array(vec![a], true).unwrap();
    h.with_array(a, |v| v.push(b)).unwrap();
    h.with_array(a, |v| v.push(a)).unwrap();
    h.collect(&roots_of(&[a]));
    assert!(h.is_live(a) && h.is_live(b));
    h.collect(&Roots::new());
    assert_eq!(h.live_objects(), 0);
    h.check_invariants().unwrap();
}

#[test]
fn self_referential_structs_survive_when_rooted() {
    let mut h = heap();
    let n = h.alloc_struct(D_NODE, vec![1, 0, 0]).unwrap();
    h.set_field(n, 2, n);
    h.collect(&roots_of(&[n]));
    assert!(h.is_live(n));
    assert_eq!(h.struct_fields(n)[2], n);
}

#[test]
fn closures_keep_their_cells_and_closed_values_alive() {
    let mut h = heap();
    let s = h.alloc_str("captured".into()).unwrap();
    let closed = h.alloc_cell(CellState::Closed(s), true).unwrap();
    let int_cell = h.alloc_cell(CellState::Closed(s), false).unwrap();
    let open = h.alloc_cell(CellState::Open(3), true).unwrap();
    let clo = h.alloc_closure(4, vec![closed, int_cell, open]).unwrap();
    let orphan = h.alloc_cell(CellState::Closed(0), false).unwrap();
    let mut r = Roots::new();
    r.handle(clo);
    h.collect(&r);
    assert!(h.is_live(clo) && h.is_live(s));
    assert_eq!(h.live_cells(), 3, "the orphan cell is freed");
    assert_eq!(h.cell_state(closed), CellState::Closed(s));
    assert_eq!(h.cell_state(open), CellState::Open(3));
    let _ = orphan;
    h.check_invariants().unwrap();
    // a non-reference cell does not keep a lookalike alive
    let mut h = heap();
    let s = h.alloc_str("x".into()).unwrap();
    let c = h.alloc_cell(CellState::Closed(s), false).unwrap();
    let clo = h.alloc_closure(0, vec![c]).unwrap();
    h.collect(&roots_of(&[clo]));
    assert!(!h.is_live(s));
}

#[test]
fn open_cells_named_as_roots_survive_without_a_closure() {
    let mut h = heap();
    let c = h.alloc_cell(CellState::Open(1), false).unwrap();
    let mut r = Roots::new();
    r.cell(c);
    h.collect(&r);
    assert_eq!(h.live_cells(), 1);
    h.set_cell_state(c, CellState::Closed(9));
    assert_eq!(h.cell_state(c), CellState::Closed(9));
    h.collect(&Roots::new());
    assert_eq!(h.live_cells(), 0);
    assert_eq!(h.bytes(), 0);
}

#[test]
fn map_preserves_insertion_order_and_overwrites() {
    let mut h = heap();
    let m = h.alloc_map(false, false, false).unwrap();
    assert!(h.map_insert(m, 5, 50).unwrap());
    assert!(h.map_insert(m, 3, 30).unwrap());
    assert!(!h.map_insert(m, 5, 55).unwrap());
    assert_eq!(h.map_entries(m), [(5, 55), (3, 30)]);
    assert_eq!(h.map_find(m, 3), Some(1));
    assert_eq!(h.map_find(m, 4), None);
    assert_eq!(h.map_remove(m, 5).unwrap(), Some((5, 55)));
    assert_eq!(h.map_find(m, 5), None);
    assert_eq!(h.map_val_at(m, h.map_find(m, 3).unwrap()), 30);
    assert_eq!(h.map_len(m), 1);
    assert_eq!(h.map_remove(m, 5).unwrap(), None);
    h.map_insert(m, 9, 90).unwrap();
    assert_eq!(h.map_entries(m), [(3, 30), (9, 90)]);
    h.map_clear(m).unwrap();
    assert!(h.map_entries(m).is_empty());
    h.check_invariants().unwrap();
}

#[test]
fn maps_agree_with_a_reference_model_under_churn() {
    use std::collections::BTreeMap;
    let mut h = heap();
    let m = h.alloc_map(false, false, false).unwrap();
    // an insertion-ordered reference: Vec of (key, value) with linear search
    let mut model: Vec<(u64, u64)> = Vec::new();
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    let mut next = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for step in 0..20_000u64 {
        let key = next() % 300 + 1;
        match next() % 5 {
            0 | 1 => {
                let fresh = h.map_insert(m, key, step).unwrap();
                match model.iter_mut().find(|e| e.0 == key) {
                    Some(e) => {
                        assert!(!fresh);
                        e.1 = step;
                    }
                    None => {
                        assert!(fresh);
                        model.push((key, step));
                    }
                }
            }
            2 | 3 => {
                let got = h.map_remove(m, key).unwrap();
                let want = model.iter().position(|e| e.0 == key).map(|i| model.remove(i));
                assert_eq!(got, want, "step {step}");
            }
            _ => {
                let want = model.iter().find(|e| e.0 == key).map(|e| e.1);
                let got = h.map_find(m, key).map(|i| h.map_val_at(m, i));
                assert_eq!(got, want, "step {step}");
            }
        }
        if step % 997 == 0 {
            assert_eq!(h.map_entries(m), model);
            assert_eq!(h.map_len(m), model.len());
            h.check_invariants().unwrap();
        }
        if step == 15_000 {
            // everything out, then reuse
            for (k, _) in std::mem::take(&mut model) {
                assert!(h.map_remove(m, k).unwrap().is_some());
            }
            assert_eq!(h.map_len(m), 0);
        }
    }
    assert_eq!(h.map_entries(m), model);
    let _ = BTreeMap::<u8, u8>::new();
}

#[test]
fn string_keys_compare_by_content() {
    let mut h = heap();
    let m = h.alloc_map(true, true, false).unwrap();
    let k1 = h.alloc_str("key".into()).unwrap();
    let k2 = h.alloc_str("key".into()).unwrap();
    let k3 = h.alloc_str("other".into()).unwrap();
    assert!(h.map_insert(m, k1, 1).unwrap());
    assert!(!h.map_insert(m, k2, 2).unwrap(), "same text, same key");
    assert!(h.map_insert(m, k3, 3).unwrap());
    assert_eq!(h.map_find(m, k2), Some(0));
    assert_eq!(h.map_entries(m), [(k1, 2), (k3, 3)]);
    // string keys are references and survive through the map
    h.collect(&roots_of(&[m]));
    assert!(h.is_live(k1) && h.is_live(k3) && !h.is_live(k2));
}

#[test]
fn map_values_and_set_elements_are_traced_per_flags() {
    let mut h = heap();
    let m = h.alloc_map(false, false, true).unwrap();
    let v = h.alloc_str("v".into()).unwrap();
    h.map_insert(m, 1, v).unwrap();
    let set = h.alloc_set(false, true).unwrap();
    let e = h.alloc_array(vec![], false).unwrap();
    h.map_insert(set, e, 0).unwrap();
    h.collect(&roots_of(&[m, set]));
    assert!(h.is_live(v) && h.is_live(e));
}

#[test]
fn update_keeps_the_accounting_exact() {
    let mut h = heap();
    let a = h.alloc_array(vec![], false).unwrap();
    let before = h.bytes();
    h.update(a, |o| {
        if let Obj::Array { elems, .. } = o {
            elems.extend(0..1000);
        }
    })
    .unwrap();
    assert!(h.bytes() >= before + 8000);
    h.check_invariants().unwrap();
    h.update(a, |o| {
        if let Obj::Array { elems, .. } = o {
            elems.clear();
            elems.shrink_to_fit();
        }
    })
    .unwrap();
    assert!(h.bytes() <= before + 16);
    h.check_invariants().unwrap();
}

#[test]
fn the_limit_is_enforced() {
    let mut h = heap_with(GcConfig { limit: Some(2000), ..GcConfig::default() });
    let mut keep = Vec::new();
    let err = loop {
        match h.alloc_array(vec![0; 16], false) {
            Ok(a) => keep.push(a),
            Err(e) => break e,
        }
    };
    assert_eq!(err, Trap::OutOfMemory);
    assert!(h.bytes() <= 2000);
    // dropping everything makes room again
    h.collect(&Roots::new());
    assert!(h.alloc_array(vec![0; 16], false).is_ok());
}

#[test]
fn growth_past_the_limit_traps_but_stays_consistent() {
    let mut h = heap_with(GcConfig { limit: Some(4096), ..GcConfig::default() });
    let a = h.alloc_array(vec![], false).unwrap();
    let r = h.update(a, |o| {
        if let Obj::Array { elems, .. } = o {
            elems.extend(0..10_000);
        }
    });
    assert_eq!(r, Err(Trap::OutOfMemory));
    h.check_invariants().unwrap();
}

#[test]
fn thresholds_adapt_to_the_live_size() {
    let mut h = heap_with(GcConfig { initial_threshold: 1000, ..GcConfig::default() });
    assert!(!h.should_collect());
    let big = h.alloc_array(vec![0; 400], false).unwrap();
    assert!(h.should_collect());
    h.collect(&roots_of(&[big]));
    assert!(!h.should_collect(), "threshold moved above the live size");
    let stress = heap_with(GcConfig { stress: true, ..GcConfig::default() });
    assert!(stress.should_collect());
    // with a hard limit the next trigger is halfway to it
    let mut lim = heap_with(GcConfig { limit: Some(10_000), initial_threshold: 100, ..GcConfig::default() });
    let a = lim.alloc_array(vec![0; 100], false).unwrap();
    lim.collect(&roots_of(&[a]));
    assert!(!lim.should_collect());
    let live = lim.bytes();
    assert!(live > 800);
}

#[test]
fn heap_view_formats_nested_values_like_the_interpreter() {
    use crate::runtime::value::format_value;
    let ds = descs();
    let mut h = heap();
    let name = h.alloc_str("a\"b".into()).unwrap();
    let pair = h.alloc_enum(D_SHAPE, 1, vec![4, name]).unwrap();
    let mut out = String::new();
    format_value(&h, &ds, DescId(D_SHAPE), pair, false, &mut out);
    assert_eq!(out, "Shape::Pair(4, \"a\\\"b\")");
}

// ---- stack scanning ---------------------------------------------------------------------------------------------

fn program_with(chunks: Vec<Chunk>, globals: Vec<GlobalInfo>) -> BytecodeProgram {
    BytecodeProgram { chunks, globals, descs: descs(), ..Default::default() }
}

fn chunk_with_maps(name: &str, maps: &[(u32, &[u8])]) -> Chunk {
    Chunk {
        name: name.into(),
        code: vec![0; 8],
        params: 0,
        frame_size: 8,
        param_refs: vec![],
        upvals: vec![],
        stack_maps: maps
            .iter()
            .map(|(pc, rs)| StackMap { pc: *pc, live_roots: rs.iter().copied().collect() })
            .collect(),
    }
}

#[test]
fn stack_scan_uses_each_frames_map_and_base() {
    let prog =
        program_with(vec![chunk_with_maps("outer", &[(2, &[0, 3])]), chunk_with_maps("inner", &[(5, &[1])])], vec![]);
    let mut h = heap();
    let objs: Vec<Handle> = (0..6).map(|i| h.alloc_str(format!("o{i}")).unwrap()).collect();
    // outer frame at base 0 (r0 and r3 are roots); inner frame at base 4 overlapping nothing
    let mut stack = vec![0u64; 16];
    stack[0] = objs[0];
    stack[1] = objs[1]; // live number lookalike, not in the map
    stack[3] = objs[2];
    stack[4] = objs[3]; // inner r0 (not in its map)
    stack[5] = objs[4]; // inner r1 (a root)
    let frames =
        [FrameInfo { chunk: 0, pc: 2, base: 0, closure: 0 }, FrameInfo { chunk: 1, pc: 5, base: 4, closure: objs[5] }];
    let mut roots = Roots::new();
    scan_stack(&prog, &frames, &stack, &mut roots);
    h.collect(&roots);
    let live: HashSet<usize> = (0..6).filter(|&i| h.is_live(objs[i])).collect();
    assert_eq!(live, [0, 2, 4, 5].into_iter().collect::<HashSet<_>>());
}

#[test]
#[should_panic(expected = "not a safe-point")]
fn scanning_a_frame_that_is_not_at_a_safe_point_panics() {
    let prog = program_with(vec![chunk_with_maps("f", &[(2, &[])])], vec![]);
    let mut roots = Roots::new();
    scan_stack(&prog, &[FrameInfo { chunk: 0, pc: 3, base: 0, closure: 0 }], &[0; 8], &mut roots);
}

#[test]
fn globals_of_reference_type_are_roots() {
    let g = |ty| GlobalInfo { name: "g".into(), ty, desc: DescId(0) };
    let prog = program_with(vec![], vec![g(IrTy::Ref), g(IrTy::Num(NumTy::I64)), g(IrTy::Ref)]);
    let mut h = heap();
    let (a, b, c) =
        (h.alloc_str("a".into()).unwrap(), h.alloc_str("b".into()).unwrap(), h.alloc_str("c".into()).unwrap());
    let mut roots = Roots::new();
    scan_globals(&prog, &[a, b, c], &mut roots);
    h.collect(&roots);
    assert!(h.is_live(a) && !h.is_live(b) && h.is_live(c));
}

// ---- randomized graph test ----------------------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn random_graphs_keep_exactly_the_reachable_objects() {
    let mut rng = Rng(0xDEADBEEFCAFEF00D);
    for round in 0..60 {
        let mut h = heap();
        let n = 5 + rng.below(80);
        // build n nodes of mixed kinds, then wire random edges between them
        let mut objs: Vec<Handle> = Vec::new();
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for _ in 0..n {
            let kind = rng.below(3);
            let o = match kind {
                0 => h.alloc_array(vec![], true).unwrap(),
                1 => h.alloc_struct(D_NODE, vec![rng.next() % 1000, 0, 0]).unwrap(),
                _ => h.alloc_map(false, false, true).unwrap(),
            };
            objs.push(o);
        }
        for i in 0..n {
            let fanout = rng.below(4);
            for _ in 0..fanout {
                let t = rng.below(n);
                match h.obj(objs[i]) {
                    Obj::Array { .. } => {
                        let target = objs[t];
                        h.with_array(objs[i], |v| v.push(target)).unwrap();
                        edges[i].push(t);
                    }
                    Obj::Struct { .. } => {
                        // `next` is the only reference field; later edges overwrite it
                        h.set_field(objs[i], 2, objs[t]);
                        edges[i] = vec![t];
                    }
                    Obj::Map(_) => {
                        let key = (edges[i].len() as u64) + 1;
                        h.map_insert(objs[i], key, objs[t]).unwrap();
                        edges[i].push(t);
                    }
                    _ => unreachable!(),
                }
            }
        }
        let roots_idx: Vec<usize> = (0..rng.below(4)).map(|_| rng.below(n)).collect();
        // oracle: plain graph search
        let mut reach = HashSet::default();
        let mut stack = roots_idx.clone();
        while let Some(i) = stack.pop() {
            if reach.insert(i) {
                stack.extend(&edges[i]);
            }
        }
        let rs: Vec<Handle> = roots_idx.iter().map(|&i| objs[i]).collect();
        h.collect(&roots_of(&rs));
        for (i, &o) in objs.iter().enumerate() {
            assert_eq!(h.is_live(o), reach.contains(&i), "round {round}, object {i}");
        }
        h.check_invariants().unwrap();
        // a second collection changes nothing; dropping the roots frees everything
        let before = h.live_objects();
        h.collect(&roots_of(&rs));
        assert_eq!(h.live_objects(), before);
        h.collect(&Roots::new());
        assert_eq!(h.live_objects(), 0);
        assert_eq!(h.bytes(), 0);
    }
}
