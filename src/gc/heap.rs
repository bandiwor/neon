//! The garbage-collected heap: objects addressed by 1-based handles
//! (0 is the null handle), a separate arena for closure upvalue cells, and an
//! exact mark-and-sweep collector driven by [`Roots`].
//!
//! Values in registers are untagged 64-bit words, so the collector needs to be
//! told which words are references. Objects carry that knowledge themselves
//! (arrays and maps remember whether their elements are references; structs
//! and enums consult the layout derived from their type descriptor), and the
//! VM supplies the live register roots from the compiler's stack maps.

use std::mem::size_of;
use std::ops::{Deref, DerefMut};

use crate::fx::{hash_bytes, mix64};
use crate::ir::TypeDesc;
use crate::runtime::{HeapView, Trap};

/// A reference to a heap object; 0 is null.
pub type Handle = u64;

/// Identifies an upvalue cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CellId(pub u32);

/// How map and set keys are hashed and compared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyKind {
    /// By the 64 bits of the value itself.
    Bits,
    /// By the contents of the string the handle refers to.
    Str,
}

/// One entry of a map or set. `hash == 0` marks a removed entry: real hashes always have the top
/// bit set (see [`Heap::hash_key`]).
#[derive(Debug, Clone, Copy)]
struct Entry {
    hash: u64,
    key: u64,
    val: u64,
}

const EMPTY: u32 = u32::MAX;
const TOP_BIT: u64 = 1 << 63;

/// Insertion-ordered hash table shared by maps and sets (sets ignore values).
///
/// `entries` keeps insertion order; `table` is an open-addressing index (linear probing, load at
/// most one half) of positions in `entries`. Removing an entry only marks it dead, so removal is
/// O(1); dead entries are squeezed out when the table is rebuilt.
#[derive(Debug)]
pub struct MapObj {
    entries: Vec<Entry>,
    table: Vec<u32>,
    live: usize,
    pub key_kind: KeyKind,
    pub key_ref: bool,
    pub val_ref: bool,
}

/// Words stored inline in an object up to this many; larger objects use a `Vec`.
const INLINE_FIELDS: usize = 4;

/// The fields of a struct or the payload of an enum. Small ones (the common case) live in the
/// object slot itself, so allocating them costs no `malloc`.
#[derive(Debug, Clone)]
pub enum Fields {
    Inline { len: u8, words: [u64; INLINE_FIELDS] },
    Heap(Vec<u64>),
}

impl Fields {
    pub fn new(src: &[u64]) -> Fields {
        if src.len() <= INLINE_FIELDS {
            let mut words = [0; INLINE_FIELDS];
            words[..src.len()].copy_from_slice(src);
            Fields::Inline { len: src.len() as u8, words }
        } else {
            Fields::Heap(src.to_vec())
        }
    }

    /// Bytes outside the object slot.
    fn extra_bytes(&self) -> usize {
        match self {
            Fields::Inline { .. } => 0,
            Fields::Heap(v) => v.capacity() * 8,
        }
    }
}

impl Deref for Fields {
    type Target = [u64];
    #[inline]
    fn deref(&self) -> &[u64] {
        match self {
            Fields::Inline { len, words } => &words[..*len as usize],
            Fields::Heap(v) => v,
        }
    }
}

impl DerefMut for Fields {
    #[inline]
    fn deref_mut(&mut self) -> &mut [u64] {
        match self {
            Fields::Inline { len, words } => &mut words[..*len as usize],
            Fields::Heap(v) => v,
        }
    }
}

#[derive(Debug)]
pub enum Obj {
    Str(String),
    Array { elems: Vec<u64>, refs: bool },
    Map(Box<MapObj>),
    Set(Box<MapObj>),
    Struct { desc: u32, fields: Fields },
    Enum { desc: u32, tag: u32, fields: Fields },
    Closure { func: u32, cells: Vec<CellId> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellState {
    /// The variable still lives in this absolute stack slot.
    Open(usize),
    Closed(u64),
}

#[derive(Debug)]
struct Cell {
    state: CellState,
    is_ref: bool,
}

#[derive(Debug)]
enum Slot {
    Free,
    Live { obj: Obj, size: usize },
}

#[derive(Debug)]
enum CellSlot {
    Free,
    Live(Cell),
}

/// Reference layout of a struct or enum type.
#[derive(Debug, Clone)]
enum Layout {
    None,
    Struct(Vec<bool>),
    Enum(Vec<Vec<bool>>),
}

/// Collector settings.
#[derive(Debug, Clone, Copy)]
pub struct GcConfig {
    /// Hard cap on live heap bytes; allocating beyond it traps with `OutOfMemory`.
    pub limit: Option<usize>,
    /// Bytes allocated before the first collection.
    pub initial_threshold: usize,
    /// Collect at every safe-point (for testing stack maps).
    pub stress: bool,
    /// Reuse freed slots. Tests turn this off so that a stale handle always
    /// lands on a freed slot and is detected instead of aliasing a new object.
    pub reuse_slots: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        GcConfig { limit: None, initial_threshold: 1 << 20, stress: false, reuse_slots: true }
    }
}

/// Counters, mostly for tests and `--stats`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HeapStats {
    pub collections: u64,
    pub allocations: u64,
    pub freed_objects: u64,
    pub freed_cells: u64,
    pub peak_bytes: usize,
}

