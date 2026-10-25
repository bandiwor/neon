//! Intrinsic operations known to the type checker and implemented by the VM.
//!
//! Sema decides *which* builtin a call refers to (and records the concrete
//! type arguments); the IR specializes them by type and the VM executes them.

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
    // strings
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_nonempty() {
        for b in Builtin::ALL {
            assert!(!b.name().is_empty());
        }
        assert_eq!(Builtin::Println.name(), "println");
    }
}
