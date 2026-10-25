//! Decoded literal values, kept out of tokens and AST nodes.

macro_rules! id_type {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);
    };
}

id_type!(
    /// Index of an integer literal in a [`LitPool`].
    IntId
);
id_type!(
    /// Index of a float literal in a [`LitPool`].
    FloatId
);
id_type!(
    /// Index of a string literal in a [`LitPool`].
    StrId
);

macro_rules! suffixes {
    ($name:ident { $($variant:ident = $text:expr,)* }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant,)* }

        impl $name {
            pub fn parse(s: &str) -> Option<$name> {
                match s { $($text => Some($name::$variant),)* _ => None }
            }

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text,)* }
            }
        }
    };
}

suffixes!(IntSuffix {
    I8 = "i8",
    I16 = "i16",
    I32 = "i32",
    I64 = "i64",
    U8 = "u8",
    U16 = "u16",
    U32 = "u32",
    U64 = "u64",
});

suffixes!(FloatSuffix {
    F32 = "f32",
    F64 = "f64",
});

impl IntSuffix {
    pub fn is_signed(self) -> bool {
        matches!(self, IntSuffix::I8 | IntSuffix::I16 | IntSuffix::I32 | IntSuffix::I64)
    }

    pub fn bits(self) -> u32 {
        match self {
            IntSuffix::I8 | IntSuffix::U8 => 8,
            IntSuffix::I16 | IntSuffix::U16 => 16,
            IntSuffix::I32 | IntSuffix::U32 => 32,
            IntSuffix::I64 | IntSuffix::U64 => 64,
        }
    }
}

/// An integer literal as written: a non-negative magnitude (a leading `-` is
/// a separate unary operator) with an optional type suffix. Range checking
/// against the final type happens in semantic analysis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntLit {
    pub value: u64,
    pub suffix: Option<IntSuffix>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FloatLit {
    pub value: f64,
    pub suffix: Option<FloatSuffix>,
}

/// Pools of decoded literals. Strings live back to back in one buffer
/// (a chunk), addressed by range, so a literal costs no allocation of its own.
#[derive(Debug, Default)]
pub struct LitPool {
    ints: Vec<IntLit>,
    floats: Vec<FloatLit>,
    str_buf: String,
    str_ranges: Vec<(usize, usize)>,
}

impl LitPool {
    pub fn new() -> LitPool {
        LitPool::default()
    }

    pub fn add_int(&mut self, lit: IntLit) -> IntId {
        self.ints.push(lit);
        IntId(self.ints.len() as u32 - 1)
    }

    pub fn add_float(&mut self, lit: FloatLit) -> FloatId {
        self.floats.push(lit);
        FloatId(self.floats.len() as u32 - 1)
    }

    pub fn add_str(&mut self, s: &str) -> StrId {
        let start = self.str_buf.len();
        self.str_buf.push_str(s);
        self.str_ranges.push((start, self.str_buf.len()));
        StrId(self.str_ranges.len() as u32 - 1)
    }

    pub fn int(&self, id: IntId) -> IntLit {
        self.ints[id.0 as usize]
    }

    pub fn float(&self, id: FloatId) -> FloatLit {
        self.floats[id.0 as usize]
    }

    pub fn str(&self, id: StrId) -> &str {
        let (lo, hi) = self.str_ranges[id.0 as usize];
        &self.str_buf[lo..hi]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suffix_round_trip() {
        for s in ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64"] {
            assert_eq!(IntSuffix::parse(s).unwrap().as_str(), s);
        }
        for s in ["f32", "f64"] {
            assert_eq!(FloatSuffix::parse(s).unwrap().as_str(), s);
        }
        assert_eq!(IntSuffix::parse("i128"), None);
        assert_eq!(FloatSuffix::parse("f16"), None);
    }

    #[test]
    fn suffix_properties() {
        assert!(IntSuffix::I16.is_signed() && !IntSuffix::U16.is_signed());
        assert_eq!(IntSuffix::U8.bits(), 8);
        assert_eq!(IntSuffix::I64.bits(), 64);
    }

    #[test]
    fn pool_stores_each_kind() {
        let mut p = LitPool::new();
        let i = p.add_int(IntLit { value: 7, suffix: Some(IntSuffix::U8) });
        let f = p.add_float(FloatLit { value: 1.5, suffix: None });
        let a = p.add_str("hello");
        let e = p.add_str("");
        let b = p.add_str("wörld");
        assert_eq!(p.int(i), IntLit { value: 7, suffix: Some(IntSuffix::U8) });
        assert_eq!(p.float(f).value, 1.5);
        assert_eq!((p.str(a), p.str(e), p.str(b)), ("hello", "", "wörld"));
        assert_eq!((a, e, b), (StrId(0), StrId(1), StrId(2)));
    }
}
