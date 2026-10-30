//! A reference interpreter for the IR.
//!
//! It exists to test the compiler front half in isolation (lowering and
//! every optimization are checked by running programs before and after) and
//! later serves as an independent oracle for the bytecode VM. It favors
//! clarity over speed: registers live in per-call vectors, the heap is an
//! arena that is never collected, and upvalues follow the same open/closed
//! protocol as the VM.

use std::collections::{HashMap, VecDeque};

use super::types::*;
use crate::builtin::Builtin;
use crate::runtime::mathfn;
use crate::runtime::numeric::{self, CmpOp, NumTy};
use crate::runtime::value::{format_value, value_cmp, value_eq};
use crate::runtime::{HeapView, Trap};

const MAX_DEPTH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Bits(u64),
    Str(String),
}

#[derive(Debug, Default)]
struct MapObj {
    entries: Vec<(u64, u64)>,
    index: HashMap<Key, usize>,
    key_is_str: bool,
}

#[derive(Debug)]
enum Obj {
    Str(String),
    Array(Vec<u64>),
    Map(MapObj),
    Set(MapObj),
    Struct(Vec<u64>),
    Enum(u32, Vec<u64>),
    Closure { func: FuncId, cells: Vec<usize> },
}

#[derive(Debug, Clone, Copy)]
enum Cell {
    Open { depth: usize, reg: u32 },
    Closed(u64),
}

#[derive(Debug)]
pub struct Interp<'p> {
    prog: &'p IrProgram,
    heap: Vec<Obj>,
    cells: Vec<Cell>,
    open: Vec<usize>,
    frames: Vec<Vec<u64>>,
    globals: Vec<u64>,
    output: String,
    input: VecDeque<String>,
    args: Vec<String>,
    steps: u64,
    max_steps: u64,
}

/// Result of running a program.
#[derive(Debug)]
pub struct RunResult {
    pub output: String,
    pub trap: Option<Trap>,
}

/// Runs `prog` from its entry function, feeding `input` lines to `input()`.
pub fn run(prog: &IrProgram, input: &[&str]) -> RunResult {
    run_with_args(prog, input, &[])
}

/// Like [`run`], with the program's command-line arguments (`sys.args`).
pub fn run_with_args(prog: &IrProgram, input: &[&str], args: &[&str]) -> RunResult {
    let mut it = Interp {
        prog,
        heap: Vec::new(),
        cells: Vec::new(),
        open: Vec::new(),
        frames: Vec::new(),
        globals: vec![0; prog.globals.len()],
        output: String::new(),
        input: input.iter().map(|s| s.to_string()).collect(),
        args: args.iter().map(|s| s.to_string()).collect(),
        steps: 0,
        max_steps: 200_000_000,
    };
    let trap = it.call(prog.entry, vec![], &[]).err();
    RunResult { output: it.output, trap }
}

/// The value a constant expression evaluates to.
#[derive(Debug, Clone, PartialEq)]
pub enum ConstValue {
    Bits(u64),
    Str(String),
}

/// Evaluates the entry function of `prog` (a call-free constant expression
/// built by the compiler) and returns its result.
pub fn eval_const(prog: &IrProgram) -> Result<ConstValue, Trap> {
    let mut it = Interp {
        prog,
        heap: Vec::new(),
        cells: Vec::new(),
        open: Vec::new(),
        frames: Vec::new(),
        globals: vec![0; prog.globals.len()],
        output: String::new(),
        input: VecDeque::new(),
        args: Vec::new(),
        steps: 0,
        max_steps: 1_000_000,
    };
    let v = it.call(prog.entry, vec![], &[])?;
    Ok(match prog.func(prog.entry).ret {
        IrTy::Ref => ConstValue::Str(it.str_of(v).to_string()),
        _ => ConstValue::Bits(v),
    })
}

