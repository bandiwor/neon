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
    /// An enum that fits one word (see [`PackedEnum`]). While the optimizer runs it is treated like an
    /// object (it is built by `NewEnum`, so private ones are scalarized and tag tests on known variants
    /// disappear); the final `legalize` pass turns its operations into arithmetic and the type into
    /// `Num(U64)`, so the backends never see it.
    Packed,
}

impl IrTy {
    /// A handle to a heap object (the only type the garbage collector looks at).
    pub fn is_ref(self) -> bool {
        self == IrTy::Ref
    }

    /// An object as far as the optimizer is concerned: a heap object or a packed enum, both built by
    /// `New*` instructions and inspected by tag / field instructions.
    pub fn is_obj(self) -> bool {
        matches!(self, IrTy::Ref | IrTy::Packed)
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
    /// Payload slot `idx` of variant `tag` of an enum value of type `desc` (the variant is known statically).
    EnumField {
        dst: VReg,
        obj: VReg,
        idx: u32,
        desc: DescId,
        tag: u32,
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

/// How one field sits inside the word of a [packed enum](PackedEnum).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackedField {
    /// Bit position of the field's lowest bit.
    pub shift: u32,
    pub bits: u32,
    pub kind: PackedKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedKind {
    Int { signed: bool },
    Bool,
    Char,
    Unit,
}

/// An enum small enough to live in one 64-bit word instead of a heap object: the variant number in the
/// lowest byte, the payload of the variant in the other seven bytes, each field at an offset that is a
/// multiple of its size. Enums are immutable, so a copy of the word is exactly as good as a shared
/// object; it is created, matched, passed, stored and printed like any enum, it just never allocates and
/// the garbage collector never has to look at it. (`Option<u8>`, `(u8, u16)`, `Result<char, bool>`, an
/// enum without payloads, ...)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackedEnum {
    /// Per variant, where its fields are.
    pub variants: Vec<Vec<PackedField>>,
}

/// The packed layout of the enum described by `d`, if all its payloads fit (see [`PackedEnum`]).
pub fn packed_enum(descs: &[TypeDesc], d: DescId) -> Option<PackedEnum> {
    let TypeDesc::Enum { variants, .. } = &descs[d.index()] else { return None };
    if variants.is_empty() || variants.len() > 256 {
        return None;
    }
    let mut out = Vec::with_capacity(variants.len());
    for (_, fields) in variants {
        // size of each field in bytes, and what it is
        let mut items: Vec<(usize, u32, PackedKind)> = Vec::new();
        for (i, f) in fields.iter().enumerate() {
            let (bytes, kind) = match &descs[f.index()] {
                TypeDesc::Bool => (1, PackedKind::Bool),
                TypeDesc::Char => (4, PackedKind::Char),
                TypeDesc::Unit => (0, PackedKind::Unit),
                TypeDesc::Num(n) if n.is_int() && n.bits() <= 32 => {
                    (n.bits() / 8, PackedKind::Int { signed: n.is_signed() })
                }
                _ => return None,
            };
            items.push((i, bytes, kind));
        }
        // first fit, biggest fields first, in the bytes after the tag; a field sits at a multiple of its size
        let mut order: Vec<usize> = (0..items.len()).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(items[i].1));
        let mut taken = [false; 8];
        taken[0] = true;
        let mut placed: Vec<Option<PackedField>> = vec![None; items.len()];
        for i in order {
            let (_, bytes, kind) = items[i];
            if bytes == 0 {
                placed[i] = Some(PackedField { shift: 0, bits: 0, kind });
                continue;
            }
            let bytes = bytes as usize;
            let slot = (1..=8 - bytes).find(|&at| at % bytes == 0 && taken[at..at + bytes].iter().all(|t| !t))?;
            taken[slot..slot + bytes].iter_mut().for_each(|t| *t = true);
            placed[i] = Some(PackedField { shift: slot as u32 * 8, bits: bytes as u32 * 8, kind });
        }
        out.push(placed.into_iter().map(|p| p.expect("every field placed")).collect());
    }
    Some(PackedEnum { variants: out })
}

/// Whether values of the type described by `d` are references (packed enums are plain words).
pub fn desc_is_ref(descs: &[TypeDesc], d: DescId) -> bool {
    descs[d.index()].is_ref() && packed_enum(descs, d).is_none()
}

impl PackedEnum {
    /// The word for variant `tag` with the given fields (canonical register values).
    pub fn pack(&self, tag: u32, fields: &[u64]) -> u64 {
        let mut w = tag as u64;
        for (f, &v) in self.variants[tag as usize].iter().zip(fields) {
            if f.bits > 0 {
                let mask = if f.bits == 64 { u64::MAX } else { (1u64 << f.bits) - 1 };
                w |= (v & mask) << f.shift;
            }
        }
        w
    }

    pub fn tag(word: u64) -> u32 {
        (word & 0xFF) as u32
    }

