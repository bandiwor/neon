//! Intrinsic operations known to the type checker and implemented by the VM.
//!
//! Sema decides *which* builtin a call refers to (and records the concrete
//! type arguments); the IR specializes them by type and the VM executes them.

use crate::effects::{Effects, Purity};

macro_rules! builtins {
    ($($variant:ident = $name:expr,)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Builtin { $($variant,)* }

        impl Builtin {
            pub const ALL: &'static [Builtin] = &[$(Builtin::$variant,)*];

            /// Source-level name (method or function name).
            pub fn name(self) -> &'static str {
                match self { $(Builtin::$variant => $name,)* }
            }
        }
    };
}

builtins! {
    // free functions
    Print = "print",
    Println = "println",
    Input = "input",
    Panic = "panic",
    Assert = "assert",
    // conversions (receiver is any primitive)
    ToString = "to_string",
    // `x.format(spec)` and the fields of format strings: Python-style format specifications
    Format = "format",
    // strings
    StrConcat = "concat",
    StrLen = "len",
    StrIsEmpty = "is_empty",
    StrChars = "chars",
    StrSubstring = "substring",
    StrContains = "contains",
    StrStartsWith = "starts_with",
    StrEndsWith = "ends_with",
    StrToUpper = "to_upper",
    StrToLower = "to_lower",
    StrTrim = "trim",
    StrSplit = "split",
    StrCharAt = "char_at",
    StrFind = "find",
    StrParseI64 = "parse_i64",
    StrParseU64 = "parse_u64",
    StrParseF64 = "parse_f64",
    // arrays
    ArrLen = "len",
    ArrIsEmpty = "is_empty",
    ArrPush = "push",
    ArrPop = "pop",
    ArrClear = "clear",
    ArrContains = "contains",
    ArrInsert = "insert",
    ArrRemove = "remove",
    ArrReverse = "reverse",
    ArrClone = "clone",
    ArrJoin = "join",
    ArrSort = "sort",
    // maps
    MapLen = "len",
    MapIsEmpty = "is_empty",
    MapInsert = "insert",
    MapGetOr = "get_or",
    MapContainsKey = "contains_key",
    MapRemove = "remove",
    MapKeys = "keys",
    MapValues = "values",
    MapClear = "clear",
    // sets
    SetLen = "len",
    SetIsEmpty = "is_empty",
    SetInsert = "insert",
    SetContains = "contains",
    SetRemove = "remove",
    SetToArray = "to_array",
    SetClear = "clear",
    // numbers
    Abs = "abs",
    Pow = "pow",
    Min = "min",
    Max = "max",
    Sqrt = "sqrt",
    Sin = "sin",
    Cos = "cos",
    Tan = "tan",
    Floor = "floor",
    Ceil = "ceil",
    Round = "round",
    Ln = "ln",
    Exp = "exp",
    IsNan = "is_nan",
    // chars
    CharIsDigit = "is_digit",
    CharIsAlphabetic = "is_alphabetic",
    CharIsWhitespace = "is_whitespace",
    CharToUpper = "to_upper",
    CharToLower = "to_lower",
    // fallible variants: they return `Option` / `Result` instead of trapping
    // (always appended, the position in `ALL` is part of the bytecode format)
    StrTryParseI64 = "try_parse_i64",
    StrTryParseU64 = "try_parse_u64",
    StrTryParseF64 = "try_parse_f64",
    StrGetChar = "get_char",
    StrTrySubstring = "try_substring",
    StrIndexOf = "index_of",
    ArrGet = "get",
    ArrTryPop = "try_pop",
    ArrTryRemove = "try_remove",
    MapGet = "get",
    CheckedAdd = "checked_add",
    CheckedSub = "checked_sub",
    CheckedMul = "checked_mul",
    CheckedDiv = "checked_div",
    CheckedRem = "checked_rem",
    CheckedPow = "checked_pow",
    CheckedNeg = "checked_neg",
    CheckedShl = "checked_shl",
    CheckedShr = "checked_shr",
    ReadLine = "read_line",
}

