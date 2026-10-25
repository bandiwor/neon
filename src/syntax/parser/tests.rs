use super::parse_source;
use crate::diag::{DiagBag, Emitter, FileId, SourceMap};
use crate::syntax::{sexpr, sexpr_module, Ast, Interner, NodeId, NodeKind};

struct Parsed {
    ast: Ast,
    interner: Interner,
    diags: DiagBag,
}

fn parse(src: &str) -> Parsed {
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let ast = parse_source(FileId(0), src, &mut interner, &mut diags);
    Parsed { ast, interner, diags }
}

impl Parsed {
    fn module(&self) -> String {
        sexpr_module(&self.ast, &self.interner)
    }

    fn show(&self, id: NodeId) -> String {
        sexpr(&self.ast, &self.interner, id)
    }

    fn items(&self) -> Vec<NodeId> {
        let NodeKind::Module { items } = *self.ast.kind(self.ast.root()) else { panic!("root is not a module") };
        self.ast.list(items).to_vec()
    }

    fn messages(&self) -> Vec<String> {
        self.diags.iter().map(|d| d.message.clone()).collect()
    }

    fn rendered(&self, src: &str) -> String {
        let mut map = SourceMap::new();
        map.add_file("t.neon", src).unwrap();
        Emitter::new(&map).render_all(&self.diags)
    }
}

/// Parses a single expression statement and returns its S-expression.
fn expr(src: &str) -> String {
    let p = parse(&format!("{src};"));
    assert!(p.diags.is_empty(), "{src}: {:?}", p.messages());
    let items = p.items();
    assert_eq!(items.len(), 1, "{src}");
    let NodeKind::ExprStmt { expr, .. } = *p.ast.kind(items[0]) else { panic!("{src}: not an expr stmt") };
    p.show(expr)
}

/// Parses `src` as a whole module and returns its S-expression.
fn module(src: &str) -> String {
    let p = parse(src);
    assert!(p.diags.is_empty(), "{src}: {:?}\n{}", p.messages(), p.rendered(src));
    p.module()
}

#[test]
fn literals() {
    assert_eq!(expr("42"), "42");
    assert_eq!(expr("42u8"), "42u8");
    assert_eq!(expr("1.5f32"), "1.5f32");
    assert_eq!(expr("\"hi\""), "\"hi\"");
    assert_eq!(expr("'x'"), "'x'");
    assert_eq!(expr("true"), "true");
    assert_eq!(expr("false"), "false");
    assert_eq!(expr("()"), "()");
}

#[test]
fn binary_precedence_and_associativity() {
    assert_eq!(expr("1 + 2 * 3"), "(+ 1 (* 2 3))");
    assert_eq!(expr("1 * 2 + 3"), "(+ (* 1 2) 3)");
    assert_eq!(expr("a - b - c"), "(- (- a b) c)");
    assert_eq!(expr("a / b % c"), "(% (/ a b) c)");
    assert_eq!(expr("(1 + 2) * 3"), "(* (+ 1 2) 3)");
    assert_eq!(expr("a || b && c"), "(|| a (&& b c))");
    assert_eq!(expr("a && b || c"), "(|| (&& a b) c)");
    assert_eq!(expr("a == b || c < d"), "(|| (== a b) (< c d))");
    assert_eq!(expr("a | b ^ c & d"), "(| a (^ b (& c d)))");
    assert_eq!(expr("a << 1 + 2"), "(<< a (+ 1 2))");
    assert_eq!(expr("a & b == c"), "(== (& a b) c)");
}

#[test]
fn unary_and_cast() {
    assert_eq!(expr("-a"), "(- a)");
    assert_eq!(expr("!a && b"), "(&& (! a) b)");
    assert_eq!(expr("~a"), "(~ a)");
    assert_eq!(expr("- - a"), "(- (- a))");
    assert_eq!(expr("-a * b"), "(* (- a) b)");
    assert_eq!(expr("x as i64"), "(as x i64)");
    assert_eq!(expr("-x as u8"), "(as (- x) u8)");
    assert_eq!(expr("a + b as f64"), "(+ a (as b f64))");
    assert_eq!(expr("a as f64 * 2.0"), "(* (as a f64) 2.0)");
    assert_eq!(expr("x as i32 as i64"), "(as (as x i32) i64)");
}

