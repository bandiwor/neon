use super::*;
use crate::diag::{DiagBag, Emitter, Severity, SourceMap};
use crate::syntax::Interner;
use crate::vfs::MemVfs;

struct Out {
    prog: Program,
    interner: Interner,
    diags: DiagBag,
    map: SourceMap,
}

fn run_files(main: &str, files: &[(&str, &str)]) -> Out {
    let mut vfs = MemVfs::new();
    for (p, t) in files {
        vfs.insert(p, t);
    }
    let mut map = SourceMap::new();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let prog = analyze_source("main.neon", main, &vfs, &mut map, &mut interner, &mut diags);
    Out { prog, interner, diags, map }
}

fn run(main: &str) -> Out {
    run_files(main, &[])
}

impl Out {
    fn rendered(&self) -> String {
        Emitter::new(&self.map).render_all(&self.diags)
    }

    fn entry(&self) -> &Module {
        self.prog.entry().unwrap()
    }

    /// Display type of the entry module's global `name`.
    fn global(&self, name: &str) -> String {
        let sym = self.interner.get(name).unwrap_or_else(|| panic!("no symbol {name}"));
        match self.entry().scope.items.get(&sym) {
            Some(Item::Global(g)) => self.prog.show_ty(&self.interner, self.prog.global(*g).ty, &Generics::default()),
            other => panic!("`{name}` is not a global: {other:?}"),
        }
    }

    fn has(&self, code: &str, fragment: &str) -> bool {
        self.diags.iter().any(|d| d.code == Some(code) && d.message.contains(fragment))
    }
}

/// Asserts the program analyzes without any diagnostics.
fn ok(src: &str) -> Out {
    let out = run(src);
    assert!(out.diags.is_empty(), "unexpected diagnostics:\n{}", out.rendered());
    out
}

/// Asserts exactly the given diagnostic appears.
fn fails(src: &str, code: &str, fragment: &str) -> Out {
    let out = run(src);
    assert!(out.has(code, fragment), "expected {code} containing {fragment:?}, got:\n{}", out.rendered());
    out
}

#[test]
fn prelude_analyzes_cleanly() {
    let out = ok("");
    assert_eq!(out.prog.modules.len(), 2);
    for (kind, name) in LangTrait::ALL {
        assert!(out.prog.lang.get(kind).is_some(), "lang trait {name} missing");
    }
    // trait methods carry an implicit `Self: Trait` bound
    let eq = out.prog.lang.get(LangTrait::Eq).unwrap();
    let ne = out.prog.traits[eq.index()].methods[1];
    assert_eq!(out.prog.fns[ne.index()].generics.params[0].bounds[0].trait_id, eq);
}

#[test]
fn literals_default_and_annotate() {
    let o = ok("let a = 1; let b = 2.5; let c = \"s\"; let d = 'c'; let e = true; let f = 7u8; let g: i64 = 3; let h = 1.5f32; let u = ();");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "f64");
    assert_eq!(o.global("c"), "string");
    assert_eq!(o.global("d"), "char");
    assert_eq!(o.global("e"), "bool");
    assert_eq!(o.global("f"), "u8");
    assert_eq!(o.global("g"), "i64");
    assert_eq!(o.global("h"), "f32");
    assert_eq!(o.global("u"), "()");
}

#[test]
fn integer_literals_default_to_i64_and_floats_to_f64() {
    let o = ok("let a = 1; let b = 2.5; let c = [1, 2]; let d = 7.max(3); let e = 1.5.max(2.5);");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "f64");
    assert_eq!(o.global("c"), "[i64]");
    assert_eq!(o.global("d"), "i64");
    assert_eq!(o.global("e"), "f64");
}

#[test]
fn integer_literals_become_floats_where_a_float_is_expected() {
    let o = ok("fn half(x: f64) -> f64 { x / 2 }\nstruct P { x: f32 }\n\
        let a: f64 = 15; let b: f32 = -4; let c = half(9); let d = P { x: 3 }; let e = [1.5, 2];\n\
        var f: f64 = 1.5; f += 1; f = 7; let g = 2 * f; let h = 1 + f * 3; let i = 3 < f; let j = f == 2; let k: f64 = if true { 1 } else { 2 };");
    assert_eq!(o.global("a"), "f64");
    assert_eq!(o.global("b"), "f32");
    assert_eq!(o.global("c"), "f64");
    assert_eq!(o.global("e"), "[f64]");
    assert_eq!(o.global("g"), "f64");
    assert_eq!(o.global("h"), "f64");
    assert_eq!(o.global("i"), "bool");
    assert_eq!(o.global("k"), "f64");
}

#[test]
fn only_literals_convert_not_integer_values() {
    fails("let n = 1; let x: f64 = n;", "E0211", "expected `f64`, found `{integer}`");
    fails("let n: i64 = 1; let x: f64 = n;", "E0211", "expected `f64`, found `i64`");
    fails("let n: i64 = 1; let x = 2.5 * n;", "E0211", "");
    fails("let x: i64 = 1.0;", "E0211", "expected `i64`");
    fails("let x: f64 = 1u8;", "E0211", "expected `f64`, found `u8`");
    fails("let x = 2 * 1.5 << 1;", "E0236", "");
}

#[test]
fn arithmetic_infers_through_operands() {
    let o = ok("let a = 1 + 2 * 3; let b: i64 = 1 + 2; let x = 5u8; let c = x + 1; let d = 1.5 * 2.0; let e = 10 % 3 << 1; let f = !true; let g = -5;");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "i64");
    assert_eq!(o.global("c"), "u8");
    assert_eq!(o.global("d"), "f64");
    assert_eq!(o.global("e"), "i64");
    assert_eq!(o.global("f"), "bool");
    assert_eq!(o.global("g"), "i64");
}