/// What the collector must treat as live, gathered by the VM.
#[derive(Debug, Default)]
pub struct Roots {
    handles: Vec<Handle>,
    cells: Vec<CellId>,
}

impl Roots {
    pub fn new() -> Roots {
        Roots::default()
    }

    /// Registers a reference held somewhere outside the heap (0 is ignored).
    pub fn handle(&mut self, h: Handle) {
        if h != 0 {
            self.handles.push(h);
        }
    }

    pub fn cell(&mut self, c: CellId) {
        self.cells.push(c);
    }

    pub fn len(&self) -> usize {
        self.handles.len() + self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug)]
pub struct Heap {
    slots: Vec<Slot>,
    free: Vec<u32>,
    cells: Vec<CellSlot>,
    free_cells: Vec<u32>,
    layouts: Vec<Layout>,
    cfg: GcConfig,
    bytes: usize,
    threshold: usize,
    stats: HeapStats,
}

const CELL_BYTES: usize = size_of::<Cell>() + 8;

impl Heap {
    pub fn new(descs: &[TypeDesc], cfg: GcConfig) -> Heap {
        let layouts = descs
            .iter()
            .map(|d| match d {
                TypeDesc::Struct { fields, .. } => {
                    Layout::Struct(fields.iter().map(|(_, f)| descs[f.index()].is_ref()).collect())
                }
                TypeDesc::Enum { variants, .. } => Layout::Enum(
                    variants.iter().map(|(_, fs)| fs.iter().map(|f| descs[f.index()].is_ref()).collect()).collect(),
                ),
                _ => Layout::None,
            })
            .collect();
        Heap {
            slots: Vec::new(),
            free: Vec::new(),
            cells: Vec::new(),
            free_cells: Vec::new(),
            layouts,
            cfg,
            bytes: 0,
            // with a hard limit, start collecting well before reaching it
            threshold: cfg.limit.map_or(cfg.initial_threshold, |l| cfg.initial_threshold.min(l / 2)),
            stats: HeapStats::default(),
        }
    }

    pub fn config(&self) -> &GcConfig {
        &self.cfg
    }

    pub fn stats(&self) -> &HeapStats {
        &self.stats
    }