#[test]
fn assignment_is_right_associative_and_loosest() {
    assert_eq!(expr("a = b = c"), "(= a (= b c))");
    assert_eq!(expr("a = b + 1"), "(= a (+ b 1))");
    assert_eq!(expr("a += 1"), "(+= a 1)");
    assert_eq!(expr("a <<= 2"), "(<<= a 2)");
    assert_eq!(expr("a >>= 2"), "(>>= a 2)");
    assert_eq!(expr("a %= 2"), "(%= a 2)");
    assert_eq!(expr("v[0] = x.y"), "(= (index v 0) (field x y))");
    assert_eq!(expr("p.x -= 1"), "(-= (field p x) 1)");
}

#[test]
fn ranges() {
    assert_eq!(expr("0..10"), "(.. 0 10)");
    assert_eq!(expr("0..=10"), "(..= 0 10)");
    assert_eq!(expr("a + 1..b * 2"), "(.. (+ a 1) (* b 2))");
    assert_eq!(expr("..5"), "(.. _ 5)");
    assert_eq!(expr("5.."), "(.. 5 _)");
    assert_eq!(expr(".."), "(.. _ _)");
    assert_eq!(expr("..=5"), "(..= _ 5)");
    assert_eq!(expr("x = 0..3"), "(= x (.. 0 3))");
}

#[test]
fn postfix_chains() {
    assert_eq!(expr("f(1, 2)"), "(call f 1 2)");
    assert_eq!(expr("f()"), "(call f)");
    assert_eq!(expr("f(1,)"), "(call f 1)");
    assert_eq!(expr("f(1)(2)"), "(call (call f 1) 2)");
    assert_eq!(expr("a[1][2]"), "(index (index a 1) 2)");
    assert_eq!(expr("a.b.c"), "(field (field a b) c)");
    assert_eq!(expr("a.m(1)"), "(mcall a m 1)");
    assert_eq!(expr("a.m().n(2, 3)"), "(mcall (mcall a m) n 2 3)");
    assert_eq!(expr("a.b[0].c(x)"), "(mcall (index (field a b) 0) c x)");
    assert_eq!(expr("x.parse::<i32>()"), "(mcall x parse<i32>)");
    assert_eq!(expr("-a.b"), "(- (field a b))");
    assert_eq!(expr("self.x + 1"), "(+ (field self x) 1)");
    assert_eq!(expr("1.max(2)"), "(mcall 1 max 2)");
}

#[test]
fn paths() {
    assert_eq!(expr("Option::Some(1)"), "(call Option::Some 1)");
    assert_eq!(expr("a::b::c"), "a::b::c");
    assert_eq!(expr("Self::new(1)"), "(call Self::new 1)");
    assert_eq!(expr("Vec::<i32>::new()"), "(call Vec<i32>::new)");
    assert_eq!(expr("parse::<Option<i32>>(s)"), "(call parse<Option<i32>> s)");
    assert_eq!(expr("i32::MAX"), "i32::MAX");
    assert_eq!(expr("m::sqrt(2.0)"), "(call m::sqrt 2.0)");
}

#[test]
fn collections() {
    assert_eq!(expr("[1, 2, 3]"), "(array 1 2 3)");
    assert_eq!(expr("[]"), "(array)");
    assert_eq!(expr("[1, 2,]"), "(array 1 2)");
    assert_eq!(expr("{ \"a\": 1, \"b\": 2 }"), "(map (\"a\" 1) (\"b\" 2))");
    assert_eq!(expr("{ 1: \"x\" }"), "(map (1 \"x\"))");
    assert_eq!(expr("{ k: v, }"), "(map (k v))");
    assert_eq!(expr("{ (a + 1): 2 }"), "(map ((+ a 1) 2))");
    assert_eq!(expr("{ Color::Red: 1 }"), "(map (Color::Red 1))");
    assert_eq!(expr("{ -1: 2 }"), "(map ((- 1) 2))");
    assert_eq!(expr("set { 1, 2, 3 }"), "(set 1 2 3)");
    assert_eq!(expr("set {}"), "(set)");
    assert_eq!(expr("{ \"a\": [1, 2], \"b\": { \"c\": 3 } }"), "(map (\"a\" (array 1 2)) (\"b\" (map (\"c\" 3))))");
}

