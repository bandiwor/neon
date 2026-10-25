//! Phase 3 end-to-end: parse whole programs from a `Vfs`, check the tree, and
//! verify that malformed input is reported without losing later declarations.

use neon::diag::{DiagBag, Emitter, SourceMap};
use neon::syntax::{parse_source, sexpr, Ast, Interner, NodeId, NodeKind};
use neon::vfs::MemVfs;

fn parse_file(vfs: &MemVfs, path: &str) -> (SourceMap, Interner, DiagBag, Ast) {
    let mut map = SourceMap::new();
    let id = map.load(vfs, path).unwrap();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let ast = parse_source(id, map.file(id).text(), &mut interner, &mut diags);
    (map, interner, diags, ast)
}

fn top_level(ast: &Ast) -> Vec<NodeId> {
    let NodeKind::Module { items } = *ast.kind(ast.root()) else { panic!("root must be a module") };
    ast.list(items).to_vec()
}

/// Names of top-level `fn` declarations (looking through `export`).
fn fn_names(ast: &Ast, interner: &Interner) -> Vec<String> {
    top_level(ast)
        .into_iter()
        .filter_map(|id| {
            let id = match *ast.kind(id) {
                NodeKind::Export { item } => item,
                _ => id,
            };
            match *ast.kind(id) {
                NodeKind::FnDecl { name, .. } => Some(interner.resolve(name).to_string()),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn parses_a_program_using_every_feature() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        r#"
import { sqrt, pow as power } from "math";
import "util/text" as text;

export const PI: f64 = 3.14159;

enum Option<T> { Some(T), None }
enum Shape { Circle(f64), Rect(f64, f64), Empty }

struct Vec2 { x: f64, y: f64 }

trait Area { fn area(self) -> f64; }
trait Add<Rhs> { fn add(self, rhs: Rhs) -> Self; }

impl Area for Shape {
    fn area(self) -> f64 {
        match self {
            Shape::Circle(r) => PI * r * r,
            Shape::Rect(w, h) => w * h,
            Shape::Empty => 0.0,
        }
    }
}

impl Add<Vec2> for Vec2 {
    fn add(self, rhs: Vec2) -> Vec2 { Vec2 { x: self.x + rhs.x, y: self.y + rhs.y } }
}

export fn fact(n: u64) -> u64 {
    if n <= 1u64 { return 1u64; }
    n * fact(n - 1u64)
}

fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }

fn unwrap_or<T>(o: Option<T>, default: T) -> T {
    match o { Option::Some(x) => x, Option::None => default }
}

fn make_counter() -> fn() -> i32 {
    var count = 0;
    return || => { count += 1; count };
}

let counter = make_counter();
counter();
let scores = { "ann": 3, "bob": 5 };
let seen = set { 1, 2, 3 };
let xs = [1, 2, 3];
var total = 0;
for x in xs { total += x; }
for i in 0..10 { if i % 2 == 0 { continue; } total += i; }
let big = 255u8 as i64 + (1 << 40);
print(total);
"#,
    );
    let (map, interner, diags, ast) = parse_file(&vfs, "main.neon");
    assert!(diags.is_empty(), "{}", Emitter::new(&map).render_all(&diags));
    assert!(!ast.has_errors());
    assert_eq!(fn_names(&ast, &interner), ["fact", "fib", "unwrap_or", "make_counter"]);

    // spot-check one function in full
    let fact = top_level(&ast)
        .into_iter()
        .find_map(|id| match *ast.kind(id) {
            NodeKind::Export { item } if matches!(ast.kind(item), NodeKind::FnDecl { .. }) => Some(item),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        sexpr(&ast, &interner, fact),
        "(fn fact (n:u64) -> u64 (block (stmt- (if (<= n 1u64) (block (stmt (return 1u64))))) (tail (* n (call fact (- n 1u64))))))"
    );
}

#[test]
fn reports_malformed_lines_and_still_parses_following_declarations() {
    let vfs = MemVfs::new().with_file(
        "broken.neon",
        "fn first() -> i32 { 1 }\n\
         let broken = ;\n\
         fn second(a: i32) -> i32 { a + }\n\
         struct { x: i32 }\n\
         fn third() { let ok = 3; ok }\n\
         enum Color { Red, Green(, Blue }\n\
         export fn fourth() -> i32 { 4 }\n",
    );
    let (map, interner, diags, ast) = parse_file(&vfs, "broken.neon");

    // Every malformed construct is reported (not just the first).
    assert!(diags.error_count() >= 4, "{}", Emitter::new(&map).render_all(&diags));
    let rendered = Emitter::new(&map).render_all(&diags);
    assert!(rendered.contains("--> broken.neon:2:14"), "{rendered}");
    assert!(rendered.contains("--> broken.neon:3:32"), "{rendered}");

    // Valid declarations on both sides of the errors survive; tooling gets a tree.
    let names = fn_names(&ast, &interner);
    assert!(names.contains(&"first".to_string()), "{names:?}");
    assert!(names.contains(&"third".to_string()), "{names:?}");
    assert!(names.contains(&"fourth".to_string()), "{names:?}");
    assert!(ast.has_errors());

    // The broken ones are placeholders spanning exactly the skipped text.
    let text = map.file(ast.file).text();
    let error_texts: Vec<&str> = top_level(&ast)
        .into_iter()
        .filter(|&id| matches!(ast.kind(id), NodeKind::Error))
        .map(|id| &text[ast.span(id).range()])
        .collect();
    assert_eq!(error_texts[0], "let broken = ;");
}

#[test]
fn diagnostics_are_deduplicated_per_token_and_ordered() {
    let vfs = MemVfs::new().with_file("m.neon", "let a = ;\nlet b = (;\nlet c = 1 + ;\n");
    let (_, _, diags, ast) = parse_file(&vfs, "m.neon");
    assert_eq!(diags.error_count(), 3);
    let lines: Vec<u32> = diags.iter().map(|d| d.primary_span().unwrap().lo).collect();
    assert!(lines.windows(2).all(|w| w[0] < w[1]), "{lines:?}");
    assert_eq!(top_level(&ast).len(), 3);
}

#[test]
fn partial_trees_support_position_queries() {
    let src = "let a = 1;\nlet b = ;\nlet c = value;\n";
    let vfs = MemVfs::new().with_file("m.neon", src);
    let (_, interner, diags, ast) = parse_file(&vfs, "m.neon");
    assert_eq!(diags.error_count(), 1);
    // A hover/definition query on a valid node after the error still works.
    let id = ast.node_at(src.rfind("value").unwrap() as u32).unwrap();
    assert_eq!(sexpr(&ast, &interner, id), "value");
    // And a position inside the broken statement lands on the error node.
    let id = ast.node_at(src.find("= ;").unwrap() as u32 + 2).unwrap();
    assert!(matches!(ast.kind(id), NodeKind::Error));
}
