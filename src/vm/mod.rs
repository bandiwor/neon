//! The register virtual machine.
//!
//! Frames overlap in one register stack: a call's argument window becomes the
//! callee's frame, and the result comes back in the window's first register.
//! Values are untagged 64-bit words; the collector learns which registers are
//! references from the compiler's stack maps (see [`crate::gc`]).

mod builtins;
mod io;

use std::fmt;

pub use io::{BufferIo, StreamIo, VmIo};

use crate::builtin::Builtin;
use crate::bytecode::opcode::{CmpKind, FusedKind, Opcode, EXT_ESCAPE};
use crate::bytecode::{BytecodeProgram, Chunk};
use crate::gc::{scan_globals, scan_stack, CellId, CellState, FrameInfo, GcConfig, Handle, Heap, HeapStats, Roots};
use crate::ir::TypeDesc;
use crate::runtime::numeric::{self, ArithOp, CmpOp, NumTy};
use crate::runtime::{mathfn, HeapView, Trap};

/// Limits and switches for a run.
#[derive(Debug, Clone, Copy)]
pub struct VmConfig {
    pub gc: GcConfig,
    /// Maximum call depth.
    pub max_frames: usize,
    /// Maximum size of the register stack, in 64-bit words.
    pub max_stack_words: usize,
    /// Stop with a panic after this many instructions (`None`: unlimited).
    pub max_steps: Option<u64>,
}

impl Default for VmConfig {
    fn default() -> Self {
        VmConfig { gc: GcConfig::default(), max_frames: 20_000, max_stack_words: 4 << 20, max_steps: None }
    }
}

/// A trap together with the call stack it happened in (innermost first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeError {
    pub trap: Trap,
    pub backtrace: Vec<String>,
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "runtime error: {}", self.trap)?;
        for (i, name) in self.backtrace.iter().enumerate() {
            if i >= 12 {
                write!(f, "\n  ... {} more frames", self.backtrace.len() - i)?;
                break;
            }
            write!(f, "\n  at {name}")?;
        }
        Ok(())
    }
}

impl std::error::Error for RuntimeError {}

/// What a run produced besides its output.
#[derive(Debug)]
pub struct RunReport {
    pub result: Result<(), RuntimeError>,
    pub gc: HeapStats,
    pub steps: u64,
}

/// Runs `prog` from its entry chunk.
pub fn run(prog: &BytecodeProgram, io: &mut dyn VmIo, cfg: &VmConfig) -> RunReport {
    let mut vm = Vm::new(prog, io, cfg);
    let result = vm.run();
    RunReport { result, gc: vm.heap.stats().clone(), steps: vm.steps }
}

/// A decoded opcode: families (`add.i32`, `lt.s`, ...) carry their operands so
/// the dispatch loop does not have to recover them per instruction.
#[derive(Debug, Clone, Copy)]
enum OpKind {
    Arith(ArithOp, NumTy),
    Neg(NumTy),
    BitNot(NumTy),
    AddImm(NumTy),
    SubImm(NumTy),
    Cmp(CmpKind),
    Fused(FusedKind),
    Other(Opcode),
    Invalid,
}

fn op_table() -> [OpKind; 256] {
    let mut t = [OpKind::Invalid; 256];
    for &op in Opcode::ALL {
        t[op as usize] = if let Some((a, ty)) = op.arith_parts() {
            OpKind::Arith(a, ty)
        } else if let Some(ty) = op.neg_ty() {
            OpKind::Neg(ty)
        } else if let Some(ty) = op.bnot_ty() {
            OpKind::BitNot(ty)
        } else if let Some(ty) = op.addi_ty() {
            OpKind::AddImm(ty)
        } else if let Some(ty) = op.subi_ty() {
            OpKind::SubImm(ty)
        } else if let Some(k) = op.cmp_kind() {
            OpKind::Cmp(k)
        } else if let Some(k) = op.fused_kind() {
            OpKind::Fused(k)
        } else {
            OpKind::Other(op)
        };
    }
    t
}

#[derive(Debug, Clone, Copy)]
struct Frame {
    chunk: u32,
    /// Absolute stack index of register 0.
    base: usize,
    /// The closure being run (0 for plain functions).
    closure: Handle,
    /// Where the return value goes (absolute stack index).
    ret_to: usize,
    /// Set while this frame is waiting for a call: the `call` instruction and
    /// the word to resume at.
    call_pc: usize,
    resume_pc: usize,
}