#[test]
fn empty_braces_are_a_map_in_value_position() {
    assert_eq!(module("let m = {};"), "(module (let m (map)))");
    assert_eq!(expr("f({})"), "(call f (map))");
}

#[test]
fn braces_without_a_key_are_blocks() {
    assert_eq!(module("let x = { let y = 1; y + 1 };"), "(module (let x (block (let y 1) (tail (+ y 1)))))");
    assert_eq!(module("let x = { 5 };"), "(module (let x (block (tail 5))))");
}

#[test]
fn set_is_a_normal_identifier_elsewhere() {
    assert_eq!(expr("m.set(1, 2)"), "(mcall m set 1 2)");
    assert_eq!(expr("set + 1"), "(+ set 1)");
    assert_eq!(module("let set = 1;"), "(module (let set 1))");
}

#[test]
fn struct_literals() {
    assert_eq!(expr("Point { x: 1, y: 2 }"), "(struct-lit Point (x 1) (y 2))");
    assert_eq!(expr("Point { x, y }"), "(struct-lit Point (x x) (y y))");
    assert_eq!(expr("Point { x: 1, }"), "(struct-lit Point (x 1))");
    assert_eq!(expr("Unit {}"), "(struct-lit Unit)");
    assert_eq!(expr("shapes::Pt { x: 0 }"), "(struct-lit shapes::Pt (x 0))");
    assert_eq!(expr("Self { x: 0 }"), "(struct-lit Self (x 0))");
}

#[test]
fn struct_literals_are_not_allowed_in_headers() {
    assert_eq!(module("if x { 1 }"), "(module (stmt- (if x (block (tail 1)))))");
    assert_eq!(module("while i < n { i += 1; }"), "(module (stmt- (while (< i n) (block (stmt (+= i 1))))))");
    assert_eq!(module("for x in xs { f(x); }"), "(module (stmt- (for x xs (block (stmt (call f x))))))");
    assert_eq!(module("match v { _ => 0 }"), "(module (stmt- (match v (arm _ 0))))");
    // ...but are fine inside parentheses within a header.
    assert_eq!(
        module("if (Point { x: 1 }).x == 1 { }"),
        "(module (stmt- (if (== (field (struct-lit Point (x 1)) x) 1) (block))))"
    );
}

#[test]
fn closures() {
    assert_eq!(expr("|a, b| => a + b"), "(closure (a b) (+ a b))");
    assert_eq!(expr("|| => 1"), "(closure () 1)");
    assert_eq!(expr("|x| => { x * 2 }"), "(closure (x) (block (tail (* x 2))))");
    assert_eq!(expr("|x: i32, mut y| => x"), "(closure (x:i32 mut y) x)");
    assert_eq!(expr("|x| -> i64 => x"), "(closure (x) -> i64 x)");
    assert_eq!(expr("|| => {}"), "(closure () (block))");
    assert_eq!(expr("|| => { \"k\": 1 }"), "(closure () (map (\"k\" 1)))");
    assert_eq!(expr("f(|x| => x, 2)"), "(call f (closure (x) x) 2)");
    assert_eq!(expr("|n| => { count += n; count }"), "(closure (n) (block (stmt (+= count n)) (tail count)))");
    assert_eq!(expr("a | b"), "(| a b)");
    assert_eq!(expr("a || b"), "(|| a b)");
}