#[test]
fn comparisons_and_logic() {
    let o = ok("let a = 1 < 2; let b = \"a\" == \"b\"; let c = 1.0 >= 2.0 && !(3 != 4) || true; let d = 'a' < 'b';");
    for n in ["a", "b", "c", "d"] {
        assert_eq!(o.global(n), "bool");
    }
}

#[test]
fn functions_and_calls() {
    let o = ok("fn add(a: i32, b: i32) -> i32 { a + b }\nfn nothing() { }\nlet r = add(1, 2); let u = nothing();");
    assert_eq!(o.global("r"), "i32");
    assert_eq!(o.global("u"), "()");
}

#[test]
fn recursion_and_forward_references() {
    ok("fn fact(n: u64) -> u64 { if n <= 1 { 1 } else { n * fact(n - 1) } }\n\
        fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\
        fn even(n: i32) -> bool { if n == 0 { true } else { odd(n - 1) } }\n\
        fn odd(n: i32) -> bool { if n == 0 { false } else { even(n - 1) } }\n\
        let a = fact(5); let b = fib(10);");
}

#[test]
fn generic_functions_are_instantiated_per_call() {
    let o = ok("fn id<T>(x: T) -> T { x }\nlet a = id(5); let b = id(\"s\"); let c = id([1, 2]); let d = id::<u8>(1);");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "string");
    assert_eq!(o.global("c"), "[i64]");
    assert_eq!(o.global("d"), "u8");
}

#[test]
fn structs_fields_and_methods() {
    let o = ok("struct P { x: i32, y: i32 }\n\
        impl P {\n  fn new(x: i32, y: i32) -> P { P { x: x, y: y } }\n  fn sum(self) -> i32 { self.x + self.y }\n  fn shift(var self, d: i32) { self.x += d; }\n}\n\
        var p = P::new(1, 2); let s = p.sum(); p.shift(3); p.y = 9; let q = P { x: 1, y: 2 }; let n = q.x;");
    assert_eq!(o.global("p"), "P");
    assert_eq!(o.global("s"), "i32");
    assert_eq!(o.global("n"), "i32");
}

#[test]
fn self_type_and_struct_shorthand() {
    ok("struct Counter { n: i64 }\nimpl Counter { fn new() -> Self { Self { n: 0 } } fn get(self) -> i64 { self.n } }\n\
        let c = Counter::new(); let v = c.get(); let n = 5i64; let d = Counter { n };");
}

#[test]
fn enums_and_match_with_payloads() {
    let o = ok("enum Shape { Circle(f64), Rect(f64, f64), Empty }\n\
        fn area(s: Shape) -> f64 { match s { Shape::Circle(r) => 3.14 * r * r, Shape::Rect(w, h) => w * h, Shape::Empty => 0.0 } }\n\
        let a = area(Shape::Rect(2.0, 3.0)); let e = Shape::Empty;");
    assert_eq!(o.global("a"), "f64");
    assert_eq!(o.global("e"), "Shape");
}

#[test]
fn generic_enum_option() {
    let o = ok("enum Option<T> { Some(T), None }\n\
        fn unwrap_or<T>(o: Option<T>, d: T) -> T { match o { Option::Some(x) => x, Option::None => d } }\n\
        let a = unwrap_or(Option::Some(5), 0); let b: Option<string> = Option::None; let c = Option::Some(Option::Some(1u8));");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "Option<string>");
    assert_eq!(o.global("c"), "Option<Option<u8>>");
}

#[test]
fn nested_patterns_guards_and_or() {
    ok("enum Option<T> { Some(T), None }\n\
        fn f(o: Option<Option<i32>>) -> i32 { match o { Option::Some(Option::Some(x)) if x > 0 => x, Option::Some(Option::Some(_)) => 0, Option::Some(Option::None) => -1, Option::None => -2 } }\n\
        fn g(n: i32) -> string { match n { 0 | 1 => \"small\", 2..=9 => \"mid\", _ => \"big\" } }\n\
        fn h(c: char) -> bool { match c { 'a'..='z' => true, _ => false } }\n\
        fn k(b: bool, n: i32) -> i32 { match b { true => n, false => 0 } }");
}

#[test]
fn traits_and_bounds() {
    let o = ok("trait Shape { fn area(self) -> f64; fn name(self) -> string { \"shape\" } }\n\
        struct Sq { s: f64 }\nstruct Circle { r: f64 }\n\
        impl Shape for Sq { fn area(self) -> f64 { self.s * self.s } }\n\
        impl Shape for Circle { fn area(self) -> f64 { 3.14 * self.r * self.r } fn name(self) -> string { \"circle\" } }\n\
        fn total<T: Shape>(x: T, y: T) -> f64 { x.area() + y.area() }\n\
        let t = total(Sq { s: 1.0 }, Sq { s: 2.0 }); let n = Sq { s: 1.0 }.name(); let a = Circle { r: 1.0 }.area();");
    assert_eq!(o.global("t"), "f64");
    assert_eq!(o.global("n"), "string");
}

#[test]
fn generic_trait_static_calls() {
    ok("trait Zero { fn zero() -> Self; }\n\
        impl Zero for i32 { fn zero() -> i32 { 0 } }\n\
        fn make<T: Zero>() -> T { T::zero() }\n\
        let a: i32 = make(); let b = i32::zero(); let c = Zero::zero();\nlet d: i32 = c;");
}

#[test]
fn impls_on_generic_types_with_bounds() {
    ok("trait Show { fn show(self) -> string; }\n\
        impl Show for i32 { fn show(self) -> string { \"int\" } }\n\
        enum Option<T> { Some(T), None }\n\
        impl<T: Show> Show for Option<T> { fn show(self) -> string { match self { Option::Some(x) => x.show(), Option::None => \"none\" } } }\n\
        let a = Option::Some(5).show(); let b = Option::Some(Option::Some(1)).show();");
}