pub struct Vm<'p, 'io> {
    prog: &'p BytecodeProgram,
    heap: Heap,
    stack: Vec<u64>,
    frames: Vec<Frame>,
    globals: Vec<u64>,
    /// Open upvalue cells, ascending by the stack slot they point at.
    open: Vec<(usize, CellId)>,
    io: &'io mut dyn VmIo,
    cfg: VmConfig,
    steps: u64,
}

impl fmt::Debug for Vm<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vm").field("frames", &self.frames.len()).field("heap", &self.heap).finish()
    }
}

/// `(from, to)` for the cast instruction's third operand.
fn cast_types(code: u8) -> (NumTy, NumTy) {
    (NumTy::ALL[(code / 10) as usize], NumTy::ALL[(code % 10) as usize])
}

impl<'p, 'io> Vm<'p, 'io> {
    pub fn new(prog: &'p BytecodeProgram, io: &'io mut dyn VmIo, cfg: &VmConfig) -> Vm<'p, 'io> {
        Vm {
            prog,
            heap: Heap::new(&prog.descs, cfg.gc),
            stack: Vec::new(),
            frames: Vec::new(),
            globals: vec![0; prog.globals.len()],
            open: Vec::new(),
            io,
            cfg: *cfg,
            steps: 0,
        }
    }

    pub fn heap(&self) -> &Heap {
        &self.heap
    }

    /// Runs the entry chunk to completion.
    pub fn run(&mut self) -> Result<(), RuntimeError> {
        let entry = self.prog.entry;
        let chunk = &self.prog.chunks[entry as usize];
        self.stack.resize(chunk.frame_size as usize, 0);
        self.frames.push(Frame { chunk: entry, base: 0, closure: 0, ret_to: 0, call_pc: 0, resume_pc: 0 });
        match self.exec() {
            Ok(()) => Ok(()),
            Err(trap) => {
                let backtrace =
                    self.frames.iter().rev().map(|f| self.prog.chunks[f.chunk as usize].name.clone()).collect();
                Err(RuntimeError { trap, backtrace })
            }
        }
    }

    // ---- collection ---------------------------------------------------------------------------------

    /// Collects garbage. `pc` is where the innermost frame is stopped (`None`
    /// right after a call transferred control: only its parameters exist).
    fn collect(&mut self, pc: Option<usize>) {
        let mut roots = Roots::new();
        let n = self.frames.len();
        let infos: Vec<FrameInfo> = self
            .frames
            .iter()
            .enumerate()
            .filter(|(i, _)| pc.is_some() || *i + 1 < n)
            .map(|(i, f)| FrameInfo {
                chunk: f.chunk,
                pc: if i + 1 == n { pc.unwrap_or(0) as u32 } else { f.call_pc as u32 },
                base: f.base,
                closure: f.closure,
            })
            .collect();
        scan_stack(self.prog, &infos, &self.stack, &mut roots);
        if pc.is_none() {
            // the new frame: its parameters, and its closure
            let f = self.frames[n - 1];
            roots.handle(f.closure);
            for (i, &is_ref) in self.prog.chunks[f.chunk as usize].param_refs.iter().enumerate() {
                if is_ref {
                    roots.handle(self.stack[f.base + i]);
                }
            }
        }
        scan_globals(self.prog, &self.globals, &mut roots);
        for &(_, c) in &self.open {
            roots.cell(c);
        }
        self.heap.collect(&roots);
    }

    #[inline]
    fn safepoint(&mut self, pc: usize) {
        if self.heap.should_collect() {
            self.collect(Some(pc));
        }
    }

    // ---- upvalues -------------------------------------------------------------------------------------

    /// The open cell over stack slot `idx`, created on first capture.
    fn capture_local(&mut self, idx: usize, is_ref: bool) -> Result<CellId, Trap> {
        match self.open.binary_search_by_key(&idx, |e| e.0) {
            Ok(i) => Ok(self.open[i].1),
            Err(i) => {
                let c = self.heap.alloc_cell(CellState::Open(idx), is_ref)?;
                self.open.insert(i, (idx, c));
                Ok(c)
            }
        }
    }

    /// Closes every open cell over a slot at or above `from`.
    fn close_from(&mut self, from: usize) {
        while let Some(&(idx, c)) = self.open.last() {
            if idx < from {
                break;
            }
            self.heap.set_cell_state(c, CellState::Closed(self.stack[idx]));
            self.open.pop();
        }
    }