#[test]
fn control_flow() {
    assert_eq!(
        expr("if a { 1 } else if b { 2 } else { 3 }"),
        "(if a (block (tail 1)) (if b (block (tail 2)) (block (tail 3))))"
    );
    assert_eq!(expr("loop { break; }"), "(loop (block (stmt (break))))");
    assert_eq!(expr("loop { break 5 }"), "(loop (block (tail (break 5))))");
    assert_eq!(expr("while true { continue; }"), "(while true (block (stmt (continue))))");
    assert_eq!(expr("for i in 0..n { }"), "(for i (.. 0 n) (block))");
    assert_eq!(expr("for _ in xs { }"), "(for _ xs (block))");
    assert_eq!(expr("return"), "(return)");
    assert_eq!(expr("return 1 + 2"), "(return (+ 1 2))");
    assert_eq!(module("fn f() { return; }"), "(module (fn f () (block (stmt (return)))))");
    assert_eq!(expr("let_ = if a { 1 } else { 2 }"), "(= let_ (if a (block (tail 1)) (block (tail 2))))");
}

#[test]
fn block_like_statements_end_at_their_brace() {
    // Without `;` after the `if`, `-1` is a new statement, not a subtraction.
    assert_eq!(module("if a { } -1;"), "(module (stmt- (if a (block))) (stmt (- 1)))");
    assert_eq!(module("{ 1 } foo();"), "(module (stmt- (block (tail 1))) (stmt (call foo)))");
    assert_eq!(
        module("fn f() -> i32 { if a { 1 } else { 2 } }"),
        "(module (fn f () -> i32 (block (tail (if a (block (tail 1)) (block (tail 2)))))))"
    );
}

#[test]
fn match_arms_and_patterns() {
    assert_eq!(
        expr("match v { Option::Some(x) => x, _ => 0 }"),
        "(match v (arm (variant Option::Some x) x) (arm _ 0))"
    );
    assert_eq!(
        expr("match v { Option::None => 0, Option::Some(x) if x > 1 => x, _ => 1 }"),
        "(match v (arm Option::None 0) (arm (variant Option::Some x) if (> x 1) x) (arm _ 1))"
    );
    assert_eq!(
        expr("match n { 0 => \"zero\", 1 | 2 => \"few\", -1 => \"neg\", 3..=9 => \"some\", _ => \"many\" }"),
        "(match n (arm 0 \"zero\") (arm (or 1 2) \"few\") (arm (- 1) \"neg\") (arm (..= 3 9) \"some\") (arm _ \"many\"))"
    );
    assert_eq!(expr("match c { 'a'..='z' => 1, true => 2 }"), "(match c (arm (..= 'a' 'z') 1) (arm true 2))");
    assert_eq!(
        expr("match s { Shape::Rect(w, h) => w * h, Shape::Circle(r) => r }"),
        "(match s (arm (variant Shape::Rect w h) (* w h)) (arm (variant Shape::Circle r) r))"
    );
    assert_eq!(expr("match x { Some(v) => v, y => y }"), "(match x (arm (variant Some v) v) (arm y y))");
    assert_eq!(
        expr("match x { Option::Some(Option::Some(v)) => v, _ => 0 }"),
        "(match x (arm (variant Option::Some (variant Option::Some v)) v) (arm _ 0))"
    );
    assert_eq!(expr("match x { mut y => y }"), "(match x (arm mut y y))");
}

#[test]
fn match_arm_separators() {
    // Commas are optional after block bodies and allowed as trailing.
    assert_eq!(
        expr("match x { 1 => { a } 2 => { b }, _ => c, }"),
        "(match x (arm 1 (block (tail a))) (arm 2 (block (tail b))) (arm _ c))"
    );
    assert_eq!(expr("match x { _ => {} }"), "(match x (arm _ (block)))");
}

#[test]
fn let_statements() {
    assert_eq!(module("let x = 1;"), "(module (let x 1))");
    assert_eq!(module("let mut x: i32 = 1;"), "(module (let mut x:i32 1))");
    assert_eq!(module("let x: [i32];"), "(module (let x:[i32]))");
    assert_eq!(module("let _ = f();"), "(module (let _ (call f)))");
}