#[test]
fn operator_overloading_via_traits() {
    let o = ok("struct V { x: f64, y: f64 }\n\
        impl Add<V> for V { fn add(self, r: V) -> V { V { x: self.x + r.x, y: self.y + r.y } } }\n\
        impl Mul<f64> for V { fn mul(self, k: f64) -> V { V { x: self.x * k, y: self.y * k } } }\n\
        impl Mul<V> for V { fn mul(self, o: V) -> V { V { x: self.x * o.x, y: self.y * o.y } } }\n\
        impl Neg for V { fn neg(self) -> V { V { x: -self.x, y: -self.y } } }\n\
        impl Eq for V { fn eq(self, o: V) -> bool { self.x == o.x && self.y == o.y } }\n\
        impl Ord for V { fn lt(self, o: V) -> bool { self.x < o.x } }\n\
        let a = V { x: 1.0, y: 2.0 }; let b = a + a; let c = a * 2.0; let d = a * b; let e = -a; let f = a == b; let g = a != b; let h = a <= b; let i = a > b;");
    assert_eq!(o.global("b"), "V");
    assert_eq!(o.global("c"), "V");
    assert_eq!(o.global("f"), "bool");
    assert_eq!(o.global("i"), "bool");
}

#[test]
fn compound_assignment_with_overloaded_operator() {
    ok("struct V { x: i32 }\nimpl Add<V> for V { fn add(self, r: V) -> V { V { x: self.x + r.x } } }\nlet var a = V { x: 1 }; a += V { x: 2 };");
}

#[test]
fn generic_functions_use_operator_bounds() {
    ok("fn sum<T: Add<T>>(a: T, b: T) -> T { a + b }\nlet x = sum(1, 2); let y = sum(1.5, 2.5); let z = sum(\"a\", \"b\");");
}

#[test]
fn closures_infer_and_capture() {
    let o = ok("let add = |a: i32, b: i32| => a + b; let r = add(1, 2);\n\
        let inc = |x| => x + 1; let s = inc(5);\n\
        fn apply(f: fn(i32) -> i32, v: i32) -> i32 { f(v) }\n\
        let t = apply(|x| => x * 2, 21);\n\
        fn make_counter() -> fn() -> i32 { var n = 0; || => { n += 1; n } }\n\
        let c = make_counter(); let v = c();");
    assert_eq!(o.global("r"), "i32");
    assert_eq!(o.global("s"), "i64");
    assert_eq!(o.global("t"), "i32");
    assert_eq!(o.global("c"), "fn() -> i32");
    // the counter closure captures `n`, which is marked captured
    let captured: Vec<_> = o.prog.locals.iter().filter(|l| l.captured).collect();
    assert_eq!(captured.len(), 1);
    assert_eq!(o.interner.resolve(captured[0].name), "n");
    let closures = &o.entry().info.closures;
    assert!(closures.values().any(|c| c.captures.len() == 1));
}

#[test]
fn nested_closures_capture_transitively() {
    let o = ok("fn f() -> i32 { let x = 1; let g = || => { let h = || => x + 1; h() }; g() }");
    let closures = &o.entry().info.closures;
    assert_eq!(closures.len(), 2);
    assert!(closures.values().all(|c| c.captures.len() == 1), "{closures:?}");
}

#[test]
fn collections() {
    let o = ok("var a = [1, 2, 3]; a.push(4); let n = a.len(); let first = a[0];\n\
        var m = { \"a\": 1, \"b\": 2 }; m.insert(\"c\", 3); let v = m[\"a\"]; let has = m.contains_key(\"a\"); let ks = m.keys();\n\
        var s = set { 1, 2, 3 }; s.insert(4); let c = s.contains(2);\n\
        let empty: [string] = []; let em: {string: i32} = {};\n\
        var total = 0; for x in a { total += x; } for i in 0..10 { total += i; } for k in m { total += m[k]; } for e in s { total += e; }");
    assert_eq!(o.global("a"), "[i64]");
    assert_eq!(o.global("n"), "i64");
    assert_eq!(o.global("m"), "{string: i64}");
    assert_eq!(o.global("ks"), "[string]");
    assert_eq!(o.global("s"), "set<i64>");
    assert_eq!(o.global("empty"), "[string]");
}

#[test]
fn builtin_methods_and_constants() {
    let o = ok("let a = i32::MAX; let b = u8::MIN; let c = f64::EPSILON; let d = 2.0.sqrt(); let e = \"abc\".len(); let f = (5).pow(2); let g = 'x'.is_alphabetic();\n\
        let h = \"a,b\".split(\",\"); let i = 3.to_string(); let j = (-4).abs(); let k = 2.5f32.floor(); let l = 1.max(2);");
    assert_eq!(o.global("a"), "i32");
    assert_eq!(o.global("b"), "u8");
    assert_eq!(o.global("c"), "f64");
    assert_eq!(o.global("e"), "i64");
    assert_eq!(o.global("h"), "[string]");
    assert_eq!(o.global("k"), "f32");
}

#[test]
fn printing_and_builtin_functions() {
    ok("print(1); println(\"x\"); println(); println([1, 2]); let line = input(); assert(1 < 2); assert(true, \"msg\");\n\
        fn die() -> i32 { panic(\"boom\") }");
}

#[test]
fn casts() {
    let o = ok("let a = 5 as i64; let b = 3.9 as i32; let c = 65u8 as char; let d = 'a' as u32; let e = true as i32; let f = -1 as u8; let g = 1 as f64;");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("c"), "char");
}

#[test]
fn control_flow_types() {
    let o =
        ok("let a = if true { 1 } else { 2 };\nlet b = loop { break 5; };\nlet var i = 0; while i < 3 { i += 1; }\n\
        fn f(n: i32) -> i32 { if n > 0 { return 1; } 2 }\n\
        fn g(n: i32) -> i32 { loop { if n > 0 { break n; } } }\n\
        let c = { let x = 1; x + 1 };");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "i64");
    assert_eq!(o.global("c"), "i64");
}

