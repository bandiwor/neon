use super::interner::Symbol;
use super::literal::{FloatId, IntId, StrId};
use crate::diag::Span;

macro_rules! keywords {
    ($($variant:ident = $text:expr,)*) => {
        /// Reserved words and primitive type names.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Keyword { $($variant,)* }

        impl Keyword {
            /// Every keyword, in the order its symbol is pre-interned.
            pub const ALL: &'static [Keyword] = &[$(Keyword::$variant,)*];

            pub fn as_str(self) -> &'static str {
                match self { $(Keyword::$variant => $text,)* }
            }
        }
    };
}

keywords! {
    Let = "let",
    Mut = "mut",
    Const = "const",
    Fn = "fn",
    Return = "return",
    If = "if",
    Else = "else",
    While = "while",
    Loop = "loop",
    For = "for",
    In = "in",
    Break = "break",
    Continue = "continue",
    Match = "match",
    Enum = "enum",
    Struct = "struct",
    Trait = "trait",
    Impl = "impl",
    Import = "import",
    Export = "export",
    As = "as",
    True = "true",
    False = "false",
    SelfValue = "self",
    SelfType = "Self",
    I8 = "i8",
    I16 = "i16",
    I32 = "i32",
    I64 = "i64",
    U8 = "u8",
    U16 = "u16",
    U32 = "u32",
    U64 = "u64",
    F32 = "f32",
    F64 = "f64",
    Bool = "bool",
    Char = "char",
    String = "string",
}

impl Keyword {
    /// The keyword a symbol denotes, relying on keywords being the first
    /// symbols of every [`Interner`](super::Interner).
    pub fn from_symbol(sym: Symbol) -> Option<Keyword> {
        Keyword::ALL.get(sym.0 as usize).copied()
    }

    /// Whether this is a primitive type name (`i32`, `bool`, `string`, ...).
    pub fn is_primitive_type(self) -> bool {
        use Keyword::*;
        matches!(self, I8 | I16 | I32 | I64 | U8 | U16 | U32 | U64 | F32 | F64 | Bool | Char | String)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Ident(Symbol),
    Kw(Keyword),
    /// Decoded values live in the [`LitPool`](super::LitPool).
    Int(IntId),
    Float(FloatId),
    Str(StrId),
    Char(char),
    /// A lone `_` (wildcard pattern); `_x` is an ordinary identifier.
    Underscore,

    LParen,
    RParen,
    LBrace,
    RBrace,
    LBracket,
    RBracket,
    Comma,
    Semi,
    Colon,
    ColonColon,
    Dot,
    DotDot,
    DotDotEq,
    Arrow,
    FatArrow,

    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Amp,
    Pipe,
    Caret,
    Tilde,
    Shl,
    Shr,
    Bang,
    AmpAmp,
    PipePipe,
    EqEq,
    BangEq,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    PlusEq,
    MinusEq,
    StarEq,
    SlashEq,
    PercentEq,
    AmpEq,
    PipeEq,
    CaretEq,
    ShlEq,
    ShrEq,

    /// Input the lexer could not make sense of (a diagnostic was reported).
    Error,
    Eof,
}

impl TokenKind {
    /// How the token is named in "expected X, found Y" messages.
    pub fn describe(self) -> &'static str {
        use TokenKind::*;
        match self {
            Ident(_) => "identifier",
            Kw(k) => k.as_str(),
            Int(_) => "integer literal",
            Float(_) => "float literal",
            Str(_) => "string literal",
            Char(_) => "character literal",
            Underscore => "`_`",
            LParen => "`(`",
            RParen => "`)`",
            LBrace => "`{`",
            RBrace => "`}`",
            LBracket => "`[`",
            RBracket => "`]`",
            Comma => "`,`",
            Semi => "`;`",
            Colon => "`:`",
            ColonColon => "`::`",
            Dot => "`.`",
            DotDot => "`..`",
            DotDotEq => "`..=`",
            Arrow => "`->`",
            FatArrow => "`=>`",
            Plus => "`+`",
            Minus => "`-`",
            Star => "`*`",
            Slash => "`/`",
            Percent => "`%`",
            Amp => "`&`",
            Pipe => "`|`",
            Caret => "`^`",
            Tilde => "`~`",
            Shl => "`<<`",
            Shr => "`>>`",
            Bang => "`!`",
            AmpAmp => "`&&`",
            PipePipe => "`||`",
            EqEq => "`==`",
            BangEq => "`!=`",
            Lt => "`<`",
            Le => "`<=`",
            Gt => "`>`",
            Ge => "`>=`",
            Eq => "`=`",
            PlusEq => "`+=`",
            MinusEq => "`-=`",
            StarEq => "`*=`",
            SlashEq => "`/=`",
            PercentEq => "`%=`",
            AmpEq => "`&=`",
            PipeEq => "`|=`",
            CaretEq => "`^=`",
            ShlEq => "`<<=`",
            ShrEq => "`>>=`",
            Error => "invalid token",
            Eof => "end of file",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyword_table_is_consistent() {
        for (i, kw) in Keyword::ALL.iter().enumerate() {
            assert_eq!(*kw as usize, i, "{kw:?} discriminant must equal its table index");
            assert!(!kw.as_str().is_empty());
        }
        let mut texts: Vec<_> = Keyword::ALL.iter().map(|k| k.as_str()).collect();
        texts.sort_unstable();
        texts.dedup();
        assert_eq!(texts.len(), Keyword::ALL.len(), "duplicate keyword text");
    }

    #[test]
    fn primitive_types() {
        assert!(Keyword::I8.is_primitive_type() && Keyword::String.is_primitive_type());
        assert!(!Keyword::Let.is_primitive_type() && !Keyword::SelfType.is_primitive_type());
        assert_eq!(Keyword::ALL.iter().filter(|k| k.is_primitive_type()).count(), 13);
    }

    #[test]
    fn describe_names_tokens() {
        assert_eq!(TokenKind::Semi.describe(), "`;`");
        assert_eq!(TokenKind::Kw(Keyword::Let).describe(), "let");
        assert_eq!(TokenKind::Eof.describe(), "end of file");
        assert_eq!(TokenKind::Ident(Symbol(0)).describe(), "identifier");
    }
}