    /// Bytes currently accounted to live and not-yet-swept objects.
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    pub fn live_objects(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s, Slot::Live { .. })).count()
    }

    pub fn live_cells(&self) -> usize {
        self.cells.iter().filter(|s| matches!(s, CellSlot::Live(_))).count()
    }

    // ---- allocation ------------------------------------------------------------------------------

    fn charge(&mut self, size: usize) -> Result<(), Trap> {
        if let Some(limit) = self.cfg.limit {
            if self.bytes + size > limit {
                return Err(Trap::OutOfMemory);
            }
        }
        self.bytes += size;
        self.stats.peak_bytes = self.stats.peak_bytes.max(self.bytes);
        Ok(())
    }

    pub fn alloc(&mut self, obj: Obj) -> Result<Handle, Trap> {
        let size = obj_size(&obj);
        self.charge(size)?;
        self.stats.allocations += 1;
        let slot = Slot::Live { obj, size };
        let idx = match self.free.pop() {
            Some(i) => {
                self.slots[i as usize] = slot;
                i as usize
            }
            None => {
                self.slots.push(slot);
                self.slots.len() - 1
            }
        };
        Ok(idx as u64 + 1)
    }

    pub fn alloc_str(&mut self, s: String) -> Result<Handle, Trap> {
        self.alloc(Obj::Str(s))
    }

    pub fn alloc_array(&mut self, elems: Vec<u64>, refs: bool) -> Result<Handle, Trap> {
        self.alloc(Obj::Array { elems, refs })
    }

    pub fn alloc_struct(&mut self, desc: u32, fields: impl AsRef<[u64]>) -> Result<Handle, Trap> {
        self.alloc(Obj::Struct { desc, fields: Fields::new(fields.as_ref()) })
    }

    pub fn alloc_enum(&mut self, desc: u32, tag: u32, fields: impl AsRef<[u64]>) -> Result<Handle, Trap> {
        self.alloc(Obj::Enum { desc, tag, fields: Fields::new(fields.as_ref()) })
    }

    pub fn alloc_closure(&mut self, func: u32, cells: Vec<CellId>) -> Result<Handle, Trap> {
        self.alloc(Obj::Closure { func, cells })
    }

    /// An empty map; `key_str` selects content-based hashing of string keys.
    pub fn alloc_map(&mut self, key_str: bool, key_ref: bool, val_ref: bool) -> Result<Handle, Trap> {
        self.alloc(Obj::Map(Box::new(MapObj::new(key_str, key_ref, val_ref))))
    }

    pub fn alloc_set(&mut self, key_str: bool, key_ref: bool) -> Result<Handle, Trap> {
        self.alloc(Obj::Set(Box::new(MapObj::new(key_str, key_ref, false))))
    }

    /// Allocates an upvalue cell over stack slot `slot` (open) .
    pub fn alloc_cell(&mut self, state: CellState, is_ref: bool) -> Result<CellId, Trap> {
        self.charge(CELL_BYTES)?;
        let cell = CellSlot::Live(Cell { state, is_ref });
        let id = match self.free_cells.pop() {
            Some(i) => {
                self.cells[i as usize] = cell;
                i
            }
            None => {
                self.cells.push(cell);
                self.cells.len() as u32 - 1
            }
        };
        Ok(CellId(id))
    }

    // ---- access --------------------------------------------------------------------------------------

    fn idx(&self, h: Handle) -> usize {
        assert!(h != 0 && (h as usize) <= self.slots.len(), "invalid heap handle {h}");
        h as usize - 1
    }

    pub fn obj(&self, h: Handle) -> &Obj {
        match &self.slots[self.idx(h)] {
            Slot::Live { obj, .. } => obj,
            Slot::Free => panic!("use of freed heap object {h} (a root was missed)"),
        }
    }

    /// Mutates an object and re-accounts its size afterwards.
    pub fn update<R>(&mut self, h: Handle, f: impl FnOnce(&mut Obj) -> R) -> Result<R, Trap> {
        let i = self.idx(h);
        let Slot::Live { obj, size } = &mut self.slots[i] else {
            panic!("use of freed heap object {h} (a root was missed)")
        };
        let r = f(obj);
        let new = obj_size(obj);
        let old = std::mem::replace(size, new);
        if new >= old {
            let grow = new - old;
            if let Some(limit) = self.cfg.limit {
                if self.bytes + grow > limit {
                    // keep the accounting honest even though the operation already happened
                    self.bytes += grow;
                    return Err(Trap::OutOfMemory);
                }
            }
            self.bytes += grow;
            self.stats.peak_bytes = self.stats.peak_bytes.max(self.bytes);
        } else {
            self.bytes -= old - new;
        }
        Ok(r)
    }

    pub fn is_live(&self, h: Handle) -> bool {
        h != 0 && (h as usize) <= self.slots.len() && matches!(self.slots[h as usize - 1], Slot::Live { .. })
    }

    pub fn cell_state(&self, c: CellId) -> CellState {
        self.cell(c).state
    }

    pub fn set_cell_state(&mut self, c: CellId, state: CellState) {
        match &mut self.cells[c.0 as usize] {
            CellSlot::Live(cell) => cell.state = state,
            CellSlot::Free => panic!("use of freed upvalue cell {}", c.0),
        }
    }

    fn cell(&self, c: CellId) -> &Cell {
        match &self.cells[c.0 as usize] {
            CellSlot::Live(cell) => cell,
            CellSlot::Free => panic!("use of freed upvalue cell {} (a root was missed)", c.0),
        }
    }

    pub fn closure_parts(&self, h: Handle) -> (u32, &[CellId]) {
        match self.obj(h) {
            Obj::Closure { func, cells } => (*func, cells),
            other => panic!("expected a closure, found {other:?}"),
        }
    }

    pub fn set_field(&mut self, h: Handle, idx: usize, v: u64) {
        match &mut self.slot_mut(h).0 {
            Obj::Struct { fields, .. } => fields[idx] = v,
            other => panic!("expected a struct, found {other:?}"),
        }
    }

    fn slot_mut(&mut self, h: Handle) -> (&mut Obj, &mut usize) {
        let i = self.idx(h);
        match &mut self.slots[i] {
            Slot::Live { obj, size } => (obj, size),
            Slot::Free => panic!("use of freed heap object {h} (a root was missed)"),
        }
    }

    /// Runs `f` on the elements of an array and re-accounts its size.
    pub fn with_array<R>(&mut self, h: Handle, f: impl FnOnce(&mut Vec<u64>) -> R) -> Result<R, Trap> {
        self.update(h, |o| match o {
            Obj::Array { elems, .. } => f(elems),
            other => panic!("expected an array, found {other:?}"),
        })
    }

    pub fn array_refs(&self, h: Handle) -> bool {
        match self.obj(h) {
            Obj::Array { refs, .. } => *refs,
            other => panic!("expected an array, found {other:?}"),
        }
    }

    // ---- maps and sets ------------------------------------------------------------------------------------

    fn map_obj(&self, h: Handle) -> &MapObj {
        match self.obj(h) {
            Obj::Map(m) | Obj::Set(m) => m,
            other => panic!("expected a map or set, found {other:?}"),
        }
    }

    fn hash_key(&self, kind: KeyKind, key: u64) -> u64 {
        let h = match kind {
            KeyKind::Bits => mix64(key),
            KeyKind::Str => hash_bytes(self.str_of(key).as_bytes()),
        };
        h | TOP_BIT
    }

    fn keys_equal(&self, kind: KeyKind, a: u64, b: u64) -> bool {
        match kind {
            KeyKind::Bits => a == b,
            KeyKind::Str => a == b || self.str_of(a) == self.str_of(b),
        }
    }

    fn find_hashed(&self, m: &MapObj, key: u64, hash: u64) -> Option<usize> {
        if m.table.is_empty() {
            return None;
        }
        let mask = m.table.len() - 1;
        let mut i = hash as usize & mask;
        loop {
            let slot = m.table[i];
            if slot == EMPTY {
                return None;
            }
            let e = &m.entries[slot as usize];
            if e.hash == hash && self.keys_equal(m.key_kind, e.key, key) {
                return Some(slot as usize);
            }
            i = (i + 1) & mask;
        }
    }

    /// Position of `key` in the entry list of a map or set (valid until the next change).
    pub fn map_find(&self, h: Handle, key: u64) -> Option<usize> {
        let m = self.map_obj(h);
        if m.live == 0 {
            return None;
        }
        self.find_hashed(m, key, self.hash_key(m.key_kind, key))
    }

    /// Whether the keys / values of a map or set are references.
    pub fn map_flags(&self, h: Handle) -> (bool, bool) {
        let m = self.map_obj(h);
        (m.key_ref, m.val_ref)
    }

    /// Number of entries.
    pub fn map_len(&self, h: Handle) -> usize {
        self.map_obj(h).live
    }

    /// The value stored at a position returned by [`Heap::map_find`].
    pub fn map_val_at(&self, h: Handle, pos: usize) -> u64 {
        self.map_obj(h).entries[pos].val
    }

    /// The live `(key, value)` pairs in insertion order.
    pub fn map_live_entries(&self, h: Handle) -> impl Iterator<Item = (u64, u64)> + '_ {
        self.map_obj(h).entries.iter().filter(|e| e.hash != 0).map(|e| (e.key, e.val))
    }

    /// Inserts or overwrites; returns whether the key was new.
    pub fn map_insert(&mut self, h: Handle, key: u64, val: u64) -> Result<bool, Trap> {
        let (hash, found) = {
            let m = self.map_obj(h);
            let hash = self.hash_key(m.key_kind, key);
            (hash, if m.live == 0 { None } else { self.find_hashed(m, key, hash) })
        };
        match found {
            Some(i) => {
                self.update(h, |o| {
                    if let Obj::Map(m) | Obj::Set(m) = o {
                        m.entries[i].val = val
                    }
                })?;
                Ok(false)
            }
            None => {
                self.update(h, |o| {
                    if let Obj::Map(m) | Obj::Set(m) = o {
                        m.push_new(hash, key, val);
                    }
                })?;
                Ok(true)
            }
        }
    }

    /// Removes `key`, preserving the order of the remaining entries.
    pub fn map_remove(&mut self, h: Handle, key: u64) -> Result<Option<(u64, u64)>, Trap> {
        let Some(pos) = self.map_find(h, key) else { return Ok(None) };
        self.update(h, |o| {
            let (Obj::Map(m) | Obj::Set(m)) = o else { unreachable!() };
            Some(m.kill(pos))
        })
    }

    pub fn map_clear(&mut self, h: Handle) -> Result<(), Trap> {
        self.update(h, |o| {
            if let Obj::Map(m) | Obj::Set(m) = o {
                m.clear();
            }
        })
    }

    // ---- collection ------------------------------------------------------------------------------------------

    /// Whether the VM should collect before its next allocating instruction.
    pub fn should_collect(&self) -> bool {
        self.cfg.stress || self.bytes >= self.threshold
    }

    /// Marks everything reachable from `roots` and frees the rest.
    pub fn collect(&mut self, roots: &Roots) {
        let mut marked = vec![false; self.slots.len()];
        let mut cell_marked = vec![false; self.cells.len()];
        let mut gray: Vec<Handle> = roots.handles.clone();
        let mut cell_gray: Vec<CellId> = roots.cells.clone();
        loop {
            if let Some(c) = cell_gray.pop() {
                let ci = c.0 as usize;
                if cell_marked[ci] {
                    continue;
                }
                cell_marked[ci] = true;
                let cell = self.cell(c);
                if let (CellState::Closed(v), true) = (cell.state, cell.is_ref) {
                    if v != 0 {
                        gray.push(v);
                    }
                }
                continue;
            }
            let Some(h) = gray.pop() else { break };
            let i = self.idx(h);
            if marked[i] {
                continue;
            }
            marked[i] = true;
            let Slot::Live { obj, .. } = &self.slots[i] else {
                panic!("the collector reached freed object {h} (a root was missed)")
            };
            self.trace(obj, &mut gray, &mut cell_gray);
        }

        let mut freed = 0;
        for (i, m) in marked.iter().enumerate() {
            if let Slot::Live { size, .. } = &self.slots[i] {
                if !m {
                    self.bytes -= *size;
                    self.slots[i] = Slot::Free;
                    if self.cfg.reuse_slots {
                        self.free.push(i as u32);
                    }
                    freed += 1;
                }
            }
        }
        let mut freed_cells = 0;
        for (i, m) in cell_marked.iter().enumerate() {
            if matches!(self.cells[i], CellSlot::Live(_)) && !m {
                self.bytes -= CELL_BYTES;
                self.cells[i] = CellSlot::Free;
                if self.cfg.reuse_slots {
                    self.free_cells.push(i as u32);
                }
                freed_cells += 1;
            }
        }
        self.stats.collections += 1;
        self.stats.freed_objects += freed;
        self.stats.freed_cells += freed_cells;
        self.threshold = match self.cfg.limit {
            // trigger again once half of the remaining headroom is used
            Some(l) => self.bytes + l.saturating_sub(self.bytes) / 2,
            None => (self.bytes * 2).max(self.cfg.initial_threshold),
        };
    }

    fn trace(&self, obj: &Obj, gray: &mut Vec<Handle>, cells: &mut Vec<CellId>) {
        let mut push = |v: u64| {
            if v != 0 {
                gray.push(v)
            }
        };
        match obj {
            Obj::Str(_) => {}
            Obj::Array { elems, refs } => {
                if *refs {
                    elems.iter().for_each(|&v| push(v));
                }
            }
            Obj::Map(m) | Obj::Set(m) => {
                for e in &m.entries {
                    if m.key_ref {
                        push(e.key);
                    }
                    if m.val_ref {
                        push(e.val);
                    }
                }
            }
            Obj::Struct { desc, fields } => {
                if let Layout::Struct(refs) = &self.layouts[*desc as usize] {
                    for (&v, &r) in fields.iter().zip(refs) {
                        if r {
                            push(v);
                        }
                    }
                }
            }
            Obj::Enum { desc, tag, fields } => {
                if let Layout::Enum(variants) = &self.layouts[*desc as usize] {
                    for (&v, &r) in fields.iter().zip(&variants[*tag as usize]) {
                        if r {
                            push(v);
                        }
                    }
                }
            }
            Obj::Closure { cells: cs, .. } => cells.extend(cs.iter().copied()),
        }
    }

    /// Internal consistency check used by tests.
    pub fn check_invariants(&self) -> Result<(), String> {
        let mut total = 0;
        for s in &self.slots {
            if let Slot::Live { obj, size } = s {
                if *size != obj_size(obj) {
                    return Err(format!("stale size accounting for {obj:?}"));
                }
                total += size;
            }
        }
        total += self.live_cells() * CELL_BYTES;
        if total != self.bytes {
            return Err(format!("bytes {} != sum of live sizes {total}", self.bytes));
        }
        for &f in &self.free {
            if !matches!(self.slots[f as usize], Slot::Free) {
                return Err(format!("free list names live slot {f}"));
            }
        }
        Ok(())
    }
}