#[test]
fn diverging_branches_unify_with_anything() {
    ok("fn f(x: i32) -> i32 { let y = if x > 0 { x } else { return 0 }; y }\nfn g(x: i32) -> i32 { match x { 0 => panic(\"zero\"), n => n } }");
}

#[test]
fn shadowing_and_scopes() {
    let o = ok("let x = 1; fn f() -> string { let x = \"s\"; let x = x; x }\nlet y = { let x = 2.5; x };");
    assert_eq!(o.global("x"), "i64");
    assert_eq!(o.global("y"), "f64");
}

#[test]
fn globals_are_visible_to_functions() {
    ok("var counter = 0; const LIMIT: i32 = 10;\nfn bump() { counter += 1; }\nfn limit() -> i32 { LIMIT }\nbump();");
}

#[test]
fn modules_import_and_alias() {
    let main = "import { add, Point, ORIGIN } from \"lib/math\";\nimport \"lib/math\" as m;\n\
        let a = add(1, 2); let p = Point { x: 1, y: 2 }; let b = m::add(3, 4); let q = m::Point { x: 0, y: 0 }; let s = p.sum(); let o = ORIGIN;";
    let lib = "export fn add(a: i32, b: i32) -> i32 { a + b }\nexport struct Point { x: i32, y: i32 }\n\
        impl Point { fn sum(self) -> i32 { self.x + self.y } }\nexport const ORIGIN: i32 = 0;\nfn private() {}";
    let out = run_files(main, &[("lib/math.neon", lib)]);
    assert!(out.diags.is_empty(), "{}", out.rendered());
    assert_eq!(out.global("a"), "i32");
    assert_eq!(out.global("s"), "i32");
    assert_eq!(out.global("o"), "i32");
}

#[test]
fn imports_rename_with_as() {
    let out = run_files(
        "import { add as plus } from \"m\";\nlet r = plus(1, 2);",
        &[("m.neon", "export fn add(a: i32, b: i32) -> i32 { a + b }")],
    );
    assert!(out.diags.is_empty(), "{}", out.rendered());
}

#[test]
fn private_items_cannot_be_imported() {
    let out = run_files("import { hidden } from \"m\";", &[("m.neon", "fn hidden() {}")]);
    assert!(out.has("E0304", "no exported item `hidden`"), "{}", out.rendered());
}

#[test]
fn symbols_link_uses_to_definitions() {
    let src = "fn target() -> i32 { 1 }\nlet a = target();\nlet b = target();";
    let o = ok(src);
    let file = o.entry().file;
    let def_off = src.find("target").unwrap() as u32;
    let use_off = src.find("target()").unwrap() as u32
        + src[src.find("target()").unwrap()..].find("= target").map(|_| 0).unwrap_or(0);
    let _ = use_off;
    let first_use = src[def_off as usize + 1..].find("target").unwrap() as u32 + def_off + 1;
    let d_def = o.prog.symbols.def_at(file, def_off).unwrap();
    let d_use = o.prog.symbols.def_at(file, first_use).unwrap();
    assert_eq!(d_def, d_use);
    assert_eq!(o.prog.symbols.uses_of(d_def).count(), 2);
    assert_eq!(o.prog.symbols.def(d_def).span.lo, def_off);
}

#[test]
fn locals_link_uses_to_bindings() {
    let src = "fn f(x: i32) -> i32 { let y = x + 1; y * x }";
    let o = ok(src);
    let file = o.entry().file;
    let x_def = src.find("x:").unwrap() as u32;
    let d = o.prog.symbols.def_at(file, x_def).unwrap();
    assert_eq!(o.prog.symbols.uses_of(d).count(), 2);
}

// ---- diagnostics ----------------------------------------------------------------------------

#[test]
fn mismatched_types() {
    fails("let x: i32 = \"a\";", "E0211", "expected `i32`, found `string`");
    fails("fn f() -> i32 { \"s\" }", "E0211", "expected `i32`, found `string`");
    fails("let x = 1 + \"a\";", "E0211", "found `string`");
    fails("let n = 1; let x: f64 = n;", "E0211", "expected `f64`, found `{integer}`");
    fails("fn f(a: i32) {} f(true);", "E0211", "expected `i32`, found `bool`");
    fails("let a = [1, \"x\"];", "E0211", "");
    fails("let a = if true { 1 } else { \"s\" };", "E0211", "");
    fails("fn f() -> i32 { }", "E0211", "found `()`");
    fails("let x = 5u8 + 5u16;", "E0211", "");
}

#[test]
fn undefined_names() {
    fails("let x = y;", "E0210", "cannot find value `y`");
    fails("let x: Foo = 1;", "E0202", "cannot find type `Foo`");
    fails("let x = nope(1);", "E0210", "cannot find value `nope`");
    fails("let x = Foo::bar();", "E0210", "cannot find `Foo`");
    fails("enum E { A }\nlet x = E::B;", "E0214", "no associated item named `B`");
}

#[test]
fn wrong_argument_counts() {
    fails("fn f(a: i32) {} f();", "E0216", "takes 1 argument(s) but 0");
    fails("fn f(a: i32) {} f(1, 2);", "E0216", "takes 1 argument(s) but 2");
    fails("enum E { A(i32) }\nlet x = E::A(1, 2);", "E0216", "");
    fails("let a = [1]; a.len(1);", "E0216", "");
}

#[test]
fn unknown_methods_and_fields() {
    fails("let a = 5; a.frobnicate();", "E0214", "no method named `frobnicate`");
    fails("struct P { x: i32 }\nlet p = P { x: 1 }; let y = p.z;", "E0217", "no field `z`");
    fails("let a = 5; let b = a.x;", "E0217", "no field `x`");
    fails("let a = [1]; a.nope();", "E0214", "");
}

