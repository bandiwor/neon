//! Phase 2 end-to-end: source -> SourceMap -> tokens, with lexical errors
//! rendered through the diagnostic emitter.

use neon::diag::{DiagBag, Emitter, SourceMap};
use neon::syntax::{tokenize, Interner, Keyword, LitPool, TokenKind};
use neon::vfs::MemVfs;

fn lex_file(vfs: &MemVfs, path: &str) -> (SourceMap, Interner, LitPool, DiagBag, Vec<neon::syntax::Token>) {
    let mut map = SourceMap::new();
    let id = map.load(vfs, path).unwrap();
    let mut interner = Interner::new();
    let mut pool = LitPool::new();
    let mut diags = DiagBag::new();
    let tokens = tokenize(id, map.file(id).text(), &mut interner, &mut pool, &mut diags);
    (map, interner, pool, diags, tokens)
}

#[test]
fn lexes_a_whole_program() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        r#"import { sqrt } from "math";
// factorial
fn fact(n: u64) -> u64 {
    if n <= 1u64 { return 1; }
    return n * fact(n - 1);
}
let m = { "a": 1, "b": 2 };
let s = set { 1, 2, 3 };
let f = |x| => { x + 0.5f32 };
"#,
    );
    let (_, interner, pool, diags, tokens) = lex_file(&vfs, "main.neon");
    assert!(diags.is_empty());
    assert_eq!(tokens.last().unwrap().kind, TokenKind::Eof);

    let idents: Vec<&str> = tokens
        .iter()
        .filter_map(|t| if let TokenKind::Ident(s) = t.kind { Some(interner.resolve(s)) } else { None })
        .collect();
    assert_eq!(idents, ["sqrt", "from", "fact", "n", "n", "n", "fact", "n", "m", "s", "set", "f", "x", "x"]);

    let strings: Vec<&str> =
        tokens.iter().filter_map(|t| if let TokenKind::Str(s) = t.kind { Some(pool.str(s)) } else { None }).collect();
    assert_eq!(strings, ["math", "a", "b"]);

    let fn_count = tokens.iter().filter(|t| t.kind == TokenKind::Kw(Keyword::Fn)).count();
    assert_eq!(fn_count, 1);
}

#[test]
fn reports_every_lexical_error_and_keeps_going() {
    let vfs = MemVfs::new().with_file("bad.neon", "let a = 0b102;\nlet b = 'xy';\nlet c = \"oops \\q\";\nlet d = 7;\n");
    let (map, _, pool, diags, tokens) = lex_file(&vfs, "bad.neon");
    assert_eq!(diags.error_count(), 3);

    // The valid declaration after the errors is still fully tokenized.
    let tail: Vec<_> = tokens.iter().rev().take(5).map(|t| t.kind).collect();
    assert_eq!(tail[0], TokenKind::Eof);
    assert_eq!(tail[1], TokenKind::Semi);
    let TokenKind::Int(id) = tail[2] else { panic!("expected int, got {:?}", tail[2]) };
    assert_eq!(pool.int(id).value, 7);

    let out = Emitter::new(&map).render_all(&diags);
    assert!(out.starts_with("error[E0005]: invalid digit `2` in binary literal\n --> bad.neon:1:9\n"), "{out}");
    assert!(out.contains("error[E0006]: character literal may only contain one character\n"), "{out}");
    assert!(out.contains("error[E0004]: unknown escape `\\q`\n --> bad.neon:3:15\n"), "{out}");
    assert!(out.ends_with("error: aborting due to 3 previous errors\n"), "{out}");
}