impl Builtin {
    /// What calling this builtin can do. The table is exhaustive on purpose: a new builtin does not
    /// compile until somebody has decided what it is allowed to be optimized as.
    pub fn effects(self) -> Effects {
        use Builtin::*;
        const R: Effects = Effects::READS;
        const W: Effects = Effects::WRITES;
        const T: Effects = Effects::TRAPS;
        const A: Effects = Effects::ALLOCS;
        match self {
            // formatting reads whatever object it is given
            Print | Println => Effects::IO.union(R),
            Input | ReadLine => Effects::IO.union(T).union(A),
            Panic | Assert => T,
            ToString => R.union(A),
            Format => R.union(A).union(T),
            StrConcat | StrChars | StrToUpper | StrToLower | StrTrim | StrSplit => A,
            StrLen | StrIsEmpty | StrContains | StrStartsWith | StrEndsWith | StrFind => Effects::NONE,
            StrSubstring => A.union(T),
            StrCharAt | StrParseI64 | StrParseU64 | StrParseF64 => T,
            ArrLen | ArrIsEmpty | ArrContains | MapLen | MapIsEmpty | MapGetOr | MapContainsKey | SetLen
            | SetIsEmpty | SetContains => R,
            ArrClone | ArrJoin | MapKeys | MapValues | SetToArray => R.union(A),
            ArrPush | ArrClear | ArrReverse | ArrSort | MapInsert | MapRemove | MapClear | SetInsert | SetRemove
            | SetClear => W,
            ArrPop | ArrInsert | ArrRemove => W.union(T),
            Abs | Pow => T,
            Min | Max | Sqrt | Sin | Cos | Tan | Floor | Ceil | Round | Ln | Exp | IsNan => Effects::NONE,
            CharIsDigit | CharIsAlphabetic | CharIsWhitespace | CharToUpper | CharToLower => Effects::NONE,
            StrTryParseI64 | StrTryParseU64 | StrTryParseF64 | StrGetChar | StrTrySubstring | StrIndexOf => A,
            ArrGet | MapGet => R.union(A),
            ArrTryPop | ArrTryRemove => W.union(A),
            CheckedAdd | CheckedSub | CheckedMul | CheckedDiv | CheckedRem | CheckedPow | CheckedNeg | CheckedShl
            | CheckedShr => A,
        }
    }

    /// Whether the object passed as argument `i` may outlive the call: stored in the heap or handed
    /// back in the result. Everything not listed only looks at its arguments. Exhaustive on purpose.
    pub fn retains_arg(self, i: usize) -> bool {
        use Builtin::*;
        match self {
            // the value being added to a container
            ArrPush | SetInsert => i == 1,
            ArrInsert => i == 2,
            MapInsert => i >= 1,
            // the default may be the result
            MapGetOr => i == 2,
            Print | Println | Input | Panic | Assert | ToString | Format | StrConcat | StrLen | StrIsEmpty
            | StrChars | StrSubstring | StrContains | StrStartsWith | StrEndsWith | StrToUpper | StrToLower
            | StrTrim | StrSplit | StrCharAt | StrFind | StrParseI64 | StrParseU64 | StrParseF64 | ArrLen
            | ArrIsEmpty | ArrPop | ArrClear | ArrContains | ArrRemove | ArrReverse | ArrClone | ArrJoin | ArrSort
            | MapLen | MapIsEmpty | MapContainsKey | MapRemove | MapKeys | MapValues | MapClear | SetLen
            | SetIsEmpty | SetContains | SetRemove | SetToArray | SetClear | Abs | Pow | Min | Max | Sqrt | Sin
            | Cos | Tan | Floor | Ceil | Round | Ln | Exp | IsNan | CharIsDigit | CharIsAlphabetic
            | CharIsWhitespace | CharToUpper | CharToLower | StrTryParseI64 | StrTryParseU64 | StrTryParseF64
            | StrGetChar | StrTrySubstring | StrIndexOf | ArrGet | ArrTryPop | ArrTryRemove | MapGet | CheckedAdd
            | CheckedSub | CheckedMul | CheckedDiv | CheckedRem | CheckedPow | CheckedNeg | CheckedShl | CheckedShr
            | ReadLine => false,
        }
    }