#[test]
fn mutability_is_enforced() {
    fails("let x = 1; x = 2;", "E0220", "cannot assign to immutable");
    fails("let x = 1; x += 2;", "E0220", "");
    fails("fn f(x: i32) { x = 1; }", "E0220", "");
    fails("struct P { x: i32 }\nlet p = P { x: 1 }; p.x = 2;", "E0220", "");
    fails("let a = [1]; a.push(2);", "E0220", "");
    fails("let a = [1]; a[0] = 2;", "E0220", "");
    fails(
        "struct C { n: i32 }\nimpl C { fn bump(var self) { self.n += 1; } }\nlet c = C { n: 0 }; c.bump();",
        "E0220",
        "",
    );
    fails("struct C { n: i32 }\nimpl C { fn bump(self) { self.n += 1; } }", "E0220", "");
    fails("const K = 1; fn f() { K = 2; }", "E0220", "");
    fails("let x = 1; let f = || => { x = 2; };", "E0220", "");
    ok("var x = 1; x = 2; var a = [1]; a.push(2); a[0] = 5; var m = {\"a\": 1}; m[\"a\"] = 2;");
}

#[test]
fn literal_ranges() {
    fails("let x: u8 = 256;", "E0235", "out of range for `u8`");
    fails("let x: i8 = 128;", "E0235", "");
    fails("let x = 300u8;", "E0235", "");
    fails("let x: u8 = -1;", "E0235", "cannot negate an unsigned");
    fails("let x = 9223372036854775808;", "E0235", "out of range for `i64`");
    fails("let x: f32 = 1e100;", "E0235", "out of range for `f32`");
    ok("let a: i8 = -128; let b: i8 = 127; let c: u64 = 18446744073709551615; let d: i64 = -9223372036854775808; let e: u8 = 255; let f: i16 = -32768;");
}

#[test]
fn invalid_operators_and_casts() {
    fails("let x = true + false;", "E0236", "cannot apply operator `+` to type `bool`");
    fails("let x = \"a\" - \"b\";", "E0236", "");
    fails("let x = 1.5 % 2.0 & 1;", "E0236", "");
    fails("let x = 1.5 << 1;", "E0236", "");
    fails("let x = -true;", "E0236", "");
    fails("let a: u8 = 1; let x = -a;", "E0236", "");
    fails("let x = [1] == [1];", "E0237", "Eq");
    fails("let x = true < false;", "E0219", "");
    fails("let x = \"s\" as i32;", "E0215", "cannot cast `string` as `i32`");
    fails("let x = 5 as bool;", "E0215", "");
    fails("let x = 1 && 2;", "E0211", "");
    fails("let x = [1] + [2];", "E0237", "");
}

#[test]
fn struct_literal_errors() {
    fails("struct P { x: i32, y: i32 }\nlet p = P { x: 1 };", "E0226", "missing field(s) `y`");
    fails("struct P { x: i32 }\nlet p = P { x: 1, z: 2 };", "E0227", "has no field named `z`");
    fails("struct P { x: i32 }\nlet p = P { x: 1, x: 2 };", "E0228", "");
    fails("struct P { x: i32 }\nlet p = P { x: \"s\" };", "E0211", "");
    fails("let p = Nope { x: 1 };", "E0210", "cannot find struct");
}

#[test]
fn trait_errors() {
    fails("trait T { fn f(self); }\nstruct S;\nimpl T for S { }", "E0207", "missing trait method `f`");
    fails(
        "trait T { fn f(self); }\nstruct S;\nimpl T for S { fn f(self) {} fn g(self) {} }",
        "E0208",
        "not a member of trait",
    );
    fails(
        "trait T { fn f(self) -> i32; }\nstruct S;\nimpl T for S { fn f(self) -> string { \"\" } }",
        "E0206",
        "incompatible type",
    );
    fails("trait T { fn f(self); }\nfn g<X: T>(x: X) {}\ng(5);", "E0237", "`i64: T`");
    fails("trait T { fn f(self); }\nstruct S;\nlet x = S { }; x.f();", "E0214", "no method named `f`");
    fails("struct S;\nimpl Nope for S { }", "E0204", "cannot find trait `Nope`");
    fails("struct V { x: i32 }\nlet a = V { x: 1 }; let b = a + a;", "E0237", "`V: Add");
}

#[test]
fn duplicate_definitions() {
    fails("fn f() {}\nfn f() {}", "E0201", "defined multiple times");
    fails("struct A;\nstruct A;", "E0201", "");
    fails("enum E { A, A }", "E0201", "variant `A`");
    fails("let x = 1; fn x() {}", "E0201", "");
}

#[test]
fn control_flow_errors() {
    fails("break;", "E0232", "outside of a loop");
    fails("continue;", "E0232", "");
    fails("while true { break 1; }", "E0232", "only allowed in `loop`");
    fails("return 1;", "E0221", "outside of a function");
    fails("fn f() -> i32 { return; }", "E0221", "without a value");
    fails("let x = 1 if true { 2 }", "E0101", "");
}

#[test]
fn let_and_pattern_errors() {
    fails("let x;", "E0229", "must be initialized");
    fails("enum E { A, B }\nfn f() { let E::A = E::A; }", "E0230", "refutable pattern");
    fails("let (a) = 1; let 5 = 5;", "E0230", "must bind a single name");
    fails("let (a, b) = 1;", "E0104", "");
    fails("enum E { A(i32) }\nfn f(e: E) { match e { E::A => {} } }", "E0233", "");
    fails("fn f(x: i32) { match x { 1 | y => {} _ => {} } }", "E0234", "");
    fails("let v = [];", "E0212", "type annotations needed");
}

#[test]
fn nested_declarations_and_imports_in_blocks() {
    fails("fn f() { fn g() {} }", "E0223", "only allowed at the top level");
    fails("fn f() { struct S; }", "E0223", "");
    fails("fn f() { import { a } from \"x\"; }", "E0303", "");
}

#[test]
fn iteration_and_ranges() {
    fails("for c in \"abc\" { }", "E0231", "cannot iterate over");
    fails("let r = 0..5;", "E0218", "ranges can only be used");
    fails("for i in 5.. { }", "E0218", "");
    ok("for c in \"abc\".chars() { } for i in 0..=3 { } let n = 5i64; for i in 0..n { }");
}