    /// Field `idx` of a word of variant `tag`, as a canonical register value.
    pub fn field(&self, word: u64, tag: u32, idx: usize) -> u64 {
        let f = self.variants[tag as usize][idx];
        if f.bits == 0 {
            return 0;
        }
        let raw = (word >> f.shift) & ((1u64 << f.bits) - 1);
        match f.kind {
            PackedKind::Int { signed: true } => {
                let sh = 64 - f.bits;
                (((raw << sh) as i64) >> sh) as u64
            }
            _ => raw,
        }
    }
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
            _ if packed_enum(&self.descs, id).is_some() => IrTy::Packed,
            _ => IrTy::Ref,
        }
    }

    /// The packed layout of the enum `id`, if it has one.
    pub fn packed(&self, id: DescId) -> Option<PackedEnum> {
        packed_enum(&self.descs, id)
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
        !self.removable(&super::effects::FuncEffects::unknown())
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

    /// The successors without allocating: up to two blocks and how many are valid.
    pub fn successor_slots(&self) -> ([BlockId; 2], usize) {
        match self {
            Term::Jump(b) => ([*b, *b], 1),
            Term::Branch { then, els, .. } | Term::BranchTag { then, els, .. } => ([*then, *els], 2),
            Term::Return(_) | Term::Unreachable => ([BlockId(0), BlockId(0)], 0),
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

#[cfg(test)]
mod packed_tests {
    use super::*;

    fn enum_desc(variants: Vec<(&str, Vec<TypeDesc>)>) -> (Vec<TypeDesc>, DescId) {
        let mut descs = Vec::new();
        let mut vs = Vec::new();
        for (name, fields) in variants {
            let mut ids = Vec::new();
            for f in fields {
                descs.push(f);
                ids.push(DescId(descs.len() as u32 - 1));
            }
            vs.push((name.to_string(), ids));
        }
        descs.push(TypeDesc::Enum { name: "E".into(), variants: vs });
        let id = DescId(descs.len() as u32 - 1);
        (descs, id)
    }

    fn num(n: NumTy) -> TypeDesc {
        TypeDesc::Num(n)
    }

    #[test]
    fn fields_are_aligned_and_never_overlap_the_tag() {
        let (d, id) = enum_desc(vec![
            ("A", vec![num(NumTy::U8), num(NumTy::I16)]),
            ("B", vec![num(NumTy::U32), num(NumTy::U16), num(NumTy::U8)]),
            ("C", vec![TypeDesc::Bool, TypeDesc::Char]),
            ("D", vec![]),
        ]);
        let pk = packed_enum(&d, id).expect("fits");
        for fields in &pk.variants {
            let mut used = 0u64;
            for f in fields.iter().filter(|f| f.bits > 0) {
                let bytes = f.bits / 8;
                assert_eq!(f.shift / 8 % bytes, 0, "{f:?} is aligned to its size");
                assert!(f.shift >= 8 && f.shift + f.bits <= 64, "{f:?} fits next to the tag");
                let mask = ((1u64 << f.bits) - 1) << f.shift;
                assert_eq!(used & mask, 0, "{f:?} overlaps");
                used |= mask;
            }
        }
    }

    #[test]
    fn values_round_trip() {
        let (d, id) = enum_desc(vec![
            ("A", vec![num(NumTy::I8), num(NumTy::I16), num(NumTy::I32)]),
            ("B", vec![TypeDesc::Bool, TypeDesc::Char]),
            ("C", vec![num(NumTy::U32), num(NumTy::U16), num(NumTy::U8)]),
        ]);
        let pk = packed_enum(&d, id).expect("fits");
        let ints = [-128i64, -1, 0, 1, 127];
        for &a in &ints {
            for &b in &[-32768i64, -2, 0, 5, 32767] {
                for &c in &[i32::MIN as i64, -1, 0, 1, i32::MAX as i64] {
                    let w = pk.pack(0, &[a as u64, b as u64, c as u64]);
                    assert_eq!(PackedEnum::tag(w), 0);
                    assert_eq!(pk.field(w, 0, 0) as i64, a);
                    assert_eq!(pk.field(w, 0, 1) as i64, b);
                    assert_eq!(pk.field(w, 0, 2) as i64, c);
                }
            }
        }
        let w = pk.pack(1, &[1, 0x10FFFF]);
        assert_eq!((PackedEnum::tag(w), pk.field(w, 1, 0), pk.field(w, 1, 1)), (1, 1, 0x10FFFF));
        let w = pk.pack(2, &[u32::MAX as u64, u16::MAX as u64, 255]);
        assert_eq!((pk.field(w, 2, 0), pk.field(w, 2, 1), pk.field(w, 2, 2)), (u32::MAX as u64, u16::MAX as u64, 255));
    }

    #[test]
    fn what_does_not_fit_stays_an_object() {
        // 64-bit numbers, floats, strings and nested enums are not packed
        for field in [num(NumTy::I64), num(NumTy::F32), num(NumTy::F64), TypeDesc::Str, TypeDesc::Fn] {
            let (d, id) = enum_desc(vec![("A", vec![field])]);
            assert!(packed_enum(&d, id).is_none());
            assert!(desc_is_ref(&d, id));
        }
        // 4 + 4 bytes do not fit in 7
        let (d, id) = enum_desc(vec![("A", vec![num(NumTy::U32), num(NumTy::U32)])]);
        assert!(packed_enum(&d, id).is_none());
        // 4 + 2 + 1 do
        let (d, id) = enum_desc(vec![("A", vec![num(NumTy::U32), num(NumTy::U16), num(NumTy::U8)])]);
        assert!(packed_enum(&d, id).is_some());
        assert!(!desc_is_ref(&d, id));
    }
}