#[test]
fn types() {
    let ty = |t: &str| {
        let src = format!("let x: {t};");
        let p = parse(&src);
        assert!(p.diags.is_empty(), "{t}: {:?}", p.messages());
        let NodeKind::Let { ty, .. } = *p.ast.kind(p.items()[0]) else { panic!() };
        p.show(ty.get().unwrap())
    };
    assert_eq!(ty("i32"), "i32");
    assert_eq!(ty("string"), "string");
    assert_eq!(ty("Point"), "Point");
    assert_eq!(ty("Option<i32>"), "Option<i32>");
    assert_eq!(ty("Option<Option<i32>>"), "Option<Option<i32>>");
    assert_eq!(ty("Option<Option<Option<u8>>>"), "Option<Option<Option<u8>>>");
    assert_eq!(ty("[i32]"), "[i32]");
    assert_eq!(ty("[[f64]]"), "[[f64]]");
    assert_eq!(ty("{string: i32}"), "{string: i32}");
    assert_eq!(ty("{string: [Option<i32>]}"), "{string: [Option<i32>]}");
    assert_eq!(ty("set<i32>"), "set<i32>");
    assert_eq!(ty("set<Option<i32>>"), "set<Option<i32>>");
    assert_eq!(ty("fn(i32, i32) -> i32"), "fn(i32, i32) -> i32");
    assert_eq!(ty("fn()"), "fn()");
    assert_eq!(ty("fn(i32) -> fn(i32) -> i32"), "fn(i32) -> fn(i32) -> i32");
    assert_eq!(ty("()"), "()");
    assert_eq!(ty("m::Point"), "m::Point");
    assert_eq!(ty("Pair<A, B>"), "Pair<A, B>");
}

#[test]
fn generic_closers_split_from_compound_tokens() {
    // `>>=` / `>=` are lexed as one token but close generics and then assign/compare.
    assert_eq!(module("let x: Option<Option<i32>>= f();"), "(module (let x:Option<Option<i32>> (call f)))");
    assert_eq!(module("let x: Option<i32>= f();"), "(module (let x:Option<i32> (call f)))");
}

#[test]
fn functions() {
    assert_eq!(module("fn f() { }"), "(module (fn f () (block)))");
    assert_eq!(
        module("fn add(a: i32, b: i32) -> i32 { a + b }"),
        "(module (fn add (a:i32 b:i32) -> i32 (block (tail (+ a b)))))"
    );
    assert_eq!(module("fn id<T>(x: T) -> T { x }"), "(module (fn id <T> (x:T) -> T (block (tail x))))");
    assert_eq!(
        module("fn show<T: Display + Debug, U>(x: T, mut y: U) { }"),
        "(module (fn show <T: Display + Debug, U> (x:T mut y:U) (block)))"
    );
    assert_eq!(
        module("fn apply(f: fn(i32) -> i32, x: i32) -> i32 { f(x) }"),
        "(module (fn apply (f:fn(i32) -> i32 x:i32) -> i32 (block (tail (call f x)))))"
    );
    assert_eq!(module("fn f<T: Into<Vec<i32>>>() { }"), "(module (fn f <T: Into<Vec<i32>>> () (block)))");
}

#[test]
fn structs_and_enums() {
    assert_eq!(module("struct P { x: f64, y: f64 }"), "(module (struct P (x:f64 y:f64)))");
    assert_eq!(module("struct P { x: f64, }"), "(module (struct P (x:f64)))");
    assert_eq!(module("struct Unit;"), "(module (struct Unit ()))");
    assert_eq!(module("struct Pair<A, B> { a: A, b: B }"), "(module (struct Pair <A, B> (a:A b:B)))");
    assert_eq!(module("enum Option<T> { Some(T), None }"), "(module (enum Option <T> (Some(T) None)))");
    assert_eq!(
        module("enum Shape { Circle(f64), Rect(f64, f64), Empty, }"),
        "(module (enum Shape (Circle(f64) Rect(f64, f64) Empty)))"
    );
}