    fn close_slot(&mut self, idx: usize) {
        if let Ok(i) = self.open.binary_search_by_key(&idx, |e| e.0) {
            let (_, c) = self.open.remove(i);
            self.heap.set_cell_state(c, CellState::Closed(self.stack[idx]));
        }
    }

    fn cell_get(&self, c: CellId) -> u64 {
        match self.heap.cell_state(c) {
            CellState::Open(i) => self.stack[i],
            CellState::Closed(v) => v,
        }
    }

    fn cell_set(&mut self, c: CellId, v: u64) {
        match self.heap.cell_state(c) {
            CellState::Open(i) => self.stack[i] = v,
            CellState::Closed(_) => self.heap.set_cell_state(c, CellState::Closed(v)),
        }
    }

    // ---- calls --------------------------------------------------------------------------------------------

    /// Pushes a frame for `chunk` whose registers start at `base`.
    fn enter(&mut self, chunk: u32, base: usize, closure: Handle, ret_to: usize) -> Result<(), Trap> {
        let prog: &'p BytecodeProgram = self.prog;
        let c: &Chunk = &prog.chunks[chunk as usize];
        let need = base + c.frame_size as usize;
        if self.frames.len() >= self.cfg.max_frames || need > self.cfg.max_stack_words {
            return Err(Trap::StackOverflow);
        }
        if self.stack.len() < need {
            self.stack.resize(need, 0);
        }
        // registers beyond the arguments start out as null / zero
        self.stack[base + c.params as usize..need].fill(0);
        self.frames.push(Frame { chunk, base, closure, ret_to, call_pc: 0, resume_pc: 0 });
        self.safepoint_on_entry();
        Ok(())
    }

    fn safepoint_on_entry(&mut self) {
        if self.heap.should_collect() {
            self.collect(None);
        }
    }

    // ---- comparisons ------------------------------------------------------------------------------------------

    fn compare(&self, kind: CmpKind, a: u64, b: u64) -> bool {
        use CmpKind::*;
        match kind {
            Eq => a == b,
            Ne => a != b,
            LtS => (a as i64) < (b as i64),
            LeS => (a as i64) <= (b as i64),
            LtU => a < b,
            LeU => a <= b,
            EqF32 => numeric::compare(CmpOp::Eq, NumTy::F32, a, b),
            NeF32 => numeric::compare(CmpOp::Ne, NumTy::F32, a, b),
            LtF32 => numeric::compare(CmpOp::Lt, NumTy::F32, a, b),
            LeF32 => numeric::compare(CmpOp::Le, NumTy::F32, a, b),
            EqF64 => numeric::compare(CmpOp::Eq, NumTy::F64, a, b),
            NeF64 => numeric::compare(CmpOp::Ne, NumTy::F64, a, b),
            LtF64 => numeric::compare(CmpOp::Lt, NumTy::F64, a, b),
            LeF64 => numeric::compare(CmpOp::Le, NumTy::F64, a, b),
            EqStr => self.heap.str_of(a) == self.heap.str_of(b),
            NeStr => self.heap.str_of(a) != self.heap.str_of(b),
            LtStr => self.heap.str_of(a) < self.heap.str_of(b),
            LeStr => self.heap.str_of(a) <= self.heap.str_of(b),
        }
    }

    // ---- the dispatch loop ---------------------------------------------------------------------------------------

