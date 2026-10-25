//! Three-address-code IR: a control-flow graph of basic blocks over an
//! unbounded set of virtual registers.
//!
//! All generics are already monomorphized: every register has a concrete
//! machine-level type ([`IrTy`]) and every call targets a concrete function.

use crate::builtin::Builtin;
use crate::runtime::{ArithOp, CmpOp, NumTy};

macro_rules! idx {
    ($($(#[$m:meta])* $name:ident),*) => {$(
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
        pub struct $name(pub u32);
        impl $name {
            pub fn index(self) -> usize { self.0 as usize }
        }
    )*};
}

idx!(
    /// A virtual register (`v0, v1, ...`). Parameters are the first registers.
    VReg,
    BlockId,
    FuncId,
    /// Index into [`IrProgram::descs`].
    DescId,
    /// Index into [`IrProgram::globals`].
    GlobalSlot
);

/// What a register holds, as far as the machine is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IrTy {
    Num(NumTy),
    Bool,
    Char,
    /// A handle to a garbage-collected object.
    Ref,
    Unit,
}

impl IrTy {
    pub fn is_ref(self) -> bool {
        self == IrTy::Ref
    }

    /// The numeric type used to compare or combine values of this type.
    pub fn as_num(self) -> Option<NumTy> {
        match self {
            IrTy::Num(n) => Some(n),
            IrTy::Bool => Some(NumTy::U8),
            IrTy::Char => Some(NumTy::U32),
            _ => None,
        }
    }
}

/// A compile-time constant, as canonical bits (see `runtime::numeric`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Const {
    /// Integers, floats, booleans and characters.
    Bits(u64),
    /// Index into [`IrProgram::strings`].
    Str(u32),
    Unit,
}

/// Operand type of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CmpTy {
    Num(NumTy),
    Str,
}