#[test]
fn traits_and_impls() {
    assert_eq!(module("trait Area { fn area(self) -> f64; }"), "(module (trait Area (fn area (self) -> f64 ;)))");
    assert_eq!(
        module("trait Shape<T> { fn scale(self, k: T) -> Self; fn name(self) -> string { \"shape\" } }"),
        "(module (trait Shape <T> (fn scale (self k:T) -> Self ;) (fn name (self) -> string (block (tail \"shape\")))))"
    );
    assert_eq!(
        module("impl Point { fn new(x: f64) -> Self { Self { x: x } } }"),
        "(module (impl Point (fn new (x:f64) -> Self (block (tail (struct-lit Self (x x)))))))"
    );
    assert_eq!(
        module("impl Add for Vec2 { fn add(self, o: Vec2) -> Vec2 { o } }"),
        "(module (impl Add for Vec2 (fn add (self o:Vec2) -> Vec2 (block (tail o)))))"
    );
    assert_eq!(
        module("impl<T> Show for Option<T> { fn show(mut self) { } }"),
        "(module (impl <T> Show for Option<T> (fn show (mut self) (block))))"
    );
    assert_eq!(module("impl Mul<f64> for Vec2 { }"), "(module (impl Mul<f64> for Vec2))");
}

#[test]
fn imports_exports_and_consts() {
    assert_eq!(module("import { sqrt, pow as power } from \"math\";"), "(module (import \"math\" sqrt pow:power))");
    assert_eq!(module("import {} from \"x\";"), "(module (import \"x\"))");
    assert_eq!(module("import \"util/strings\" as s;"), "(module (import \"util/strings\" as s))");
    assert_eq!(module("export fn f() { }"), "(module (export (fn f () (block))))");
    assert_eq!(module("export struct S { a: i32 }"), "(module (export (struct S (a:i32))))");
    assert_eq!(module("export enum E { A }"), "(module (export (enum E (A))))");
    assert_eq!(module("export trait T { }"), "(module (export (trait T)))");
    assert_eq!(module("export const PI: f64 = 3.14;"), "(module (export (const PI:f64 3.14)))");
    assert_eq!(module("const N = 5;"), "(module (const N 5))");
}

#[test]
fn module_mixes_items_and_statements_and_ends_with_optional_expression() {
    assert_eq!(module("fn f() { }\nlet x = f();\nx"), "(module (fn f () (block)) (let x (call f)) (stmt- x))");
    assert_eq!(module(";;"), "(module)");
    assert_eq!(module(""), "(module)");
    assert_eq!(module("// only a comment"), "(module)");
}

#[test]
fn nested_items_in_blocks_are_parsed() {
    assert_eq!(
        module("fn outer() { fn inner() { } inner(); }"),
        "(module (fn outer () (block (fn inner () (block)) (stmt (call inner)))))"
    );
}

#[test]
fn spans_cover_whole_constructs() {
    let src = "let x = 1 +\n  2 * f(3);";
    let p = parse(src);
    let items = p.items();
    assert_eq!(&src[p.ast.span(items[0]).range()], src);
    let NodeKind::Let { init, .. } = *p.ast.kind(items[0]) else { panic!() };
    assert_eq!(&src[p.ast.span(init.get().unwrap()).range()], "1 +\n  2 * f(3)");
    // parentheses count toward the parent's span but not the inner node's
    let p = parse("(1 + 2) * 3;");
    let NodeKind::ExprStmt { expr, .. } = *p.ast.kind(p.items()[0]) else { panic!() };
    assert_eq!(&"(1 + 2) * 3;"[p.ast.span(expr).range()], "(1 + 2) * 3");
}

#[test]
fn path_segment_spans_are_individual() {
    let src = "a::bb::ccc;";
    let p = parse(src);
    let NodeKind::ExprStmt { expr, .. } = *p.ast.kind(p.items()[0]) else { panic!() };
    let NodeKind::Path { segments } = *p.ast.kind(expr) else { panic!() };
    let texts: Vec<_> = p.ast.list(segments).iter().map(|&s| &src[p.ast.span(s).range()]).collect();
    assert_eq!(texts, ["a", "bb", "ccc"]);
}