    fn exec(&mut self) -> Result<(), Trap> {
        let prog: &'p BytecodeProgram = self.prog;
        let ops = op_table();
        let mut frame = *self.frames.last().expect("a frame");
        let mut code: &'p [u32] = &prog.chunks[frame.chunk as usize].code;
        let mut base = frame.base;
        let mut pc = 0usize;
        let fuel = self.cfg.max_steps;

        macro_rules! reg {
            ($i:expr) => {
                self.stack[base + $i]
            };
        }
        // reload the cached frame state after the frame stack changed
        macro_rules! reload {
            ($resume:expr) => {{
                frame = *self.frames.last().expect("a frame");
                code = &prog.chunks[frame.chunk as usize].code;
                base = frame.base;
                pc = $resume;
            }};
        }
        macro_rules! jump {
            ($next:expr, $off:expr) => {
                pc = ($next as isize + $off as isize) as usize
            };
        }

        loop {
            if let Some(limit) = fuel {
                self.steps += 1;
                if self.steps > limit {
                    return Err(Trap::Panic("step limit exceeded".into()));
                }
            }
            let w = code[pc];
            let top = (w >> 24) as u8;
            let (opb, a, b, c) = if top == EXT_ESCAPE {
                (((w >> 16) & 0xFF) as u8, ((w >> 8) & 0xFF) as usize, (w & 0xFF) as usize, 0usize)
            } else {
                (top, ((w >> 16) & 0xFF) as usize, ((w >> 8) & 0xFF) as usize, (w & 0xFF) as usize)
            };
            match ops[opb as usize] {
                OpKind::Arith(op, ty) => {
                    reg!(a) = numeric::arith(op, ty, reg!(b), reg!(c))?;
                    pc += 1;
                }
                OpKind::Neg(ty) => {
                    reg!(a) = numeric::neg(ty, reg!(b))?;
                    pc += 1;
                }
                OpKind::BitNot(ty) => {
                    reg!(a) = numeric::bit_not(ty, reg!(b));
                    pc += 1;
                }
                OpKind::AddImm(ty) => {
                    let v = numeric::decode_int(ty, reg!(b)) + (c as u8 as i8) as i128;
                    reg!(a) = numeric::encode_int(ty, v).ok_or(Trap::Overflow)?;
                    pc += 1;
                }
                OpKind::SubImm(ty) => {
                    let v = numeric::decode_int(ty, reg!(b)) - c as i128;
                    reg!(a) = numeric::encode_int(ty, v).ok_or(Trap::Overflow)?;
                    pc += 1;
                }
                OpKind::Cmp(kind) => {
                    reg!(a) = self.compare(kind, reg!(b), reg!(c)) as u64;
                    pc += 1;
                }
                OpKind::Fused(kind) => {
                    let off = code[pc + 1] as i32;
                    if self.compare(kind.cmp(), reg!(a), reg!(b)) {
                        pc += 2;
                    } else {
                        jump!(pc + 2, off);
                    }
                }
                OpKind::Invalid => return Err(Trap::Panic(format!("invalid opcode {opb:#x}"))),
                OpKind::Other(op) => match op {
                    Opcode::Move => {
                        reg!(a) = reg!(b);
                        pc += 1;
                    }
                    Opcode::LoadI => {
                        reg!(a) = (w & 0xFFFF) as u16 as i16 as i64 as u64;
                        pc += 1;
                    }
                    Opcode::LoadK => {
                        reg!(a) = prog.constants[(w & 0xFFFF) as usize];
                        pc += 1;
                    }
                    Opcode::LoadKW => {
                        reg!(a) = prog.constants[code[pc + 1] as usize];
                        pc += 2;
                    }
                    Opcode::LoadStr | Opcode::LoadStrW => {
                        self.safepoint(pc);
                        let (idx, len) =
                            if op == Opcode::LoadStr { ((w & 0xFFFF) as usize, 1) } else { (code[pc + 1] as usize, 2) };
                        reg!(a) = self.heap.alloc_str(prog.strings[idx].clone())?;
                        pc += len;
                    }
                    Opcode::And => {
                        reg!(a) = reg!(b) & reg!(c);
                        pc += 1;
                    }
                    Opcode::Or => {
                        reg!(a) = reg!(b) | reg!(c);
                        pc += 1;
                    }
                    Opcode::Xor => {
                        reg!(a) = reg!(b) ^ reg!(c);
                        pc += 1;
                    }
                    Opcode::BoolNot => {
                        reg!(a) = (reg!(b) == 0) as u64;
                        pc += 1;
                    }
                    Opcode::Cast => {
                        let (from, to) = cast_types(c as u8);
                        reg!(a) = numeric::cast(from, to, reg!(b));
                        pc += 1;
                    }
                    Opcode::Jmp => jump!(pc + 1, (w & 0xFFFF) as u16 as i16),
                    Opcode::JmpIf => {
                        if reg!(a) != 0 {
                            jump!(pc + 1, (w & 0xFFFF) as u16 as i16);
                        } else {
                            pc += 1;
                        }
                    }
                    Opcode::JmpIfNot => {
                        if reg!(a) == 0 {
                            jump!(pc + 1, (w & 0xFFFF) as u16 as i16);
                        } else {
                            pc += 1;
                        }
                    }
                    Opcode::JmpW => jump!(pc + 2, code[pc + 1] as i32),
                    Opcode::JmpIfW => {
                        if reg!(a) != 0 {
                            jump!(pc + 2, code[pc + 1] as i32);
                        } else {
                            pc += 2;
                        }
                    }
                    Opcode::JmpIfNotW => {
                        if reg!(a) == 0 {
                            jump!(pc + 2, code[pc + 1] as i32);
                        } else {
                            pc += 2;
                        }
                    }
                    Opcode::MatchTagJmp => {
                        let (tag, _) = self.heap.enum_parts(reg!(a));
                        if tag as usize == b {
                            pc += 2;
                        } else {
                            jump!(pc + 2, code[pc + 1] as i32);
                        }
                    }
                    Opcode::Unreachable => return Err(Trap::Panic("reached unreachable code".into())),
                    Opcode::Ret => {
                        let v = reg!(a);
                        let done = self.frames.pop().expect("a frame");
                        self.close_from(done.base);
                        self.stack[done.ret_to] = v;
                        if self.frames.is_empty() {
                            return Ok(());
                        }
                        let resume = self.frames.last().unwrap().resume_pc;
                        reload!(resume);
                    }
                    Opcode::Call | Opcode::CallW => {
                        let (func, len) = if op == Opcode::Call { (w & 0xFFFF, 1) } else { (code[pc + 1], 2) };
                        let callee_base = base + a;
                        let top = self.frames.last_mut().unwrap();
                        top.call_pc = pc;
                        top.resume_pc = pc + len;
                        self.enter(func, callee_base, 0, callee_base)?;
                        reload!(0);
                    }
                    Opcode::CallClosure => {
                        let clo = reg!(a);
                        let (func, _) = self.heap.closure_parts(clo);
                        let top = self.frames.last_mut().unwrap();
                        top.call_pc = pc;
                        top.resume_pc = pc + 1;
                        // the closure sits in the window's first register; its arguments follow
                        self.enter(func, base + a + 1, clo, base + a)?;
                        reload!(0);
                    }
                    Opcode::GetField => {
                        reg!(a) = self.heap.struct_fields(reg!(b))[c];
                        pc += 1;
                    }
                    Opcode::SetField => {
                        let (o, v) = (reg!(a), reg!(c));
                        self.heap.set_field(o, b, v);
                        pc += 1;
                    }
                    Opcode::EnumTag => {
                        reg!(a) = self.heap.enum_parts(reg!(b)).0 as u64;
                        pc += 1;
                    }
                    Opcode::EnumField => {
                        reg!(a) = self.heap.enum_parts(reg!(b)).1[c];
                        pc += 1;
                    }
                    Opcode::ArrayGet => {
                        let arr = self.heap.array_of(reg!(b));
                        let i = mathfn::checked_index(NumTy::I64, reg!(c), arr.len())?;
                        reg!(a) = arr[i];
                        pc += 1;
                    }
                    Opcode::ArraySet => {
                        let (h, v) = (reg!(a), reg!(c));
                        let len = self.heap.array_of(h).len();
                        let i = mathfn::checked_index(NumTy::I64, reg!(b), len)?;
                        self.heap.with_array(h, |arr| arr[i] = v)?;
                        pc += 1;
                    }
                    Opcode::MapGet => {
                        let h = reg!(b);
                        let i = self.heap.map_find(h, reg!(c)).ok_or(Trap::KeyNotFound)?;
                        reg!(a) = self.heap.map_entries_ref(h)[i].1;
                        pc += 1;
                    }
                    Opcode::MapSet => {
                        self.safepoint(pc);
                        self.heap.map_insert(reg!(a), reg!(b), reg!(c))?;
                        pc += 1;
                    }
                    Opcode::GetUpval => {
                        let (_, cells) = self.heap.closure_parts(frame.closure);
                        reg!(a) = self.cell_get(cells[b]);
                        pc += 1;
                    }
                    Opcode::SetUpval => {
                        let (_, cells) = self.heap.closure_parts(frame.closure);
                        let cell = cells[a];
                        let v = reg!(b);
                        self.cell_set(cell, v);
                        pc += 1;
                    }
                    Opcode::Close => {
                        self.close_slot(base + a);
                        pc += 1;
                    }
                    Opcode::GetGlobal => {
                        reg!(a) = self.globals[(w & 0xFFFF) as usize];
                        pc += 1;
                    }
                    Opcode::SetGlobal => {
                        self.globals[(w & 0xFFFF) as usize] = reg!(a);
                        pc += 1;
                    }
                    Opcode::NewStruct | Opcode::NewEnum | Opcode::NewArray | Opcode::NewSet | Opcode::NewMap => {
                        self.safepoint(pc);
                        let w1 = code[pc + 1];
                        let desc = w1 & 0xFFFF;
                        let n = ((w1 >> 16) & 0xFF) as usize;
                        let start = base + b;
                        let h = match op {
                            Opcode::NewStruct => {
                                let fields = self.stack[start..start + n].to_vec();
                                self.heap.alloc_struct(desc, fields)?
                            }
                            Opcode::NewEnum => {
                                let fields = self.stack[start..start + n].to_vec();
                                self.heap.alloc_enum(desc, w1 >> 24, fields)?
                            }
                            Opcode::NewArray => {
                                let elems = self.stack[start..start + n].to_vec();
                                let refs = self.array_elems_are_refs(desc);
                                self.heap.alloc_array(elems, refs)?
                            }
                            Opcode::NewSet => {
                                let (key_str, key_ref) = self.set_shape(desc);
                                let h = self.heap.alloc_set(key_str, key_ref)?;
                                for i in 0..n {
                                    let k = self.stack[start + i];
                                    self.heap.map_insert(h, k, 0)?;
                                }
                                h
                            }
                            _ => {
                                let (key_str, key_ref, val_ref) = self.map_shape(desc);
                                let h = self.heap.alloc_map(key_str, key_ref, val_ref)?;
                                for i in 0..n {
                                    let (k, v) = (self.stack[start + 2 * i], self.stack[start + 2 * i + 1]);
                                    self.heap.map_insert(h, k, v)?;
                                }
                                h
                            }
                        };
                        reg!(a) = h;
                        pc += 2;
                    }
                    Opcode::MakeClosure => {
                        self.safepoint(pc);
                        let func = code[pc + 1];
                        let callee = &prog.chunks[func as usize];
                        let mut cells = Vec::with_capacity(b);
                        for k in 0..b {
                            let spec = code[pc + 2 + k];
                            let cell = if spec & 0x100 != 0 {
                                let (_, own) = self.heap.closure_parts(frame.closure);
                                own[(spec & 0xFF) as usize]
                            } else {
                                self.capture_local(base + spec as usize, callee.upvals[k])?
                            };
                            cells.push(cell);
                        }
                        reg!(a) = self.heap.alloc_closure(func, cells)?;
                        pc += 2 + b;
                    }
                    Opcode::Intrinsic => {
                        self.safepoint(pc);
                        let w1 = code[pc + 1];
                        let desc = (w1 & 0xFFFF) as u16;
                        let nargs = ((w1 >> 16) & 0xFF) as usize;
                        let builtin = Builtin::ALL[(w1 >> 24) as usize];
                        let mut args = [0u64; 8];
                        args[..nargs].copy_from_slice(&self.stack[base + b..base + b + nargs]);
                        let v = self.intrinsic(
                            builtin,
                            (desc != crate::bytecode::NO_DESC).then_some(desc as u32),
                            &args[..nargs],
                        )?;
                        reg!(a) = v;
                        pc += 2;
                    }
                    other => return Err(Trap::Panic(format!("opcode {other:?} is not executable here"))),
                },
            }
        }
    }