/// How a closure obtains one captured variable at creation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Capture {
    /// An open upvalue pointing at this register of the creating function.
    Local(VReg),
    /// Share the creating closure's own upvalue.
    Upvalue(u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    Const {
        dst: VReg,
        val: Const,
    },
    Move {
        dst: VReg,
        src: VReg,
    },
    Arith {
        dst: VReg,
        op: ArithOp,
        ty: NumTy,
        lhs: VReg,
        rhs: VReg,
    },
    Cmp {
        dst: VReg,
        op: CmpOp,
        ty: CmpTy,
        lhs: VReg,
        rhs: VReg,
    },
    Neg {
        dst: VReg,
        ty: NumTy,
        src: VReg,
    },
    BitNot {
        dst: VReg,
        ty: NumTy,
        src: VReg,
    },
    BoolNot {
        dst: VReg,
        src: VReg,
    },
    Cast {
        dst: VReg,
        from: NumTy,
        to: NumTy,
        src: VReg,
    },

    NewStruct {
        dst: VReg,
        desc: DescId,
        fields: Vec<VReg>,
    },
    GetField {
        dst: VReg,
        obj: VReg,
        idx: u32,
    },
    SetField {
        obj: VReg,
        idx: u32,
        src: VReg,
    },
    NewEnum {
        dst: VReg,
        desc: DescId,
        tag: u32,
        fields: Vec<VReg>,
    },
    /// The variant number of an enum value.
    EnumTag {
        dst: VReg,
        obj: VReg,
    },
    /// Payload slot `idx` of an enum value (the variant is known statically).
    EnumField {
        dst: VReg,
        obj: VReg,
        idx: u32,
    },
    NewArray {
        dst: VReg,
        desc: DescId,
        elems: Vec<VReg>,
    },
    /// Bounds-checked; `idx_ty` is the integer type of the index register.
    ArrayGet {
        dst: VReg,
        arr: VReg,
        idx: VReg,
        idx_ty: NumTy,
    },
    ArraySet {
        arr: VReg,
        idx: VReg,
        idx_ty: NumTy,
        src: VReg,
    },
    NewMap {
        dst: VReg,
        desc: DescId,
        entries: Vec<(VReg, VReg)>,
    },
    /// Traps if the key is absent.
    MapGet {
        dst: VReg,
        map: VReg,
        key: VReg,
    },
    MapSet {
        map: VReg,
        key: VReg,
        val: VReg,
    },
    NewSet {
        dst: VReg,
        desc: DescId,
        elems: Vec<VReg>,
    },

    Call {
        dst: VReg,
        func: FuncId,
        args: Vec<VReg>,
    },
    CallClosure {
        dst: VReg,
        callee: VReg,
        args: Vec<VReg>,
    },
    MakeClosure {
        dst: VReg,
        func: FuncId,
        captures: Vec<Capture>,
    },
    GetUpval {
        dst: VReg,
        idx: u32,
    },
    SetUpval {
        idx: u32,
        src: VReg,
    },
    /// Ends the lifetime of these captured registers: open upvalues pointing
    /// at them are closed (their values move to the heap).
    CloseUpvals {
        regs: Vec<VReg>,
    },
    GetGlobal {
        dst: VReg,
        slot: GlobalSlot,
    },
    SetGlobal {
        slot: GlobalSlot,
        src: VReg,
    },
    /// A builtin; `desc` describes the receiver / first argument type.
    Intrinsic {
        dst: VReg,
        b: Builtin,
        desc: Option<DescId>,
        args: Vec<VReg>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    Jump(BlockId),
    Branch {
        cond: VReg,
        then: BlockId,
        els: BlockId,
    },
    /// Jump to `then` if the enum value's variant is `tag`, else to `els`.
    BranchTag {
        obj: VReg,
        tag: u32,
        then: BlockId,
        els: BlockId,
    },
    Return(VReg),
    /// Control cannot reach here (after a diverging call or a covered `match`).
    Unreachable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub insts: Vec<Inst>,
    pub term: Term,
}

#[derive(Debug, Clone)]
pub struct IrFunc {
    pub name: String,
    /// The first `params` registers receive the arguments.
    pub params: u32,
    pub ret: IrTy,
    /// Type of every virtual register.
    pub regs: Vec<IrTy>,
    /// Registers captured by a closure: they need a stable frame slot until closed.
    pub pinned: Vec<bool>,
    /// Types of the upvalues this closure reads (empty for plain functions).
    pub upvals: Vec<IrTy>,
    pub is_closure: bool,
    /// Block 0 is the entry.
    pub blocks: Vec<Block>,
}

impl IrFunc {
    pub fn new(name: impl Into<String>) -> IrFunc {
        IrFunc {
            name: name.into(),
            params: 0,
            ret: IrTy::Unit,
            regs: Vec::new(),
            pinned: Vec::new(),
            upvals: Vec::new(),
            is_closure: false,
            blocks: Vec::new(),
        }
    }

    pub fn new_reg(&mut self, ty: IrTy) -> VReg {
        self.regs.push(ty);
        self.pinned.push(false);
        VReg(self.regs.len() as u32 - 1)
    }

    pub fn new_block(&mut self) -> BlockId {
        self.blocks.push(Block { insts: Vec::new(), term: Term::Unreachable });
        BlockId(self.blocks.len() as u32 - 1)
    }

    pub fn reg_ty(&self, r: VReg) -> IrTy {
        self.regs[r.index()]
    }

    /// Total instruction count (terminators included).
    pub fn size(&self) -> usize {
        self.blocks.iter().map(|b| b.insts.len() + 1).sum()
    }
}

/// Run-time type information used for allocation layout, GC tracing and
/// printing.
#[derive(Debug, Clone, PartialEq)]
pub enum TypeDesc {
    Unit,
    Num(NumTy),
    Bool,
    Char,
    Str,
    Array(DescId),
    Set(DescId),
    Map(DescId, DescId),
    Struct { name: String, fields: Vec<(String, DescId)> },
    Enum { name: String, variants: Vec<(String, Vec<DescId>)> },
    Fn,
}

impl TypeDesc {
    pub fn is_ref(&self) -> bool {
        !matches!(self, TypeDesc::Unit | TypeDesc::Num(_) | TypeDesc::Bool | TypeDesc::Char)
    }
}

#[derive(Debug, Clone)]
pub struct GlobalInfo {
    pub name: String,
    pub ty: IrTy,
    pub desc: DescId,
}

#[derive(Debug, Default, Clone)]
pub struct IrProgram {
    pub funcs: Vec<IrFunc>,
    pub strings: Vec<String>,
    pub descs: Vec<TypeDesc>,
    pub globals: Vec<GlobalInfo>,
    /// Runs every module's top-level code, then `main` if defined.
    pub entry: FuncId,
}

impl IrProgram {
    pub fn func(&self, id: FuncId) -> &IrFunc {
        &self.funcs[id.index()]
    }

    pub fn desc(&self, id: DescId) -> &TypeDesc {
        &self.descs[id.index()]
    }

    pub fn desc_ir_ty(&self, id: DescId) -> IrTy {
        match self.desc(id) {
            TypeDesc::Unit => IrTy::Unit,
            TypeDesc::Num(n) => IrTy::Num(*n),
            TypeDesc::Bool => IrTy::Bool,
            TypeDesc::Char => IrTy::Char,
            _ => IrTy::Ref,
        }
    }
}

impl Inst {
    /// The register this instruction defines, if any.
    pub fn def(&self) -> Option<VReg> {
        use Inst::*;
        match self {
            Const { dst, .. }
            | Move { dst, .. }
            | Arith { dst, .. }
            | Cmp { dst, .. }
            | Neg { dst, .. }
            | BitNot { dst, .. }
            | BoolNot { dst, .. }
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
            | Call { dst, .. }
            | CallClosure { dst, .. }
            | MakeClosure { dst, .. }
            | GetUpval { dst, .. }
            | GetGlobal { dst, .. }
            | Intrinsic { dst, .. } => Some(*dst),
            SetField { .. }
            | ArraySet { .. }
            | MapSet { .. }
            | SetUpval { .. }
            | CloseUpvals { .. }
            | SetGlobal { .. } => None,
        }
    }

    /// Calls `f` on every register this instruction reads.
    pub fn for_each_use(&self, mut f: impl FnMut(VReg)) {
        use Inst::*;
        match self {
            Const { .. } | GetUpval { .. } | GetGlobal { .. } => {}
            Move { src, .. } | Neg { src, .. } | BitNot { src, .. } | BoolNot { src, .. } | Cast { src, .. } => f(*src),
            Arith { lhs, rhs, .. } | Cmp { lhs, rhs, .. } => {
                f(*lhs);
                f(*rhs);
            }
            NewStruct { fields, .. } | NewEnum { fields, .. } => fields.iter().for_each(|&r| f(r)),
            GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => f(*obj),
            SetField { obj, src, .. } => {
                f(*obj);
                f(*src);
            }
            NewArray { elems, .. } | NewSet { elems, .. } => elems.iter().for_each(|&r| f(r)),
            ArrayGet { arr, idx, .. } => {
                f(*arr);
                f(*idx);
            }
            ArraySet { arr, idx, src, .. } => {
                f(*arr);
                f(*idx);
                f(*src);
            }
            NewMap { entries, .. } => entries.iter().for_each(|&(k, v)| {
                f(k);
                f(v);
            }),
            MapGet { map, key, .. } => {
                f(*map);
                f(*key);
            }
            MapSet { map, key, val } => {
                f(*map);
                f(*key);
                f(*val);
            }
            Call { args, .. } | Intrinsic { args, .. } => args.iter().for_each(|&r| f(r)),
            CallClosure { callee, args, .. } => {
                f(*callee);
                args.iter().for_each(|&r| f(r));
            }
            MakeClosure { captures, .. } => captures.iter().for_each(|c| {
                if let Capture::Local(r) = c {
                    f(*r)
                }
            }),
            SetUpval { src, .. } | SetGlobal { src, .. } => f(*src),
            CloseUpvals { regs } => regs.iter().for_each(|&r| f(r)),
        }
    }

    /// Mutable access to every register read, for renaming.
    pub fn for_each_use_mut(&mut self, mut f: impl FnMut(&mut VReg)) {
        use Inst::*;
        match self {
            Const { .. } | GetUpval { .. } | GetGlobal { .. } => {}
            Move { src, .. } | Neg { src, .. } | BitNot { src, .. } | BoolNot { src, .. } | Cast { src, .. } => f(src),
            Arith { lhs, rhs, .. } | Cmp { lhs, rhs, .. } => {
                f(lhs);
                f(rhs);
            }
            NewStruct { fields, .. } | NewEnum { fields, .. } => fields.iter_mut().for_each(&mut f),
            GetField { obj, .. } | EnumTag { obj, .. } | EnumField { obj, .. } => f(obj),
            SetField { obj, src, .. } => {
                f(obj);
                f(src);
            }
            NewArray { elems, .. } | NewSet { elems, .. } => elems.iter_mut().for_each(&mut f),
            ArrayGet { arr, idx, .. } => {
                f(arr);
                f(idx);
            }
            ArraySet { arr, idx, src, .. } => {
                f(arr);
                f(idx);
                f(src);
            }
            NewMap { entries, .. } => entries.iter_mut().for_each(|(k, v)| {
                f(k);
                f(v);
            }),
            MapGet { map, key, .. } => {
                f(map);
                f(key);
            }
            MapSet { map, key, val } => {
                f(map);
                f(key);
                f(val);
            }
            Call { args, .. } | Intrinsic { args, .. } => args.iter_mut().for_each(&mut f),
            CallClosure { callee, args, .. } => {
                f(callee);
                args.iter_mut().for_each(&mut f);
            }
            MakeClosure { captures, .. } => captures.iter_mut().for_each(|c| {
                if let Capture::Local(r) = c {
                    f(r)
                }
            }),
            SetUpval { src, .. } | SetGlobal { src, .. } => f(src),
            CloseUpvals { regs } => regs.iter_mut().for_each(&mut f),
        }
    }

    /// Mutable access to the defined register.
    pub fn def_mut(&mut self) -> Option<&mut VReg> {
        use Inst::*;
        match self {
            Const { dst, .. }
            | Move { dst, .. }
            | Arith { dst, .. }
            | Cmp { dst, .. }
            | Neg { dst, .. }
            | BitNot { dst, .. }
            | BoolNot { dst, .. }
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
            | Call { dst, .. }
            | CallClosure { dst, .. }
            | MakeClosure { dst, .. }
            | GetUpval { dst, .. }
            | GetGlobal { dst, .. }
            | Intrinsic { dst, .. } => Some(dst),
            _ => None,
        }
    }

    /// Whether removing this instruction (when its result is unused) could
    /// change behavior: it writes memory, calls out, allocates observably or
    /// may trap.
    pub fn has_side_effects(&self) -> bool {
        use Inst::*;
        match self {
            Const { .. }
            | Move { .. }
            | Cmp { .. }
            | BoolNot { .. }
            | BitNot { .. }
            | Cast { .. }
            | GetUpval { .. }
            | GetGlobal { .. } => false,
            Arith { op, ty, .. } => ty.is_int() && !matches!(op, ArithOp::And | ArithOp::Or | ArithOp::Xor),
            Neg { ty, .. } => ty.is_int(),
            // allocation is unobservable once its result is unused
            NewStruct { .. }
            | NewEnum { .. }
            | NewArray { .. }
            | NewMap { .. }
            | NewSet { .. }
            | MakeClosure { .. } => false,
            GetField { .. } | EnumTag { .. } | EnumField { .. } => false,
            _ => true,
        }
    }
}

impl Term {
    pub fn for_each_use(&self, mut f: impl FnMut(VReg)) {
        match self {
            Term::Branch { cond, .. } => f(*cond),
            Term::BranchTag { obj, .. } => f(*obj),
            Term::Return(r) => f(*r),
            Term::Jump(_) | Term::Unreachable => {}
        }
    }

    pub fn successors(&self) -> Vec<BlockId> {
        match self {
            Term::Jump(b) => vec![*b],
            Term::Branch { then, els, .. } | Term::BranchTag { then, els, .. } => vec![*then, *els],
            Term::Return(_) | Term::Unreachable => vec![],
        }
    }

    pub fn successors_mut(&mut self) -> Vec<&mut BlockId> {
        match self {
            Term::Jump(b) => vec![b],
            Term::Branch { then, els, .. } | Term::BranchTag { then, els, .. } => vec![then, els],
            Term::Return(_) | Term::Unreachable => vec![],
        }
    }
}