impl HeapView for Interp<'_> {
    fn str_of(&self, h: u64) -> &str {
        match &self.heap[h as usize] {
            Obj::Str(s) => s,
            other => panic!("expected a string, found {other:?}"),
        }
    }

    fn array_of(&self, h: u64) -> &[u64] {
        match &self.heap[h as usize] {
            Obj::Array(v) => v,
            other => panic!("expected an array, found {other:?}"),
        }
    }

    fn map_entries(&self, h: u64) -> Vec<(u64, u64)> {
        match &self.heap[h as usize] {
            Obj::Map(m) => m.entries.clone(),
            other => panic!("expected a map, found {other:?}"),
        }
    }

    fn set_elems(&self, h: u64) -> Vec<u64> {
        match &self.heap[h as usize] {
            Obj::Set(m) => m.entries.iter().map(|e| e.0).collect(),
            other => panic!("expected a set, found {other:?}"),
        }
    }

    fn struct_fields(&self, h: u64) -> &[u64] {
        match &self.heap[h as usize] {
            Obj::Struct(f) => f,
            other => panic!("expected a struct, found {other:?}"),
        }
    }

    fn enum_parts(&self, h: u64) -> (u32, &[u64]) {
        match &self.heap[h as usize] {
            Obj::Enum(t, f) => (*t, f),
            other => panic!("expected an enum, found {other:?}"),
        }
    }
}

