//! Names in the generated C: which identifiers are taken, and how source names become safe C locals.

use std::collections::HashSet;
use std::sync::OnceLock;

/// The C keywords, the identifiers the generated code uses itself, and the library names it may call.
const FIXED: &[&str] = &[
    // keywords (C99, C11, C23 and the common extensions)
    "auto",
    "break",
    "case",
    "char",
    "const",
    "continue",
    "default",
    "do",
    "double",
    "else",
    "enum",
    "extern",
    "float",
    "for",
    "goto",
    "if",
    "inline",
    "int",
    "long",
    "register",
    "restrict",
    "return",
    "short",
    "signed",
    "sizeof",
    "static",
    "struct",
    "switch",
    "typedef",
    "union",
    "unsigned",
    "void",
    "volatile",
    "while",
    "_Bool",
    "_Complex",
    "_Imaginary",
    "_Alignas",
    "_Alignof",
    "_Atomic",
    "_Generic",
    "_Noreturn",
    "_Static_assert",
    "_Thread_local",
    "asm",
    "typeof",
    "alignas",
    "alignof",
    "bool",
    "true",
    "false",
    "nullptr",
    "constexpr",
    // what the generator writes into function bodies
    "t",
    "o",
    "up",
    "fr",
    "fr_roots",
    "main",
    "argc",
    "argv",
    "k_lit",
    "k_str",
    "kstr",
    "user_roots",
    // the library
    "NULL",
    "stdin",
    "stdout",
    "stderr",
    "errno",
    "fflush",
    "printf",
    "puts",
    "putchar",
    "exit",
    "abort",
    "malloc",
    "calloc",
    "realloc",
    "free",
    "memcpy",
    "memmove",
    "memset",
    "memcmp",
    "strlen",
    "strcmp",
    "sqrt",
    "sqrtf",
    "sin",
    "cos",
    "tan",
    "asin",
    "acos",
    "atan",
    "atan2",
    "sinh",
    "cosh",
    "tanh",
    "exp",
    "exp2",
    "log",
    "log2",
    "log10",
    "pow",
    "powf",
    "floor",
    "floorf",
    "ceil",
    "ceilf",
    "round",
    "roundf",
    "trunc",
    "truncf",
    "fabs",
    "fabsf",
    "fmod",
    "fmodf",
    "fmin",
    "fmax",
    "cbrt",
    "hypot",
    "isnan",
    "isinf",
    "signbit",
    "copysign",
    "abs",
    "labs",
    "llabs",
    "div",
    "ldiv",
    "INT8_MIN",
    "INT8_MAX",
    "INT16_MIN",
    "INT16_MAX",
    "INT32_MIN",
    "INT32_MAX",
    "INT64_MIN",
    "INT64_MAX",
    "UINT8_MAX",
    "UINT16_MAX",
    "UINT32_MAX",
    "UINT64_MAX",
    "SIZE_MAX",
    "INFINITY",
    "NAN",
    "HUGE_VAL",
    "M_PI",
    "M_E",
];

