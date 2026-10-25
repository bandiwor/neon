//! Structured instructions: what the compiler emits, the peephole rewrites,
//! the assembler encodes and the disassembler prints.
//!
//! Jump targets are symbolic [`Label`]s (declared with [`Insn::Label`]
//! pseudo-instructions); the assembler turns them into relative offsets and
//! chooses between short (16-bit) and wide (32-bit) jump encodings.

use super::opcode::{CmpKind, FusedKind};
use crate::builtin::Builtin;
use crate::runtime::numeric::{ArithOp, NumTy};

/// A frame slot (register) number.
pub type Reg = u8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Label(pub u32);

/// How a closure obtains one captured variable when it is created.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capture {
    /// An open upvalue over this register of the creating frame.
    Local(Reg),
    /// Share the creating closure's own upvalue with this index.
    Upvalue(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Insn {
    /// Marks a jump target; occupies no space.
    Label(Label),

    Move {
        dst: Reg,
        src: Reg,
    },
    /// Loads a sign-extended 16-bit immediate (as raw 64-bit bits).
    LoadI {
        dst: Reg,
        imm: i16,
    },
    /// Loads constant-pool entry `k`.
    LoadK {
        dst: Reg,
        k: u32,
    },
    /// Allocates a string from string-pool entry `s`.
    LoadStr {
        dst: Reg,
        s: u32,
    },

    /// Checked arithmetic or shift on `ty` (never `And`/`Or`/`Xor`).
    Arith {
        op: ArithOp,
        ty: NumTy,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `And` / `Or` / `Xor` on canonical bits.
    Bitwise {
        op: ArithOp,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    /// `src + imm` on integer type `ty`, overflow-checked.
    AddImm {
        ty: NumTy,
        dst: Reg,
        src: Reg,
        imm: i8,
    },
    /// `src - imm` on integer type `ty`, overflow-checked.
    SubImm {
        ty: NumTy,
        dst: Reg,
        src: Reg,
        imm: u8,
    },
    Neg {
        ty: NumTy,
        dst: Reg,
        src: Reg,
    },
    BitNot {
        ty: NumTy,
        dst: Reg,
        src: Reg,
    },
    BoolNot {
        dst: Reg,
        src: Reg,
    },
    Cmp {
        kind: CmpKind,
        dst: Reg,
        a: Reg,
        b: Reg,
    },
    Cast {
        from: NumTy,
        to: NumTy,
        dst: Reg,
        src: Reg,
    },

    Jmp {
        to: Label,
    },
    JmpIf {
        cond: Reg,
        to: Label,
    },
    JmpIfNot {
        cond: Reg,
        to: Label,
    },
    /// Jumps to `to` if the comparison of `a` and `b` is *false*.
    CmpJmp {
        kind: FusedKind,
        a: Reg,
        b: Reg,
        to: Label,
    },
    /// Jumps to `to` if the enum value in `obj` is *not* variant `tag`.
    MatchTagJmp {
        obj: Reg,
        tag: u8,
        to: Label,
    },

    /// Calls function `func` with its arguments in `base..base+nargs`; the
    /// callee's frame starts at `base` and the result is left in `base`.
    /// (`nargs` is compile-time information only and is not encoded.)
    Call {
        base: Reg,
        func: u32,
        nargs: u8,
    },
    /// Calls the closure in `base` with arguments in `base+1..=base+nargs`;
    /// the result replaces the closure in `base`.
    CallClosure {
        base: Reg,
        nargs: u8,
    },
    Ret {
        src: Reg,
    },
    Unreachable,

    /// Builds a struct from registers `base..base+n`.
    NewStruct {
        dst: Reg,
        base: Reg,
        n: u8,
        desc: u16,
    },
    GetField {
        dst: Reg,
        obj: Reg,
        idx: u8,
    },
    SetField {
        obj: Reg,
        idx: u8,
        src: Reg,
    },
    NewEnum {
        dst: Reg,
        base: Reg,
        n: u8,
        desc: u16,
        tag: u8,
    },
    EnumTag {
        dst: Reg,
        obj: Reg,
    },
    EnumField {
        dst: Reg,
        obj: Reg,
        idx: u8,
    },
    NewArray {
        dst: Reg,
        base: Reg,
        n: u8,
        desc: u16,
    },
    ArrayGet {
        dst: Reg,
        arr: Reg,
        idx: Reg,
    },
    ArraySet {
        arr: Reg,
        idx: Reg,
        src: Reg,
    },
    /// Builds a map from `pairs` key/value register pairs at `base`.
    NewMap {
        dst: Reg,
        base: Reg,
        pairs: u8,
        desc: u16,
    },
    MapGet {
        dst: Reg,
        map: Reg,
        key: Reg,
    },
    MapSet {
        map: Reg,
        key: Reg,
        val: Reg,
    },
    NewSet {
        dst: Reg,
        base: Reg,
        n: u8,
        desc: u16,
    },

    MakeClosure {
        dst: Reg,
        func: u32,
        captures: Vec<Capture>,
    },
    GetUpval {
        dst: Reg,
        idx: u8,
    },
    SetUpval {
        idx: u8,
        src: Reg,
    },
    /// Closes open upvalues over register `reg`.
    Close {
        reg: Reg,
    },
    GetGlobal {
        dst: Reg,
        g: u16,
    },
    SetGlobal {
        src: Reg,
        g: u16,
    },

    /// A builtin with arguments in `base..base+nargs`.
    Intrinsic {
        dst: Reg,
        base: Reg,
        nargs: u8,
        b: Builtin,
        desc: Option<u16>,
    },
}

/// An instruction plus compile-time metadata that is not part of the encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub insn: Insn,
    /// The value this instruction defines is a garbage-collected reference.
    /// (Needed by stack-map computation where the type is not evident from
    /// the instruction: field loads, calls, ...)
    pub ref_result: bool,
}

impl Item {
    pub fn new(insn: Insn) -> Item {
        Item { insn, ref_result: false }
    }

    pub fn with_ref(insn: Insn, ref_result: bool) -> Item {
        Item { insn, ref_result }
    }
}

impl Insn {
    /// Calls `f` with every register this instruction reads.
    pub fn for_each_use(&self, mut f: impl FnMut(Reg)) {
        use Insn::*;
        match self {
            Label(_)
            | LoadI { .. }
            | LoadK { .. }
            | LoadStr { .. }
            | Jmp { .. }
            | Unreachable
            | GetUpval { .. }
            | GetGlobal { .. } => {}
            Move { src, .. } | Neg { src, .. } | BitNot { src, .. } | BoolNot { src, .. } | Cast { src, .. } => f(*src),
            AddImm { src, .. } | SubImm { src, .. } => f(*src),
            Arith { a, b, .. } | Bitwise { a, b, .. } | Cmp { a, b, .. } | CmpJmp { a, b, .. } => {
                f(*a);
                f(*b);
            }
            JmpIf { cond, .. } | JmpIfNot { cond, .. } => f(*cond),
            MatchTagJmp { obj, .. } => f(*obj),
            Call { base, nargs, .. } => (0..*nargs as u16).for_each(|i| f((*base as u16 + i) as Reg)),
            CallClosure { base, nargs } => (0..=*nargs as u16).for_each(|i| f((*base as u16 + i) as Reg)),
            Ret { src } => f(*src),
            NewStruct { base, n, .. } | NewEnum { base, n, .. } | NewArray { base, n, .. } | NewSet { base, n, .. } => {
                (0..*n as u16).for_each(|i| f((*base as u16 + i) as Reg))
            }
            NewMap { base, pairs, .. } => (0..*pairs as u16 * 2).for_each(|i| f((*base as u16 + i) as Reg)),
            GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => f(*obj),
            SetField { obj, src, .. } => {
                f(*obj);
                f(*src);
            }
            ArrayGet { arr, idx, .. } => {
                f(*arr);
                f(*idx);
            }
            ArraySet { arr, idx, src } => {
                f(*arr);
                f(*idx);
                f(*src);
            }
            MapGet { map, key, .. } => {
                f(*map);
                f(*key);
            }
            MapSet { map, key, val } => {
                f(*map);
                f(*key);
                f(*val);
            }
            MakeClosure { captures, .. } => captures.iter().for_each(|c| {
                if let Capture::Local(r) = c {
                    f(*r)
                }
            }),
            SetUpval { src, .. } | SetGlobal { src, .. } => f(*src),
            Close { reg } => f(*reg),
            Intrinsic { base, nargs, .. } => (0..*nargs as u16).for_each(|i| f((*base as u16 + i) as Reg)),
        }
    }

    /// The register this instruction writes, if any (calls write `base`).
    pub fn def(&self) -> Option<Reg> {
        use Insn::*;
        match self {
            Move { dst, .. }
            | LoadI { dst, .. }
            | LoadK { dst, .. }
            | LoadStr { dst, .. }
            | Arith { dst, .. }
            | Bitwise { dst, .. }
            | AddImm { dst, .. }
            | SubImm { dst, .. }
            | Neg { dst, .. }
            | BitNot { dst, .. }
            | BoolNot { dst, .. }
            | Cmp { dst, .. }
            | Cast { dst, .. }
            | NewStruct { dst, .. }
            | GetField { dst, .. }
            | NewEnum { dst, .. }
            | EnumTag { dst, .. }
            | EnumField { dst, .. }
            | NewArray { dst, .. }
            | ArrayGet { dst, .. }
            | NewMap { dst, .. }
            | MapGet { dst, .. }
            | NewSet { dst, .. }
            | MakeClosure { dst, .. }
            | GetUpval { dst, .. }
            | GetGlobal { dst, .. }
            | Intrinsic { dst, .. } => Some(*dst),
            Call { base, .. } | CallClosure { base, .. } => Some(*base),
            _ => None,
        }
    }

    /// For calls: every register at or above this one is overwritten by the callee.
    pub fn clobbers_from(&self) -> Option<u16> {
        match self {
            Insn::Call { base, .. } => Some(*base as u16),
            Insn::CallClosure { base, .. } => Some(*base as u16 + 1),
            _ => None,
        }
    }

    /// The label this instruction may jump to.
    pub fn target(&self) -> Option<Label> {
        match self {
            Insn::Jmp { to }
            | Insn::JmpIf { to, .. }
            | Insn::JmpIfNot { to, .. }
            | Insn::CmpJmp { to, .. }
            | Insn::MatchTagJmp { to, .. } => Some(*to),
            _ => None,
        }
    }

    pub fn target_mut(&mut self) -> Option<&mut Label> {
        match self {
            Insn::Jmp { to }
            | Insn::JmpIf { to, .. }
            | Insn::JmpIfNot { to, .. }
            | Insn::CmpJmp { to, .. }
            | Insn::MatchTagJmp { to, .. } => Some(to),
            _ => None,
        }
    }

    /// Whether execution can continue with the next instruction.
    pub fn falls_through(&self) -> bool {
        !matches!(self, Insn::Jmp { .. } | Insn::Ret { .. } | Insn::Unreachable)
    }

    /// Instructions that can allocate on the heap or run arbitrary code, so
    /// the collector may run there and a stack map is required.
    pub fn is_allocation_or_call(&self) -> bool {
        use Insn::*;
        matches!(
            self,
            LoadStr { .. }
                | NewStruct { .. }
                | NewEnum { .. }
                | NewArray { .. }
                | NewMap { .. }
                | NewSet { .. }
                | MakeClosure { .. }
                | Call { .. }
                | CallClosure { .. }
                | Intrinsic { .. }
        )
    }

    /// Has no effect other than writing its destination and cannot trap.
    pub fn is_pure(&self) -> bool {
        use Insn::*;
        matches!(
            self,
            Move { .. }
                | LoadI { .. }
                | LoadK { .. }
                | Bitwise { .. }
                | BoolNot { .. }
                | Cmp { .. }
                | Cast { .. }
                | GetUpval { .. }
                | GetGlobal { .. }
                | BitNot { .. }
        ) || matches!(self, Arith { ty, .. } if ty.is_float())
            || matches!(self, Neg { ty, .. } if ty.is_float())
    }

    /// Replaces the destination register (for instructions with exactly one).
    pub fn set_def(&mut self, new: Reg) -> bool {
        use Insn::*;
        match self {
            Move { dst, .. }
            | LoadI { dst, .. }
            | LoadK { dst, .. }
            | LoadStr { dst, .. }
            | Arith { dst, .. }
            | Bitwise { dst, .. }
            | AddImm { dst, .. }
            | SubImm { dst, .. }
            | Neg { dst, .. }
            | BitNot { dst, .. }
            | BoolNot { dst, .. }
            | Cmp { dst, .. }
            | Cast { dst, .. }
            | GetField { dst, .. }
            | EnumTag { dst, .. }
            | EnumField { dst, .. }
            | ArrayGet { dst, .. }
            | MapGet { dst, .. }
            | GetUpval { dst, .. }
            | GetGlobal { dst, .. }
            | NewStruct { dst, .. }
            | NewEnum { dst, .. }
            | NewArray { dst, .. }
            | NewMap { dst, .. }
            | NewSet { dst, .. }
            | MakeClosure { dst, .. }
            | Intrinsic { dst, .. } => {
                *dst = new;
                true
            }
            _ => false,
        }
    }

    /// Replaces every register this instruction reads through `f`.
    pub fn map_uses(&mut self, mut f: impl FnMut(Reg) -> Reg) {
        use Insn::*;
        match self {
            Move { src, .. }
            | Neg { src, .. }
            | BitNot { src, .. }
            | BoolNot { src, .. }
            | Cast { src, .. }
            | AddImm { src, .. }
            | SubImm { src, .. } => *src = f(*src),
            Arith { a, b, .. } | Bitwise { a, b, .. } | Cmp { a, b, .. } | CmpJmp { a, b, .. } => {
                *a = f(*a);
                *b = f(*b);
            }
            JmpIf { cond, .. } | JmpIfNot { cond, .. } => *cond = f(*cond),
            MatchTagJmp { obj, .. } | GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => {
                *obj = f(*obj)
            }
            Ret { src } | SetUpval { src, .. } | SetGlobal { src, .. } => *src = f(*src),
            SetField { obj, src, .. } => {
                *obj = f(*obj);
                *src = f(*src);
            }
            ArrayGet { arr, idx, .. } => {
                *arr = f(*arr);
                *idx = f(*idx);
            }
            ArraySet { arr, idx, src } => {
                *arr = f(*arr);
                *idx = f(*idx);
                *src = f(*src);
            }
            MapGet { map, key, .. } => {
                *map = f(*map);
                *key = f(*key);
            }
            MapSet { map, key, val } => {
                *map = f(*map);
                *key = f(*key);
                *val = f(*val);
            }
            Close { reg } => *reg = f(*reg),
            // window-based instructions read fixed ranges and are never renamed
            _ => {}
        }
    }
}