#[test]
fn node_at_resolves_positions() {
    let src = "let total = price * 2;";
    let p = parse(src);
    let id = p.ast.node_at(src.find("price").unwrap() as u32).unwrap();
    assert_eq!(p.show(id), "price");
    let id = p.ast.node_at(src.find('*').unwrap() as u32).unwrap();
    assert_eq!(p.show(id), "(* price 2)");
}

// ---- error recovery --------------------------------------------------------

#[test]
fn recovers_at_statement_boundaries() {
    let src = "let a = ;\nlet b = 2;\nlet c = (;\nlet d = 4;";
    let p = parse(src);
    assert_eq!(p.diags.error_count(), 2, "{:?}", p.messages());
    assert_eq!(p.module(), "(module <error> (let b 2) <error> (let d 4))");
    assert!(p.ast.has_errors());
}

#[test]
fn error_node_spans_the_skipped_text() {
    let src = "let a = ;\nlet b = 2;";
    let p = parse(src);
    let items = p.items();
    assert_eq!(&src[p.ast.span(items[0]).range()], "let a = ;");
    assert_eq!(&src[p.ast.span(items[1]).range()], "let b = 2;");
}

#[test]
fn missing_semicolon_is_reported_without_losing_the_statement() {
    let src = "let a = 1\nlet b = 2;";
    let p = parse(src);
    assert_eq!(p.diags.error_count(), 1);
    assert!(p.messages()[0].starts_with("expected `;`, found keyword `let`"), "{:?}", p.messages());
    // The error points just after `1`, where the `;` belongs.
    assert_eq!(p.diags.as_slice()[0].primary_span().unwrap().lo, 9);
    assert!(p.module().ends_with("(let b 2))"), "{}", p.module());

    let p = parse("f()\ng();");
    assert_eq!(p.diags.error_count(), 1);
    assert_eq!(p.module(), "(module (stmt- (call f)) (stmt (call g)))");
}

#[test]
fn recovers_inside_blocks_and_keeps_siblings() {
    let src = "fn f() {\n  let a = ;\n  let b = 2;\n  b\n}\nfn g() { 1 }";
    let p = parse(src);
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    assert_eq!(p.module(), "(module (fn f () (block <error> (let b 2) (tail b))) (fn g () (block (tail 1))))");
}

#[test]
fn recovers_from_broken_declarations() {
    let src = "struct { x: i32 }\nfn ok() { }\nenum E { A, B(, C }\nfn also_ok() { }";
    let p = parse(src);
    assert!(p.diags.error_count() >= 2);
    let m = p.module();
    assert!(m.contains("(fn ok () (block))"), "{m}");
    assert!(m.contains("(fn also_ok () (block))"), "{m}");
}

#[test]
fn recovers_inside_match_arms() {
    let src = "match x { 1 => 10, 2 => ), 3 => 30 }";
    let p = parse(src);
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    assert_eq!(p.module(), "(module (stmt- (match x (arm 1 10) <error> (arm 3 30))))");
}

#[test]
fn recovers_inside_impl_and_trait_bodies() {
    let p = parse("impl T { 5 fn ok(self) { } }");
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    assert!(p.module().contains("(fn ok (self) (block))"), "{}", p.module());

    let p = parse("trait T { fn a(self); let x; fn b(self); }");
    assert!(p.diags.error_count() >= 1);
    let m = p.module();
    assert!(m.contains("(fn a (self) ;)") && m.contains("(fn b (self) ;)"), "{m}");
}

#[test]
fn unclosed_block_reports_the_opening_brace() {
    let src = "fn f() {\n  let a = 1;\n";
    let p = parse(src);
    assert_eq!(p.diags.error_count(), 1);
    let out = p.rendered(src);
    assert!(out.contains("error[E0105]: unclosed delimiter `{`"), "{out}");
    assert!(out.contains("1 | fn f() {"), "{out}");
    // what was parsed so far survives
    assert!(p.module().contains("(let a 1)"), "{}", p.module());
}

#[test]
fn unmatched_close_brace_is_skipped() {
    let p = parse("let a = 1; } let b = 2;");
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    assert_eq!(p.module(), "(module (let a 1) <error> (let b 2))");
}