/// Identifiers declared at the top level of `neon.h`: types, macros, enumerators, functions, variables.
/// (Parameter and member names are not among them: a local may be called `len` or `next`.)
fn header_names(text: &str) -> HashSet<String> {
    // comments out
    let mut code = String::with_capacity(text.len());
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
            code.push(' ');
        } else if b[i] == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            code.push(b[i] as char);
            i += 1;
        }
    }
    let mut out = HashSet::new();
    let (mut brace, mut paren) = (0usize, 0usize);
    // the stack of open braces: is it an `enum { .. }` body?
    let mut enum_body: Vec<bool> = Vec::new();
    let mut last_word = String::new();
    let mut seen_enum = false;
    let mut lines = code.lines().peekable();
    while let Some(line) = lines.next() {
        let t = line.trim_start();
        if t.starts_with('#') {
            // a directive: only `#define NAME` declares something (a continued line is skipped whole)
            if let Some(rest) = t.strip_prefix('#').map(str::trim_start).and_then(|r| r.strip_prefix("define")) {
                let name: String = rest.trim_start().chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !name.is_empty() {
                    out.insert(name);
                }
            }
            let mut cur = line.to_string();
            while cur.ends_with('\\') {
                match lines.next() {
                    Some(l) => cur = l.to_string(),
                    None => break,
                }
            }
            continue;
        }
        let cs: Vec<char> = line.chars().collect();
        let mut k = 0;
        while k < cs.len() {
            let c = cs[k];
            if c.is_alphabetic() || c == '_' {
                let start = k;
                while k < cs.len() && (cs[k].is_alphanumeric() || cs[k] == '_') {
                    k += 1;
                }
                let word: String = cs[start..k].iter().collect();
                let in_enum = enum_body.last().copied().unwrap_or(false);
                if (brace == 0 && paren == 0) || (in_enum && paren == 0) {
                    // an enumerator is a word right after `{` or `,`
                    if !in_enum || matches!(last_word.as_str(), "{" | ",") {
                        out.insert(word.clone());
                    }
                }
                if word == "enum" {
                    seen_enum = true;
                }
                last_word = word;
                continue;
            }
            match c {
                '{' => {
                    brace += 1;
                    enum_body.push(seen_enum);
                    seen_enum = false;
                    last_word = "{".into();
                }
                '}' => {
                    brace = brace.saturating_sub(1);
                    enum_body.pop();
                }
                '(' => paren += 1,
                ')' => paren = paren.saturating_sub(1),
                ',' => last_word = ",".into(),
                ';' => seen_enum = false,
                _ => {}
            }
            k += 1;
        }
    }
    out
}

/// Every identifier a local variable must not take.
pub fn reserved() -> &'static HashSet<String> {
    static SET: OnceLock<HashSet<String>> = OnceLock::new();
    SET.get_or_init(|| {
        let mut s = header_names(super::NEON_H);
        s.extend(FIXED.iter().map(|x| x.to_string()));
        s
    })
}

/// A source name as a C identifier, if it is one already (ASCII letters, digits and `_`).
pub fn c_ident(name: &str) -> Option<String> {
    let ok = !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    // `__x` and `_X` are reserved for the implementation
    let reserved_form =
        name.starts_with("__") || name.starts_with('_') && name[1..].starts_with(|c: char| c.is_ascii_uppercase());
    (ok && !reserved_form).then(|| name.to_string())
}

/// `name`, or `name_2`, `name_3`, ... : the first one not in `taken`; the result is added to `taken`.
pub fn unique(name: &str, taken: &mut HashSet<String>) -> String {
    let mut cand = name.to_string();
    let mut n = 2;
    while taken.contains(&cand) || reserved().contains(&cand) {
        cand = format!("{name}_{n}");
        n += 1;
    }
    taken.insert(cand.clone());
    cand
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_declarations_are_reserved_but_members_and_parameters_are_not() {
        let r = reserved();
        for n in ["Obj", "i64", "str_new", "add_i64", "K_STR", "MF_SET", "GC_POINT", "FRAME", "int", "rt_sb", "Arr"] {
            assert!(r.contains(n), "{n}");
        }
        for n in ["len", "cap", "next", "top", "fmt", "index", "count", "x", "total", "name"] {
            assert!(!r.contains(n), "{n} should be free");
        }
    }

    #[test]
    fn source_names_become_unique_identifiers() {
        let mut taken = HashSet::new();
        assert_eq!(unique("total", &mut taken), "total");
        assert_eq!(unique("total", &mut taken), "total_2");
        assert_eq!(unique("int", &mut taken), "int_2");
        assert_eq!(unique("Obj", &mut taken), "Obj_2");
        assert_eq!(c_ident("a_1").as_deref(), Some("a_1"));
        assert_eq!(c_ident("1a"), None);
        assert_eq!(c_ident("привет"), None);
        assert_eq!(c_ident("__x"), None);
        assert_eq!(c_ident("_X"), None);
        assert_eq!(c_ident("_x").as_deref(), Some("_x"));
    }
}