impl MapObj {
    fn new(key_str: bool, key_ref: bool, val_ref: bool) -> MapObj {
        MapObj {
            entries: Vec::new(),
            table: Vec::new(),
            live: 0,
            key_kind: if key_str { KeyKind::Str } else { KeyKind::Bits },
            key_ref,
            val_ref,
        }
    }

    fn place(&mut self, hash: u64, pos: u32) {
        let mask = self.table.len() - 1;
        let mut i = hash as usize & mask;
        while self.table[i] != EMPTY {
            i = (i + 1) & mask;
        }
        self.table[i] = pos;
    }

    /// Appends a new entry (the key is known to be absent).
    fn push_new(&mut self, hash: u64, key: u64, val: u64) {
        if (self.entries.len() + 1) * 2 > self.table.len() {
            self.rebuild();
        }
        let pos = self.entries.len() as u32;
        self.entries.push(Entry { hash, key, val });
        self.place(hash, pos);
        self.live += 1;
    }

    /// Squeezes out dead entries and re-creates the index with room for the next insertion.
    fn rebuild(&mut self) {
        if self.live < self.entries.len() {
            self.entries.retain(|e| e.hash != 0);
        }
        let size = ((self.entries.len() + 1) * 2).next_power_of_two().max(8);
        self.table.clear();
        self.table.resize(size, EMPTY);
        for pos in 0..self.entries.len() {
            let hash = self.entries[pos].hash;
            self.place(hash, pos as u32);
        }
    }