#[test]
fn non_exhaustive_matches() {
    let o = fails(
        "enum E { A, B, C }\nfn f(e: E) -> i32 { match e { E::A => 1, E::B => 2 } }",
        "E0240",
        "`E::C` not covered",
    );
    assert!(o.rendered().contains("E0240"));
    fails(
        "enum Option<T> { Some(T), None }\nfn f(o: Option<i32>) { match o { Option::Some(x) => {} } }",
        "E0240",
        "`Option::None` not covered",
    );
    fails("enum Option<T> { Some(T), None }\nfn f(o: Option<Option<i32>>) { match o { Option::Some(Option::Some(_)) => {} Option::None => {} } }", "E0240", "Option::Some(Option::None)");
    fails("fn f(n: i32) { match n { 0 => {} 1 => {} } }", "E0240", "`_` not covered");
    fails("fn f(b: bool) { match b { true => {} } }", "E0240", "`false` not covered");
    fails("enum E { A, B }\nfn f(e: E) { match e { E::A => {} E::B if false => {} } }", "E0240", "`E::B`");
    fails("fn f(s: string) { match s { \"a\" => {} } }", "E0240", "");
}

#[test]
fn exhaustive_matches_are_accepted() {
    ok("enum E { A, B(bool), C(E2) }\nenum E2 { X, Y }\n\
        fn f(e: E) -> i32 { match e { E::A => 1, E::B(true) => 2, E::B(false) => 3, E::C(E2::X) => 4, E::C(E2::Y) => 5 } }\n\
        fn g(e: E) -> i32 { match e { E::A | E::B(_) => 1, E::C(_) => 2 } }\n\
        fn h(n: i32) -> i32 { match n { 0 => 1, x => x } }\n\
        fn k(e: E) -> i32 { match e { E::A => 1, other => 2 } }");
}

#[test]
fn unreachable_arms_warn() {
    let o = run("enum E { A, B }\nfn f(e: E) -> i32 { match e { E::A => 1, E::B => 2, E::A => 3 } }\nfn g(n: i32) -> i32 { match n { _ => 1, 5 => 2 } }");
    let warnings: Vec<_> = o.diags.iter().filter(|d| d.severity == Severity::Warning).collect();
    assert_eq!(warnings.len(), 2, "{}", o.rendered());
    assert_eq!(o.diags.error_count(), 0, "{}", o.rendered());
}

#[test]
fn guards_do_not_make_arms_exhaustive_but_are_checked() {
    ok("fn f(n: i32) -> i32 { match n { x if x > 0 => 1, x if x < 0 => -1, _ => 0 } }");
    fails("fn f(n: i32) { match n { x if 5 => {} _ => {} } }", "E0211", "");
}

#[test]
fn module_errors_surface() {
    let o = run("import { x } from \"missing\";");
    assert!(o.has("E0301", "cannot find module"), "{}", o.rendered());
    let o = run_files(
        "import { a } from \"a\";",
        &[
            ("a.neon", "import { b } from \"b\";\nexport fn a() {}"),
            ("b.neon", "import { a } from \"a\";\nexport fn b() {}"),
        ],
    );
    assert!(o.has("E0302", "circular import"), "{}", o.rendered());
}

#[test]
fn generic_arity_errors() {
    fails("struct W<T> { v: T }\nlet x: W = 1;", "E0203", "takes 1 generic argument");
    fails("struct W { v: i32 }\nlet x: W<i32> = 1;", "E0203", "takes no generic arguments");
    fails("fn f<T>(x: T) {}\nf::<i32, i32>(1);", "E0203", "");
}

#[test]
fn self_misuse() {
    fails("let x: Self = 1;", "E0205", "only available in impls");
    fails("fn f(self) {}", "E0209", "");
    fails("struct S;\nimpl S { fn f(a: i32, self) {} }", "E0209", "first parameter");
}

#[test]
fn errors_do_not_cascade_wildly() {
    // one unknown name should produce one error, not a chain
    let o = run("let a = nope; let b = a + 1; let c = b * 2;");
    assert_eq!(o.diags.error_count(), 1, "{}", o.rendered());
}

#[test]
fn many_independent_errors_are_all_reported() {
    let o = run("let a: i32 = \"x\";\nlet b = undefined;\nfn f() -> bool { 1 }\nlet c = 1.foo();");
    assert!(o.diags.error_count() >= 4, "{}", o.rendered());
}

#[test]
fn recursive_types_through_generics() {
    ok("enum List<T> { Cons(T, List<T>), Nil }\nfn len<T>(l: List<T>) -> i32 { match l { List::Cons(_, rest) => 1 + len(rest), List::Nil => 0 } }\n\
        let l = List::Cons(1, List::Cons(2, List::Nil)); let n = len(l);");
}

#[test]
fn trait_default_methods_use_required_ones() {
    ok("struct N { v: i32 }\nimpl Ord for N { fn lt(self, o: N) -> bool { self.v < o.v } }\n\
        let a = N { v: 1 }; let b = N { v: 2 }; let x = a < b; let y = a >= b; let z = b > a;");
}

#[test]
fn methods_on_primitives_via_user_impls() {
    let o = ok("trait Double { fn double(self) -> Self; }\nimpl Double for i64 { fn double(self) -> i64 { self * 2 } }\nimpl Double for f64 { fn double(self) -> f64 { self * 2.0 } }\n\
        impl i64 { fn triple(self) -> i64 { self * 3 } }\nlet a = 4.double(); let b = 1.5.double(); let c = 5i64.triple();");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "f64");
    assert_eq!(o.global("c"), "i64");
}

#[test]
fn display_trait_for_user_types() {
    ok("struct P { x: i32 }\nimpl Display for P { fn to_string(self) -> string { \"P\" } }\nlet s = P { x: 1 }.to_string(); let t = 5.to_string(); println(P { x: 2 });");
}