    /// Builtins that work on the container in argument 0 alone: its length, its slots, the values put
    /// into it. On an object that nothing else can see, their reads and writes are invisible.
    pub fn is_container_op(self) -> bool {
        use Builtin::*;
        matches!(
            self,
            ArrLen
                | ArrIsEmpty
                | ArrPush
                | ArrPop
                | ArrClear
                | ArrInsert
                | ArrRemove
                | ArrReverse
                | ArrGet
                | ArrTryPop
                | ArrTryRemove
                | MapLen
                | MapIsEmpty
                | MapInsert
                | MapGetOr
                | MapContainsKey
                | MapRemove
                | MapKeys
                | MapValues
                | MapClear
                | MapGet
                | SetLen
                | SetIsEmpty
                | SetInsert
                | SetContains
                | SetRemove
                | SetToArray
                | SetClear
        )
    }

    /// const / pure / impure.
    pub fn purity(self) -> Purity {
        self.effects().purity()
    }

    /// No effects at all: removable, hoistable, reusable anywhere.
    pub fn is_pure(self) -> bool {
        self.effects() == Effects::NONE
    }

    /// Only reads heap objects (`len`, `contains`, ...): removable, and reusable while nothing writes.
    pub fn reads_memory_only(self) -> bool {
        self.effects() == Effects::READS
    }

    /// Builds fresh objects and nothing else.
    pub fn only_allocates(self) -> bool {
        self.effects() == Effects::ALLOCS
    }

    /// Writes to an existing array, map or set.
    pub fn mutates_heap(self) -> bool {
        self.effects().writes()
    }

    /// Builtins that return an `Option` / `Result`. Their descriptor operand
    /// describes the *result* type (the VM builds the enum value from it).
    pub fn returns_adt(self) -> bool {
        use Builtin::*;
        matches!(
            self,
            StrTryParseI64
                | StrTryParseU64
                | StrTryParseF64
                | StrGetChar
                | StrTrySubstring
                | StrIndexOf
                | ArrGet
                | ArrTryPop
                | ArrTryRemove
                | MapGet
                | CheckedAdd
                | CheckedSub
                | CheckedMul
                | CheckedDiv
                | CheckedRem
                | CheckedPow
                | CheckedNeg
                | CheckedShl
                | CheckedShr
                | ReadLine
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_table_is_consistent() {
        for &b in Builtin::ALL {
            let e = b.effects();
            // writing or printing is never "pure"; reading is never "const"
            if e.writes() || e.intersects(Effects::IO) {
                assert_eq!(b.purity(), Purity::Impure, "{b:?}");
            }
            if e.reads() {
                assert_ne!(b.purity(), Purity::Const, "{b:?}");
            }
            // nothing that can be dropped may be hiding a trap
            if e.removable() {
                assert!(!e.traps(), "{b:?}");
            }
            // builtins returning Option/Result build it
            if b.returns_adt() && !matches!(b, Builtin::ReadLine) {
                assert!(e.allocates(), "{b:?}");
            }
            assert!(!e.diverges(), "{b:?}");
        }
        assert_eq!(Builtin::Println.purity(), Purity::Impure);
        assert_eq!(Builtin::ArrPush.purity(), Purity::Impure);
        assert_eq!(Builtin::ArrLen.purity(), Purity::Pure);
        // printing and to_string look inside the objects they format
        assert!(Builtin::Println.effects().reads() && Builtin::Print.effects().reads());
        assert!(Builtin::ToString.effects().reads() && Builtin::ToString.effects().allocates());
        assert_eq!(Builtin::StrLen.purity(), Purity::Const);
        assert_eq!(Builtin::StrConcat.purity(), Purity::Const);
        assert!(Builtin::StrConcat.effects().allocates() && !Builtin::StrConcat.effects().traps());
        assert!(Builtin::Abs.effects().traps() && Builtin::Sqrt.is_pure());
        assert!(Builtin::ArrPop.mutates_heap() && !Builtin::ArrGet.mutates_heap());
    }

    #[test]
    fn names_are_nonempty() {
        for b in Builtin::ALL {
            assert!(!b.name().is_empty());
        }
        assert_eq!(Builtin::Println.name(), "println");
    }
}