    /// Marks the entry at `pos` removed and returns its key and value.
    fn kill(&mut self, pos: usize) -> (u64, u64) {
        let e = &mut self.entries[pos];
        let removed = (e.key, e.val);
        *e = Entry { hash: 0, key: 0, val: 0 };
        self.live -= 1;
        if self.live == 0 {
            self.clear();
        }
        removed
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.live = 0;
        self.table.iter_mut().for_each(|s| *s = EMPTY);
    }
}

fn obj_size(o: &Obj) -> usize {
    let base = size_of::<Obj>() + 16;
    base + match o {
        Obj::Str(s) => s.capacity(),
        Obj::Array { elems, .. } => elems.capacity() * 8,
        Obj::Map(m) | Obj::Set(m) => {
            size_of::<MapObj>() + m.entries.capacity() * size_of::<Entry>() + m.table.capacity() * 4
        }
        Obj::Struct { fields, .. } | Obj::Enum { fields, .. } => fields.extra_bytes(),
        Obj::Closure { cells, .. } => cells.capacity() * 4,
    }
}

impl HeapView for Heap {
    fn str_of(&self, h: u64) -> &str {
        match self.obj(h) {
            Obj::Str(s) => s,
            other => panic!("expected a string, found {other:?}"),
        }
    }

    fn array_of(&self, h: u64) -> &[u64] {
        match self.obj(h) {
            Obj::Array { elems, .. } => elems,
            other => panic!("expected an array, found {other:?}"),
        }
    }

    fn map_entries(&self, h: u64) -> Vec<(u64, u64)> {
        match self.obj(h) {
            Obj::Map(m) => m.entries.iter().filter(|e| e.hash != 0).map(|e| (e.key, e.val)).collect(),
            other => panic!("expected a map, found {other:?}"),
        }
    }

    fn set_elems(&self, h: u64) -> Vec<u64> {
        match self.obj(h) {
            Obj::Set(m) => m.entries.iter().filter(|e| e.hash != 0).map(|e| e.key).collect(),
            other => panic!("expected a set, found {other:?}"),
        }
    }

    fn struct_fields(&self, h: u64) -> &[u64] {
        match self.obj(h) {
            Obj::Struct { fields, .. } => fields,
            other => panic!("expected a struct, found {other:?}"),
        }
    }

    fn enum_parts(&self, h: u64) -> (u32, &[u64]) {
        match self.obj(h) {
            Obj::Enum { tag, fields, .. } => (*tag, fields),
            other => panic!("expected an enum, found {other:?}"),
        }
    }
}