#[test]
fn closure_param_inferred_from_later_use() {
    let o = ok("let add = |a, b| => a + b; let r = add(1u8, 2);");
    assert_eq!(o.global("r"), "u8");
}

#[test]
fn generic_containers_with_generic_methods() {
    let o = ok("enum Option<T> { Some(T), None }\n\
        impl<T> Option<T> {\n\
            fn map<U>(self, f: fn(T) -> U) -> Option<U> { match self { Option::Some(x) => Option::Some(f(x)), Option::None => Option::None } }\n\
            fn is_some(self) -> bool { match self { Option::Some(_) => true, Option::None => false } }\n\
            fn unwrap_or(self, d: T) -> T { match self { Option::Some(x) => x, Option::None => d } }\n\
        }\n\
        struct Stack<T> { items: [T] }\n\
        impl<T> Stack<T> {\n\
            fn new() -> Stack<T> { Stack { items: [] } }\n\
            fn push(var self, x: T) { self.items.push(x); }\n\
            fn pop(var self) -> Option<T> { if self.items.is_empty() { Option::None } else { Option::Some(self.items.pop()) } }\n\
            fn size(self) -> i64 { self.items.len() }\n\
        }\n\
        let a = Option::Some(2).map(|x| => x * 2).unwrap_or(0);\n\
        let b = Option::Some(1).map(|x| => \"s\").is_some();\n\
        var st = Stack::new(); st.push(1u8); st.push(2); let top = st.pop(); let n = st.size();\n\
        var names = Stack::<string>::new(); names.push(\"x\");");
    assert_eq!(o.global("a"), "i64");
    assert_eq!(o.global("b"), "bool");
    assert_eq!(o.global("top"), "Option<u8>");
    assert_eq!(o.global("n"), "i64");
    assert_eq!(o.global("names"), "Stack<string>");
}

#[test]
fn function_typed_fields_and_higher_order_functions() {
    ok("struct Op { name: string, f: fn(i32, i32) -> i32 }\n\
        fn run(op: Op, a: i32, b: i32) -> i32 { (op.f)(a, b) }\n\
        fn compose(f: fn(i32) -> i32, g: fn(i32) -> i32) -> fn(i32) -> i32 { |x| => g(f(x)) }\n\
        fn map(xs: [i32], f: fn(i32) -> i32) -> [i32] { var out = []; for x in xs { out.push(f(x)); } out }\n\
        fn double(x: i32) -> i32 { x * 2 }\n\
        let add = Op { name: \"add\", f: |a, b| => a + b };\n\
        let r = run(add, 1, 2); let h = compose(double, |x| => x + 1); let v = h(5); let m = map([1, 2, 3], double);");
}

#[test]
fn self_calls_within_impls() {
    ok("struct Acc { total: i64 }\n\
        impl Acc {\n\
            fn new() -> Acc { Acc { total: 0 } }\n\
            fn add(var self, n: i64) -> Acc { self.total += n; self }\n\
            fn twice(var self, n: i64) { self.add(n); self.add(n); }\n\
            fn fresh() -> Acc { Self::new() }\n\
            fn get(self) -> i64 { self.total }\n\
        }\n\
        var a = Acc::fresh(); a.twice(5); let t = a.get();");
}

#[test]
fn trait_bounded_generic_collections() {
    ok("trait Scorer { fn score(self) -> i32; }\n\
        struct A { n: i32 }\nimpl Scorer for A { fn score(self) -> i32 { self.n } }\n\
        fn best<T: Scorer>(items: [T]) -> i32 { var b = 0; for it in items { if it.score() > b { b = it.score(); } } b }\n\
        fn total<T: Scorer>(items: [T]) -> i32 { var t = 0; for it in items { t += it.score(); } t }\n\
        let x = best([A { n: 3 }, A { n: 9 }]); let y = total([A { n: 1 }]);");
}

#[test]
fn bit_operations_and_shifts_across_widths() {
    ok("let a = 0xFFu8 & 0x0F; let b = 1u64 << 40; let c = 1i8 << 3u8; let d = 5u32 ^ 3; let e = !0u16; let f = ~7i32; let g = 255u8 | 1;");
}

/// All trait callees recorded in the entry module, in node order.
fn trait_callees(o: &Out) -> Vec<(FnId, Vec<Ty>)> {
    let mut v: Vec<_> = o
        .entry()
        .info
        .callees
        .iter()
        .filter_map(|(n, c)| if let Callee::Trait(f, a) = c { Some((*n, *f, a.clone())) } else { None })
        .collect();
    v.sort_by_key(|(n, _, _)| n.0);
    v.into_iter().map(|(_, f, a)| (f, a)).collect()
}

#[test]
fn mono_time_resolution_picks_the_impl() {
    let mut o = ok("trait Show { fn show(self) -> string; fn twice(self) -> string { \"twice\" } }\n\
        struct A { n: i32 }\nimpl Show for A { fn show(self) -> string { \"a\" } }\n\
        enum Option<T> { Some(T), None }\n\
        impl<T: Show> Show for Option<T> { fn show(self) -> string { \"opt\" } }\n\
        let a = A { n: 1 };\nlet s1 = a.show();\nlet s2 = Option::Some(A { n: 2 }).show();\nlet s3 = a.twice();\nlet s4 = 5.to_string();");
    let callees = trait_callees(&o);
    assert_eq!(callees.len(), 4);
    let owner_of = |prog: &Program, f: FnId| prog.fns[f.index()].owner;

    // a.show(): impl Show for A
    let (m, args) = callees[0].clone();
    let Some(Resolved::Fn(f, fargs)) = resolve_trait_method(&mut o.prog, m, &args) else { panic!() };
    assert!(matches!(owner_of(&o.prog, f), FnOwner::Impl(_)));
    assert!(fargs.is_empty());

    // Option::Some(A).show(): the generic impl, with T = A
    let (m, args) = callees[1].clone();
    let Some(Resolved::Fn(f, fargs)) = resolve_trait_method(&mut o.prog, m, &args) else { panic!() };
    let FnOwner::Impl(imp) = owner_of(&o.prog, f) else { panic!() };
    assert_eq!(o.prog.impl_def(imp).generics.len(), 1);
    assert_eq!(fargs.len(), 1);
    assert_eq!(o.prog.show_ty(&o.interner, fargs[0], &Generics::default()), "A");

    // a.twice(): not overridden, so the trait's default body with Self = A
    let (m, args) = callees[2].clone();
    let Some(Resolved::Fn(f, fargs)) = resolve_trait_method(&mut o.prog, m, &args) else { panic!() };
    assert_eq!(f, m);
    assert_eq!(fargs, args);

    // 5.to_string(): builtin Display for primitives
    let (m, args) = callees[3].clone();
    assert_eq!(resolve_trait_method(&mut o.prog, m, &args), Some(Resolved::BuiltinOp(LangTrait::Display)));
}

