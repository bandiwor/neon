use crate::fx::FxHashMap as HashMap;
use std::sync::Arc;

use super::token::Keyword;

/// An interned identifier. Comparing symbols is comparing integers.
///
/// The first [`Keyword::ALL`]`.len()` symbols of every [`Interner`] are the
/// keywords, in declaration order, so keyword recognition is a range check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Symbol(pub u32);

/// Symbols that every interner pre-interns right after the keywords, for
/// contextual keywords (ordinary identifiers that the parser treats
/// specially in one position).
pub mod sym {
    use super::Symbol;
    use crate::syntax::token::Keyword;

    pub(super) const PREDEFINED: &[&str] = &["from", "set"];

    const BASE: u32 = Keyword::ALL.len() as u32;

    /// `from` in `import { a } from "path";`
    pub const FROM: Symbol = Symbol(BASE);
    /// `set` in `set { 1, 2 }` and `set<T>`; still usable as a method name.
    pub const SET: Symbol = Symbol(BASE + 1);
}

/// Deduplicating string table.
#[derive(Debug)]
pub struct Interner {
    strings: Vec<Arc<str>>,
    map: HashMap<Arc<str>, Symbol>,
}

impl Default for Interner {
    fn default() -> Self {
        Interner::new()
    }
}

impl Interner {
    /// An interner with keywords and predefined symbols already registered.
    pub fn new() -> Interner {
        let mut i = Interner { strings: Vec::new(), map: HashMap::default() };
        for kw in Keyword::ALL {
            i.intern(kw.as_str());
        }
        for s in sym::PREDEFINED {
            i.intern(s);
        }
        i
    }

    pub fn intern(&mut self, s: &str) -> Symbol {
        if let Some(&sym) = self.map.get(s) {
            return sym;
        }
        let sym = Symbol(self.strings.len() as u32);
        let shared: Arc<str> = Arc::from(s);
        self.strings.push(Arc::clone(&shared));
        self.map.insert(shared, sym);
        sym
    }

    /// The symbol for `s` if it was interned before; never inserts.
    pub fn get(&self, s: &str) -> Option<Symbol> {
        self.map.get(s).copied()
    }

    /// The text of a symbol. Panics on a symbol from another interner.
    pub fn resolve(&self, sym: Symbol) -> &str {
        &self.strings[sym.0 as usize]
    }

    pub fn len(&self) -> usize {
        self.strings.len()
    }

    pub fn is_empty(&self) -> bool {
        self.strings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_dedupes() {
        let mut i = Interner::new();
        let a = i.intern("alpha");
        let b = i.intern("beta");
        assert_ne!(a, b);
        assert_eq!(i.intern("alpha"), a);
        assert_eq!(i.resolve(a), "alpha");
        assert_eq!(i.resolve(b), "beta");
    }

    #[test]
    fn get_never_inserts() {
        let mut i = Interner::new();
        let before = i.len();
        assert_eq!(i.get("nope"), None);
        assert_eq!(i.len(), before);
        let s = i.intern("nope");
        assert_eq!(i.get("nope"), Some(s));
    }

    #[test]
    fn keywords_are_preinterned_in_order() {
        let i = Interner::new();
        for (idx, kw) in Keyword::ALL.iter().enumerate() {
            assert_eq!(i.get(kw.as_str()), Some(Symbol(idx as u32)), "{kw:?}");
            assert_eq!(Keyword::from_symbol(Symbol(idx as u32)), Some(*kw));
        }
        assert_eq!(i.len(), Keyword::ALL.len() + sym::PREDEFINED.len());
    }

    #[test]
    fn contextual_symbols_are_not_keywords() {
        let i = Interner::new();
        assert_eq!(i.get("from"), Some(sym::FROM));
        assert_eq!(i.resolve(sym::FROM), "from");
        assert_eq!(i.get("set"), Some(sym::SET));
        assert_eq!(i.resolve(sym::SET), "set");
        assert_eq!(Keyword::from_symbol(sym::FROM), None);
        assert_eq!(Keyword::from_symbol(sym::SET), None);
    }

    #[test]
    fn handles_unicode_and_empty() {
        let mut i = Interner::new();
        let e = i.intern("");
        let u = i.intern("naïve_名前");
        assert_eq!(i.resolve(e), "");
        assert_eq!(i.resolve(u), "naïve_名前");
        assert!(!i.is_empty());
    }
}