impl Interp<'_> {
    fn alloc(&mut self, o: Obj) -> u64 {
        self.heap.push(o);
        self.heap.len() as u64 - 1
    }

    fn alloc_str(&mut self, s: String) -> u64 {
        self.alloc(Obj::Str(s))
    }

    fn key_of(&self, m: &MapObj, bits: u64) -> Key {
        if m.key_is_str {
            Key::Str(self.str_of(bits).to_string())
        } else {
            Key::Bits(bits)
        }
    }

    fn map_ref(&self, h: u64) -> &MapObj {
        match &self.heap[h as usize] {
            Obj::Map(m) | Obj::Set(m) => m,
            other => panic!("expected a map or set, found {other:?}"),
        }
    }

    fn map_mut(&mut self, h: u64) -> &mut MapObj {
        match &mut self.heap[h as usize] {
            Obj::Map(m) | Obj::Set(m) => m,
            other => panic!("expected a map or set, found {other:?}"),
        }
    }

    fn map_insert(&mut self, h: u64, k: u64, v: u64) {
        let key = {
            let m = self.map_ref(h);
            self.key_of(m, k)
        };
        let m = self.map_mut(h);
        match m.index.get(&key) {
            Some(&i) => m.entries[i].1 = v,
            None => {
                m.index.insert(key, m.entries.len());
                m.entries.push((k, v));
            }
        }
    }

    fn map_find(&self, h: u64, k: u64) -> Option<usize> {
        let m = self.map_ref(h);
        m.index.get(&self.key_of(m, k)).copied()
    }

    fn map_remove(&mut self, h: u64, k: u64) -> bool {
        let Some(i) = self.map_find(h, k) else { return false };
        let key_is_str = self.map_ref(h).key_is_str;
        let entries: Vec<(u64, u64)> = {
            let m = self.map_mut(h);
            m.entries.remove(i);
            m.entries.clone()
        };
        let keys: Vec<Key> = entries
            .iter()
            .map(|e| if key_is_str { Key::Str(self.str_of(e.0).to_string()) } else { Key::Bits(e.0) })
            .collect();
        let m = self.map_mut(h);
        m.index = keys.into_iter().enumerate().map(|(i, k)| (k, i)).collect();
        true
    }

    // ---- calls -------------------------------------------------------------------------------

    fn call(&mut self, func: FuncId, args: Vec<u64>, upvals: &[usize]) -> Result<u64, Trap> {
        if self.frames.len() >= MAX_DEPTH {
            return Err(Trap::StackOverflow);
        }
        let f = &self.prog.funcs[func.index()];
        let mut regs = vec![0u64; f.regs.len()];
        regs[..args.len()].copy_from_slice(&args);
        self.frames.push(regs);
        let fi = self.frames.len() - 1;
        let result = self.run_body(func, fi, upvals);
        self.close_frame(fi);
        self.frames.pop();
        result
    }

    fn close_frame(&mut self, depth: usize) {
        self.close_where(|d, _| d == depth);
    }

    fn close_where(&mut self, pred: impl Fn(usize, u32) -> bool) {
        let mut still_open = Vec::new();
        for &c in &self.open {
            if let Cell::Open { depth, reg } = self.cells[c] {
                if pred(depth, reg) {
                    self.cells[c] = Cell::Closed(self.frames[depth][reg as usize]);
                    continue;
                }
            }
            still_open.push(c);
        }
        self.open = still_open;
    }

    fn capture_local(&mut self, depth: usize, reg: u32) -> usize {
        for &c in &self.open {
            if let Cell::Open { depth: d, reg: r } = self.cells[c] {
                if d == depth && r == reg {
                    return c;
                }
            }
        }
        self.cells.push(Cell::Open { depth, reg });
        let id = self.cells.len() - 1;
        self.open.push(id);
        id
    }

    fn cell_get(&self, c: usize) -> u64 {
        match self.cells[c] {
            Cell::Open { depth, reg } => self.frames[depth][reg as usize],
            Cell::Closed(v) => v,
        }
    }

    fn cell_set(&mut self, c: usize, v: u64) {
        match self.cells[c] {
            Cell::Open { depth, reg } => self.frames[depth][reg as usize] = v,
            Cell::Closed(_) => self.cells[c] = Cell::Closed(v),
        }
    }

    fn run_body(&mut self, func: FuncId, fi: usize, upvals: &[usize]) -> Result<u64, Trap> {
        let prog = self.prog;
        let f = &prog.funcs[func.index()];
        let mut block = 0usize;
        loop {
            let b = &f.blocks[block];
            for inst in &b.insts {
                self.steps += 1;
                if self.steps > self.max_steps {
                    return Err(Trap::Panic("step limit exceeded".into()));
                }
                self.exec(inst, fi, upvals)?;
            }
            match &b.term {
                Term::Jump(t) => block = t.index(),
                Term::Branch { cond, then, els } => {
                    block = if self.frames[fi][cond.index()] != 0 { then.index() } else { els.index() };
                }
                Term::BranchTag { obj, tag, then, els } => {
                    let (t, _) = self.enum_parts(self.frames[fi][obj.index()]);
                    block = if t == *tag { then.index() } else { els.index() };
                }
                Term::Return(r) => return Ok(self.frames[fi][r.index()]),
                Term::Unreachable => return Err(Trap::Panic(format!("reached unreachable code in `{}`", f.name))),
            }
        }
    }

    fn exec(&mut self, inst: &Inst, fi: usize, upvals: &[usize]) -> Result<(), Trap> {
        macro_rules! r {
            ($v:expr) => {
                self.frames[fi][$v.index()]
            };
        }
        match inst {
            Inst::Const { dst, val } => {
                r!(dst) = match val {
                    Const::Bits(b) => *b,
                    Const::Unit => 0,
                    Const::Str(i) => {
                        let s = self.prog.strings[*i as usize].clone();
                        self.alloc_str(s)
                    }
                };
            }
            Inst::Move { dst, src } => r!(dst) = r!(src),
            Inst::Arith { dst, op, ty, lhs, rhs } => r!(dst) = numeric::arith(*op, *ty, r!(lhs), r!(rhs))?,
            Inst::Cmp { dst, op, ty, lhs, rhs } => {
                let (a, b) = (r!(lhs), r!(rhs));
                let res = match ty {
                    CmpTy::Num(n) => numeric::compare(*op, *n, a, b),
                    CmpTy::Str => {
                        let ord = self.str_of(a).cmp(self.str_of(b));
                        match op {
                            CmpOp::Eq => ord.is_eq(),
                            CmpOp::Ne => ord.is_ne(),
                            CmpOp::Lt => ord.is_lt(),
                            CmpOp::Le => ord.is_le(),
                            CmpOp::Gt => ord.is_gt(),
                            CmpOp::Ge => ord.is_ge(),
                        }
                    }
                };
                r!(dst) = res as u64;
            }
            Inst::Neg { dst, ty, src } => r!(dst) = numeric::neg(*ty, r!(src))?,
            Inst::BitNot { dst, ty, src } => r!(dst) = numeric::bit_not(*ty, r!(src)),
            Inst::BoolNot { dst, src } => r!(dst) = (r!(src) == 0) as u64,
            Inst::Cast { dst, from, to, src } => r!(dst) = numeric::cast(*from, *to, r!(src)),

            Inst::NewStruct { dst, fields, .. } => {
                let f = fields.iter().map(|&x| r!(x)).collect();
                r!(dst) = self.alloc(Obj::Struct(f));
            }
            Inst::GetField { dst, obj, idx } => r!(dst) = self.struct_fields(r!(obj))[*idx as usize],
            Inst::SetField { obj, idx, src } => {
                let (o, v) = (r!(obj), r!(src));
                match &mut self.heap[o as usize] {
                    Obj::Struct(f) => f[*idx as usize] = v,
                    other => panic!("SetField on {other:?}"),
                }
            }
            Inst::NewEnum { dst, tag, fields, .. } => {
                let f = fields.iter().map(|&x| r!(x)).collect();
                r!(dst) = self.alloc(Obj::Enum(*tag, f));
            }
            Inst::EnumTag { dst, obj } => r!(dst) = self.enum_parts(r!(obj)).0 as u64,
            Inst::EnumField { dst, obj, idx } => r!(dst) = self.enum_parts(r!(obj)).1[*idx as usize],
            Inst::NewArray { dst, elems, .. } => {
                let v = elems.iter().map(|&x| r!(x)).collect();
                r!(dst) = self.alloc(Obj::Array(v));
            }
            Inst::ArrayGet { dst, arr, idx, idx_ty } => {
                let a = self.array_of(r!(arr));
                let _ = idx_ty; // the register is read as an i64, like the VM does
                let i = mathfn::checked_index(NumTy::I64, r!(idx), a.len())?;
                r!(dst) = a[i];
            }
            Inst::ArraySet { arr, idx, idx_ty, src } => {
                let (h, v) = (r!(arr), r!(src));
                let len = self.array_of(h).len();
                let _ = idx_ty;
                let i = mathfn::checked_index(NumTy::I64, r!(idx), len)?;
                match &mut self.heap[h as usize] {
                    Obj::Array(a) => a[i] = v,
                    other => panic!("ArraySet on {other:?}"),
                }
            }
            Inst::NewMap { dst, desc, entries } => {
                let TypeDesc::Map(k, _) = self.prog.desc(*desc) else { panic!("NewMap with a non-map descriptor") };
                let key_is_str = matches!(self.prog.desc(*k), TypeDesc::Str);
                let h = self.alloc(Obj::Map(MapObj { key_is_str, ..MapObj::default() }));
                for &(k, v) in entries {
                    self.map_insert(h, r!(k), r!(v));
                }
                r!(dst) = h;
            }
            Inst::MapGet { dst, map, key } => {
                let h = r!(map);
                let i = self.map_find(h, r!(key)).ok_or(Trap::KeyNotFound)?;
                r!(dst) = self.map_ref(h).entries[i].1;
            }
            Inst::MapSet { map, key, val } => self.map_insert(r!(map), r!(key), r!(val)),
            Inst::NewSet { dst, desc, elems } => {
                let TypeDesc::Set(e) = self.prog.desc(*desc) else { panic!("NewSet with a non-set descriptor") };
                let key_is_str = matches!(self.prog.desc(*e), TypeDesc::Str);
                let h = self.alloc(Obj::Set(MapObj { key_is_str, ..MapObj::default() }));
                for &e in elems {
                    self.map_insert(h, r!(e), 0);
                }
                r!(dst) = h;
            }

            Inst::Call { dst, func, args } => {
                let a = args.iter().map(|&x| r!(x)).collect();
                let v = self.call(*func, a, &[])?;
                r!(dst) = v;
            }
            Inst::CallClosure { dst, callee, args } => {
                let a: Vec<u64> = args.iter().map(|&x| r!(x)).collect();
                let h = r!(callee);
                let (func, cells) = match &self.heap[h as usize] {
                    Obj::Closure { func, cells } => (*func, cells.clone()),
                    other => panic!("calling {other:?}"),
                };
                let v = self.call(func, a, &cells)?;
                r!(dst) = v;
            }
            Inst::MakeClosure { dst, func, captures } => {
                let mut cells = Vec::new();
                for c in captures {
                    cells.push(match c {
                        Capture::Local(reg) => self.capture_local(fi, reg.0),
                        Capture::Upvalue(i) => upvals[*i as usize],
                    });
                }
                r!(dst) = self.alloc(Obj::Closure { func: *func, cells });
            }
            Inst::GetUpval { dst, idx } => r!(dst) = self.cell_get(upvals[*idx as usize]),
            Inst::SetUpval { idx, src } => {
                let v = r!(src);
                self.cell_set(upvals[*idx as usize], v);
            }
            Inst::CloseUpvals { regs } => {
                let regs: Vec<u32> = regs.iter().map(|r| r.0).collect();
                self.close_where(|d, r| d == fi && regs.contains(&r));
            }
            Inst::GetGlobal { dst, slot } => r!(dst) = self.globals[slot.index()],
            Inst::SetGlobal { slot, src } => self.globals[slot.index()] = r!(src),
            Inst::Intrinsic { dst, b, desc, args } => {
                let a: Vec<u64> = args.iter().map(|&x| r!(x)).collect();
                let v = self.intrinsic(*b, *desc, &a)?;
                r!(dst) = v;
            }
        }
        Ok(())
    }

    // ---- builtins ---------------------------------------------------------------------------------

    fn num_ty(&self, desc: Option<DescId>) -> NumTy {
        match desc.map(|d| self.prog.desc(d)) {
            Some(TypeDesc::Num(n)) => *n,
            other => panic!("expected a numeric descriptor, got {other:?}"),
        }
    }

    fn elem_desc(&self, desc: Option<DescId>) -> DescId {
        match desc.map(|d| self.prog.desc(d)) {
            Some(TypeDesc::Array(e)) | Some(TypeDesc::Set(e)) => *e,
            Some(TypeDesc::Map(k, _)) => *k,
            other => panic!("expected a collection descriptor, got {other:?}"),
        }
    }

    fn format(&self, d: DescId, bits: u64) -> String {
        let mut s = String::new();
        format_value(self, &self.prog.descs, d, bits, false, &mut s);
        s
    }

    fn some(&mut self, v: u64) -> u64 {
        self.alloc(Obj::Enum(crate::runtime::value::SOME_TAG, vec![v]))
    }

    fn none(&mut self) -> u64 {
        self.alloc(Obj::Enum(crate::runtime::value::NONE_TAG, vec![]))
    }

    fn option(&mut self, v: Option<u64>) -> u64 {
        match v {
            Some(v) => self.some(v),
            None => self.none(),
        }
    }

    fn ok(&mut self, v: u64) -> u64 {
        self.alloc(Obj::Enum(crate::runtime::value::OK_TAG, vec![v]))
    }

    fn err_str(&mut self, message: String) -> u64 {
        let m = self.alloc_str(message);
        self.alloc(Obj::Enum(crate::runtime::value::ERR_TAG, vec![m]))
    }

    fn intrinsic(&mut self, b: Builtin, desc: Option<DescId>, a: &[u64]) -> Result<u64, Trap> {
        use Builtin::*;
        let char_at = |s: &str, i: usize| s.chars().nth(i);
        Ok(match b {
            Print | Println => {
                if let Some(&v) = a.first() {
                    let s = self.format(desc.expect("print has a descriptor"), v);
                    self.output.push_str(&s);
                }
                if b == Println {
                    self.output.push('\n');
                }
                0
            }
            Input => {
                let line = self.input.pop_front().unwrap_or_default();
                self.alloc_str(line)
            }
            Panic => return Err(Trap::Panic(self.str_of(a[0]).to_string())),
            Assert => {
                if a[0] == 0 {
                    let msg = a.get(1).map(|&m| format!(": {}", self.str_of(m))).unwrap_or_default();
                    return Err(Trap::Panic(format!("assertion failed{msg}")));
                }
                0
            }
            ToString => {
                let s = self.format(desc.expect("to_string has a descriptor"), a[0]);
                self.alloc_str(s)
            }
            Format => {
                let d = desc.expect("format has a descriptor");
                let kind = crate::runtime::fmtspec::kind_of(self.prog.desc(d))
                    .ok_or_else(|| Trap::Panic("this type cannot take a format specifier".into()))?;
                let text = (kind == crate::runtime::fmtspec::Kind::Str).then(|| self.str_of(a[0]).to_string());
                let s = crate::runtime::fmtspec::format_with(kind, a[0], text.as_deref(), self.str_of(a[1]))
                    .map_err(|e| Trap::Panic(format!("invalid format specifier: {e}")))?;
                self.alloc_str(s)
            }

            StrConcat => {
                let s = format!("{}{}", self.str_of(a[0]), self.str_of(a[1]));
                self.alloc_str(s)
            }
            StrLen => self.str_of(a[0]).chars().count() as u64,
            StrIsEmpty => self.str_of(a[0]).is_empty() as u64,
            StrChars => {
                let v: Vec<u64> = self.str_of(a[0]).chars().map(|c| c as u64).collect();
                self.alloc(Obj::Array(v))
            }
            StrSubstring => {
                let s = self.str_of(a[0]);
                let n = s.chars().count();
                let (lo, hi) = (a[1] as i64, a[2] as i64);
                if lo < 0 || hi < lo || hi as usize > n {
                    return Err(Trap::IndexOutOfBounds {
                        index: if lo < 0 || hi < lo { lo as i128 } else { hi as i128 },
                        len: n,
                    });
                }
                let sub: String = s.chars().skip(lo as usize).take((hi - lo) as usize).collect();
                self.alloc_str(sub)
            }
            StrContains => self.str_of(a[0]).contains(self.str_of(a[1])) as u64,
            StrStartsWith => self.str_of(a[0]).starts_with(self.str_of(a[1])) as u64,
            StrEndsWith => self.str_of(a[0]).ends_with(self.str_of(a[1])) as u64,
            StrToUpper => {
                let s = self.str_of(a[0]).to_uppercase();
                self.alloc_str(s)
            }
            StrToLower => {
                let s = self.str_of(a[0]).to_lowercase();
                self.alloc_str(s)
            }
            StrTrim => {
                let s = self.str_of(a[0]).trim().to_string();
                self.alloc_str(s)
            }
            StrSplit => {
                let parts: Vec<String> = {
                    let (s, sep) = (self.str_of(a[0]), self.str_of(a[1]));
                    if sep.is_empty() {
                        s.chars().map(String::from).collect()
                    } else {
                        s.split(sep).map(String::from).collect()
                    }
                };
                let handles: Vec<u64> = parts.into_iter().map(|p| self.alloc_str(p)).collect();
                self.alloc(Obj::Array(handles))
            }
            StrCharAt => {
                let s = self.str_of(a[0]);
                let i = a[1] as i64;
                match (i >= 0).then(|| char_at(s, i as usize)).flatten() {
                    Some(c) => c as u64,
                    None => return Err(Trap::IndexOutOfBounds { index: i as i128, len: s.chars().count() }),
                }
            }
            StrFind => {
                let (s, sub) = (self.str_of(a[0]), self.str_of(a[1]));
                match s.find(sub) {
                    Some(byte) => s[..byte].chars().count() as u64,
                    None => (-1i64) as u64,
                }
            }
            StrParseI64 => mathfn::parse_i64(self.str_of(a[0]))?,
            StrParseU64 => mathfn::parse_u64(self.str_of(a[0]))?,
            StrParseF64 => mathfn::parse_f64(self.str_of(a[0]))?,

            ArrLen => self.array_of(a[0]).len() as u64,
            ArrIsEmpty => self.array_of(a[0]).is_empty() as u64,
            ArrPush => {
                match &mut self.heap[a[0] as usize] {
                    Obj::Array(v) => v.push(a[1]),
                    other => panic!("push on {other:?}"),
                }
                0
            }
            ArrPop => match &mut self.heap[a[0] as usize] {
                Obj::Array(v) => v.pop().ok_or(Trap::EmptyArray)?,
                other => panic!("pop on {other:?}"),
            },
            ArrClear => {
                if let Obj::Array(v) = &mut self.heap[a[0] as usize] {
                    v.clear();
                }
                0
            }
            ArrContains => {
                let e = self.elem_desc(desc);
                self.array_of(a[0]).iter().any(|&x| value_eq(self, &self.prog.descs, e, x, a[1])) as u64
            }
            ArrInsert => {
                let len = self.array_of(a[0]).len();
                let i = a[1] as i64;
                if i < 0 || i as usize > len {
                    return Err(Trap::IndexOutOfBounds { index: i as i128, len });
                }
                if let Obj::Array(v) = &mut self.heap[a[0] as usize] {
                    v.insert(i as usize, a[2]);
                }
                0
            }
            ArrRemove => {
                let len = self.array_of(a[0]).len();
                let i = a[1] as i64;
                if i < 0 || i as usize >= len {
                    return Err(Trap::IndexOutOfBounds { index: i as i128, len });
                }
                match &mut self.heap[a[0] as usize] {
                    Obj::Array(v) => v.remove(i as usize),
                    other => panic!("remove on {other:?}"),
                }
            }
            ArrReverse => {
                if let Obj::Array(v) = &mut self.heap[a[0] as usize] {
                    v.reverse();
                }
                0
            }
            ArrClone => {
                let v = self.array_of(a[0]).to_vec();
                self.alloc(Obj::Array(v))
            }
            ArrJoin => {
                let sep = self.str_of(a[1]).to_string();
                let joined =
                    self.array_of(a[0]).iter().map(|&h| self.str_of(h).to_string()).collect::<Vec<_>>().join(&sep);
                self.alloc_str(joined)
            }
            ArrSort => {
                let e = self.elem_desc(desc);
                let mut v = self.array_of(a[0]).to_vec();
                // a stable sort with unordered (NaN) values last
                v.sort_by(|&x, &y| {
                    value_cmp(self, &self.prog.descs, e, x, y).unwrap_or_else(|| {
                        let nan_x = value_cmp(self, &self.prog.descs, e, x, x).is_none();
                        let nan_y = value_cmp(self, &self.prog.descs, e, y, y).is_none();
                        nan_x.cmp(&nan_y)
                    })
                });
                if let Obj::Array(arr) = &mut self.heap[a[0] as usize] {
                    *arr = v;
                }
                0
            }

            MapLen | SetLen => self.map_ref(a[0]).entries.len() as u64,
            MapIsEmpty | SetIsEmpty => self.map_ref(a[0]).entries.is_empty() as u64,
            MapInsert => {
                self.map_insert(a[0], a[1], a[2]);
                0
            }
            MapGetOr => match self.map_find(a[0], a[1]) {
                Some(i) => self.map_ref(a[0]).entries[i].1,
                None => a[2],
            },
            MapContainsKey | SetContains => self.map_find(a[0], a[1]).is_some() as u64,
            MapRemove | SetRemove => self.map_remove(a[0], a[1]) as u64,
            MapKeys | SetToArray => {
                let v = self.map_ref(a[0]).entries.iter().map(|e| e.0).collect();
                self.alloc(Obj::Array(v))
            }
            MapValues => {
                let v = self.map_ref(a[0]).entries.iter().map(|e| e.1).collect();
                self.alloc(Obj::Array(v))
            }
            MapClear | SetClear => {
                let m = self.map_mut(a[0]);
                m.entries.clear();
                m.index.clear();
                0
            }
            SetInsert => {
                let fresh = self.map_find(a[0], a[1]).is_none();
                self.map_insert(a[0], a[1], 0);
                fresh as u64
            }

            StrTryParseI64 | StrTryParseU64 | StrTryParseF64 => {
                let parsed = match b {
                    StrTryParseI64 => mathfn::parse_i64(self.str_of(a[0])),
                    StrTryParseU64 => mathfn::parse_u64(self.str_of(a[0])),
                    _ => mathfn::parse_f64(self.str_of(a[0])),
                };
                match parsed {
                    Ok(v) => self.ok(v),
                    Err(e) => self.err_str(e.to_string()),
                }
            }
            StrGetChar => {
                let i = a[1] as i64;
                let c = (i >= 0).then(|| self.str_of(a[0]).chars().nth(i as usize)).flatten().map(|c| c as u64);
                self.option(c)
            }
            StrTrySubstring => {
                let s = self.str_of(a[0]);
                let n = s.chars().count();
                let (lo, hi) = (a[1] as i64, a[2] as i64);
                if lo < 0 || hi < lo || hi as usize > n {
                    let index = if lo < 0 || hi < lo { lo as i128 } else { hi as i128 };
                    self.err_str(Trap::IndexOutOfBounds { index, len: n }.to_string())
                } else {
                    let sub: String = s.chars().skip(lo as usize).take((hi - lo) as usize).collect();
                    let v = self.alloc_str(sub);
                    self.ok(v)
                }
            }
            StrIndexOf => {
                let (s, sub) = (self.str_of(a[0]), self.str_of(a[1]));
                let at = s.find(sub).map(|byte| s[..byte].chars().count() as u64);
                self.option(at)
            }
            ArrGet => {
                let i = a[1] as i64;
                let e = (i >= 0).then(|| self.array_of(a[0]).get(i as usize).copied()).flatten();
                self.option(e)
            }
            ArrTryPop => {
                let e = match &mut self.heap[a[0] as usize] {
                    Obj::Array(v) => v.pop(),
                    other => panic!("pop on {other:?}"),
                };
                self.option(e)
            }
            ArrTryRemove => {
                let len = self.array_of(a[0]).len();
                let i = a[1] as i64;
                let e = if i >= 0 && (i as usize) < len {
                    match &mut self.heap[a[0] as usize] {
                        Obj::Array(v) => Some(v.remove(i as usize)),
                        other => panic!("remove on {other:?}"),
                    }
                } else {
                    None
                };
                self.option(e)
            }
            MapGet => {
                let e = self.map_find(a[0], a[1]).map(|i| self.map_ref(a[0]).entries[i].1);
                self.option(e)
            }
            CheckedAdd | CheckedSub | CheckedMul | CheckedDiv | CheckedRem | CheckedPow | CheckedShl | CheckedShr
            | CheckedNeg => {
                let ty = crate::runtime::value::option_payload_num(&self.prog.descs, desc.expect("result descriptor"));
                let x = if b == CheckedNeg { 0 } else { a[1] };
                self.option(mathfn::checked_op(b, ty, a[0], x))
            }
            Args => {
                let handles: Vec<u64> = self.args.clone().into_iter().map(|a| self.alloc_str(a)).collect();
                self.alloc(Obj::Array(handles))
            }
            ReadLine => match self.input.pop_front() {
                Some(l) => {
                    let s = self.alloc_str(l);
                    self.some(s)
                }
                None => self.none(),
            },

            Abs => mathfn::abs(self.num_ty(desc), a[0])?,
            Pow => mathfn::pow(self.num_ty(desc), a[0], a[1])?,
            Min => mathfn::min_max(self.num_ty(desc), a[0], a[1], false),
            Max => mathfn::min_max(self.num_ty(desc), a[0], a[1], true),
            Sqrt | Sin | Cos | Tan | Floor | Ceil | Round | Ln | Exp => {
                mathfn::float_fn(b, self.num_ty(desc), a[0]).expect("float builtin")
            }
            IsNan => mathfn::is_nan(self.num_ty(desc), a[0]) as u64,

            CharIsDigit => char::from_u32(a[0] as u32).is_some_and(|c| c.is_ascii_digit()) as u64,
            CharIsAlphabetic => char::from_u32(a[0] as u32).is_some_and(char::is_alphabetic) as u64,
            CharIsWhitespace => char::from_u32(a[0] as u32).is_some_and(char::is_whitespace) as u64,
            CharToUpper => char::from_u32(a[0] as u32).and_then(|c| c.to_uppercase().next()).map_or(a[0], |c| c as u64),
            CharToLower => char::from_u32(a[0] as u32).and_then(|c| c.to_lowercase().next()).map_or(a[0], |c| c as u64),
        })
    }
}