#[test]
fn mono_time_resolution_of_operators_in_generic_code() {
    let mut o = ok("fn sum<T: Add<T>>(a: T, b: T) -> T { a + b }\nstruct V { x: i32 }\nimpl Add<V> for V { fn add(self, r: V) -> V { V { x: self.x + r.x } } }");
    let sum = o.entry().scope.items[&o.interner.get("sum").unwrap()];
    let Item::Fn(_) = sum else { panic!() };
    let (m, args) = trait_callees(&o)[0].clone();
    // inside `sum`, the operand type is the generic parameter T
    assert!(o.prog.types.has_param(args[0]));
    let i32t = o.prog.types.prim(Prim::I32);
    assert_eq!(resolve_trait_method(&mut o.prog, m, &[i32t, i32t]), Some(Resolved::BuiltinOp(LangTrait::Add)));
    let v = o.entry().scope.items[&o.interner.get("V").unwrap()];
    let Item::Adt(vid) = v else { panic!() };
    let vt = o.prog.types.adt(vid, vec![]);
    let Some(Resolved::Fn(f, _)) = resolve_trait_method(&mut o.prog, m, &[vt, vt]) else { panic!() };
    assert!(matches!(o.prog.fns[f.index()].owner, FnOwner::Impl(_)));
    // no impl: unresolved
    let strt = o.prog.types.prim(Prim::Bool);
    assert_eq!(resolve_trait_method(&mut o.prog, m, &[strt, strt]), None);
}

#[test]
fn prelude_option_and_result_have_the_variant_order_the_vm_assumes() {
    use crate::runtime::value::{ERR_TAG, NONE_TAG, OK_TAG, SOME_TAG};
    let (prog, interner) = {
        let mut map = crate::diag::SourceMap::new();
        let mut interner = crate::syntax::Interner::new();
        let mut diags = crate::diag::DiagBag::new();
        let prog = crate::sema::analyze_source(
            "main.neon",
            "",
            &crate::vfs::MemVfs::new(),
            &mut map,
            &mut interner,
            &mut diags,
        );
        assert!(!diags.has_errors());
        (prog, interner)
    };
    let variant = |adt: crate::sema::AdtId, name: &str| {
        prog.adt(adt).variants().iter().position(|v| interner.resolve(v.name) == name).unwrap() as u32
    };
    let opt = prog.lang_adts.option.expect("Option");
    let res = prog.lang_adts.result.expect("Result");
    assert_eq!((variant(opt, "Some"), variant(opt, "None")), (SOME_TAG, NONE_TAG));
    assert_eq!((variant(res, "Ok"), variant(res, "Err")), (OK_TAG, ERR_TAG));
}

fn error_codes(o: &Out) -> Vec<&'static str> {
    o.diags.iter().filter_map(|d| d.code).collect()
}

#[test]
fn question_mark_typechecks_in_matching_functions() {
    let o = run("fn a(s: string) -> Option<i64> { let n = s.index_of(\"x\")?; Option::Some(n + 1) }\n\
                 fn b(s: string) -> Result<i64, string> { let n = s.try_parse_i64()?; Result::Ok(n) }\n\
                 fn c() -> Option<i64> { let v = [1, 2].get(0)?; let w = [3].get(0)?; Option::Some(v + w) }");
    assert!(!o.diags.has_errors(), "{}", o.rendered());
}

#[test]
fn question_mark_rejects_the_wrong_operand_and_the_wrong_function() {
    // not an Option / Result
    let o = run("fn f() -> Option<i64> { let n = 5?; Option::Some(n) }");
    assert_eq!(error_codes(&o), ["E0260"], "{}", o.rendered());
    assert!(o.rendered().contains("needs an `Option` or a `Result`"));
    // function returns something else
    let o = run("fn f(s: string) -> i64 { s.try_parse_i64()? }");
    assert_eq!(error_codes(&o), ["E0261"], "{}", o.rendered());
    let o = run("fn f(s: string) { let n = s.index_of(\"a\")?; println(n); }");
    assert_eq!(error_codes(&o), ["E0261"], "{}", o.rendered());
    // an Option inside a Result function: suggest ok_or
    let o = run("fn f(s: string) -> Result<i64, string> { let n = s.index_of(\"a\")?; Result::Ok(n) }");
    assert_eq!(error_codes(&o), ["E0261"], "{}", o.rendered());
    assert!(o.rendered().contains("ok_or"), "{}", o.rendered());
    // different error types
    let o = run("fn f(s: string) -> Result<i64, i64> { let n = s.try_parse_i64()?; Result::Ok(n) }");
    assert_eq!(error_codes(&o), ["E0261"], "{}", o.rendered());
    // no function around
    let o = run("let n = \"1\".try_parse_i64()?;");
    assert_eq!(error_codes(&o), ["E0262"], "{}", o.rendered());
}