#[test]
fn errors_are_not_cascaded_at_one_token() {
    let p = parse("let = 5;");
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    let p = parse("1 + ;");
    assert_eq!(p.diags.error_count(), 1);
}

#[test]
fn lexer_errors_are_not_reported_twice() {
    let p = parse("let a = 'ab';\nlet b = 1;");
    assert_eq!(p.diags.error_count(), 1, "{:?}", p.messages());
    assert!(p.module().ends_with("(let b 1))"));
}

#[test]
fn specific_error_messages() {
    let cases: &[(&str, &str)] = &[
        ("let x = ;", "expected expression, found `;`"),
        ("let = 1;", "expected pattern, found `=`"),
        ("let x: = 1;", "expected type, found `=`"),
        ("fn () {}", "expected identifier, found `(`"),
        ("fn f(x) {}", "expected `:`, found `)`"),
        ("f(1, 2", "expected `)`, found end of file"),
        ("a[1;", "expected `]`, found `;`"),
        ("import foo;", "expected `{`, found `foo`"),
        ("import { a } \"m\";", "expected `from`, found `\"m\"`"),
        ("import { a } from m;", "expected a module path string, found `m`"),
        ("export let x = 1;", "expected a declaration after `export`, found keyword `let`"),
        ("1 = 2;", "invalid left-hand side of assignment"),
        ("f() = 2;", "invalid left-hand side of assignment"),
        ("a < b < c;", "comparison operators cannot be chained"),
        ("match x { 1 2 }", "expected `=>`, found `2`"),
        ("match x { 1 => a 2 => b }", "expected `,` after match arm, found `2`"),
        ("struct S { a }", "expected `:`, found `}`"),
        ("enum E { 1 }", "expected identifier, found `1`"),
        ("impl T { let x; }", "expected `fn` or `}`, found keyword `let`"),
        ("for x xs {}", "expected `in`, found `xs`"),
    ];
    for (src, expect) in cases {
        let p = parse(src);
        assert!(
            p.messages().iter().any(|m| m.contains(expect)),
            "{src:?}: expected message containing {expect:?}, got {:?}",
            p.messages()
        );
    }
}

#[test]
fn rendered_syntax_error_has_codes_and_locations() {
    let src = "let a = 1;\nlet b = ;\n";
    let p = parse(src);
    let out = p.rendered(src);
    assert_eq!(
        out,
        "error[E0102]: expected expression, found `;`\n\
         \x20--> t.neon:2:9\n\
         \x20 |\n\
         2 | let b = ;\n\
         \x20 |         ^ expected expression\n\
         \n\
         error: aborting due to 1 previous error\n"
    );
}

#[test]
fn always_terminates_and_yields_a_module_on_garbage() {
    // Deterministic fuzz: random token soup must not hang, panic, or lose the root.
    const PIECES: &[&str] = &[
        "let", "fn", "x", "y", "1", "2.5", "\"s\"", "'c'", "(", ")", "{", "}", "[", "]", ",", ";", ":", "::", ".",
        "..", "=", "==", "=>", "->", "+", "-", "*", "|", "||", "&&", "<", ">", ">>", "match", "if", "else", "while",
        "for", "in", "return", "enum", "struct", "impl", "trait", "import", "export", "from", "as", "mut", "_", "set",
        "?", "true", "i32", "Self", "self", "const", "break", "loop", "\n",
    ];
    let mut state = 0x2545F4914F6CDD1Du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..4000 {
        let n = (next() % 60) as usize;
        let src: String = (0..n).map(|_| PIECES[(next() % PIECES.len() as u64) as usize]).collect::<Vec<_>>().join(" ");
        let p = parse(&src);
        assert!(matches!(p.ast.kind(p.ast.root()), NodeKind::Module { .. }), "{src:?}");
        for (id, node) in p.ast.iter() {
            assert!(node.span.lo <= node.span.hi && node.span.hi as usize <= src.len(), "{src:?} node {id:?}");
        }
    }
}