    // ---- type descriptor queries ------------------------------------------------------------------------------------

    fn desc(&self, d: u32) -> &'p TypeDesc {
        &self.prog.descs[d as usize]
    }

    fn array_elems_are_refs(&self, d: u32) -> bool {
        match self.desc(d) {
            TypeDesc::Array(e) => self.desc(e.0).is_ref(),
            other => panic!("expected an array descriptor, got {other:?}"),
        }
    }

    /// `(keys are strings, keys are references)` of a set type.
    fn set_shape(&self, d: u32) -> (bool, bool) {
        match self.desc(d) {
            TypeDesc::Set(e) => (matches!(self.desc(e.0), TypeDesc::Str), self.desc(e.0).is_ref()),
            other => panic!("expected a set descriptor, got {other:?}"),
        }
    }

    /// `(keys are strings, keys are references, values are references)` of a map type.
    fn map_shape(&self, d: u32) -> (bool, bool, bool) {
        match self.desc(d) {
            TypeDesc::Map(k, v) => {
                (matches!(self.desc(k.0), TypeDesc::Str), self.desc(k.0).is_ref(), self.desc(v.0).is_ref())
            }
            other => panic!("expected a map descriptor, got {other:?}"),
        }
    }

    fn format(&self, d: u32, bits: u64) -> String {
        let mut s = String::new();
        crate::runtime::value::format_value(&self.heap, &self.prog.descs, crate::ir::DescId(d), bits, false, &mut s);
        s
    }
}

#[cfg(test)]
mod tests;
