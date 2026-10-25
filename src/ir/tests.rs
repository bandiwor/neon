use super::{interp, optimize, verify_program, IrProgram, OptConfig};
use crate::driver::compile_ir_source;
use crate::runtime::Trap;
use crate::vfs::MemVfs;

fn compile(src: &str) -> IrProgram {
    compile_with(src, &[])
}

fn compile_with(src: &str, files: &[(&str, &str)]) -> IrProgram {
    let mut vfs = MemVfs::new();
    for (p, t) in files {
        vfs.insert(p, t);
    }
    let c = compile_ir_source("main.neon", src, &vfs);
    assert!(!c.diags.has_errors(), "compile errors:\n{}", c.render_diagnostics());
    let ir = c.ir.expect("no errors, so IR");
    verify_program(&ir).unwrap_or_else(|e| panic!("invalid IR: {e}\n{ir}"));
    ir
}

/// Every optimizer configuration the tests compare against the unoptimized program.
fn configs() -> Vec<(&'static str, OptConfig)> {
    let base = OptConfig::none();
    vec![
        ("fold", OptConfig { fold: true, ..base }),
        ("dce", OptConfig { dce: true, ..base }),
        ("simplify", OptConfig { simplify: true, ..base }),
        ("licm", OptConfig { licm: true, ..base }),
        ("inline", OptConfig { inline: true, ..base }),
        ("inline-aggressive", OptConfig { fold: true, dce: true, inline: true, inline_threshold: 200, ..base }),
        ("all", OptConfig::default()),
    ]
}

/// Runs `ir` unoptimized and under every optimizer config, requiring
/// identical output and traps and valid IR throughout.
fn run_ir(ir: &IrProgram, input: &[&str]) -> interp::RunResult {
    let reference = interp::run(ir, input);
    for (name, cfg) in configs() {
        let mut opt = ir.clone();
        optimize(&mut opt, &cfg);
        verify_program(&opt).unwrap_or_else(|e| panic!("[{name}] invalid IR after optimization: {e}\n{opt}"));
        let r = interp::run(&opt, input);
        assert_eq!(r.output, reference.output, "[{name}] output differs after optimization\n{opt}");
        assert_eq!(r.trap, reference.trap, "[{name}] trap differs after optimization\n{opt}");
    }
    reference
}

/// Runs a program and returns its output, failing on a trap.
fn run(src: &str) -> String {
    let ir = compile(src);
    let r = run_ir(&ir, &[]);
    assert!(r.trap.is_none(), "trap: {:?}\noutput so far: {}\n{ir}", r.trap, r.output);
    r.output
}

fn trap_of(src: &str) -> Trap {
    let ir = compile(src);
    let r = run_ir(&ir, &[]);
    r.trap.unwrap_or_else(|| panic!("expected a trap; output: {}", r.output))
}

#[test]
fn hello_and_arithmetic() {
    assert_eq!(
        run("println(\"hello\"); println(1 + 2 * 3); print(7); print(\" \"); println(2.5);"),
        "hello\n7\n7 2.5\n"
    );
}

#[test]
fn variables_and_assignment() {
    assert_eq!(
        run("var x = 1; x = x + 1; x += 5; x *= 2; println(x); let y = x; x = 0; println(y); println(x);"),
        "14\n14\n0\n"
    );
}

#[test]
fn control_flow() {
    assert_eq!(
        run("var i = 0; var s = 0; while i < 5 { s += i; i += 1; } println(s);\n\
             let a = if s > 5 { \"big\" } else { \"small\" }; println(a);\n\
             var n = 0; let r = loop { n += 1; if n == 4 { break n * 10; } }; println(r);\n\
             for k in 0..3 { print(k); } println(\"\"); for k in 1..=3 { print(k); } println(\"\");\n\
             for k in 0..10 { if k % 2 == 0 { continue; } if k > 6 { break; } print(k); } println(\"\");"),
        "10\nbig\n40\n012\n123\n135\n"
    );
}

#[test]
fn functions_recursion_and_fib() {
    assert_eq!(
        run("fn fact(n: u64) -> u64 { if n <= 1 { 1 } else { n * fact(n - 1) } }\n\
             fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }\n\
             println(fact(20)); println(fib(15));"),
        "2432902008176640000\n610\n"
    );
}

#[test]
fn factorial_overflow_traps() {
    assert_eq!(
        trap_of("fn fact(n: u64) -> u64 { if n <= 1 { 1 } else { n * fact(n - 1) } }\nprintln(fact(21));"),
        Trap::Overflow
    );
}

#[test]
fn structs_methods_and_fields() {
    assert_eq!(
        run("struct P { x: i32, y: i32 }\nimpl P { fn new(x: i32, y: i32) -> P { P { x: x, y: y } } fn sum(self) -> i32 { self.x + self.y } fn shift(var self, d: i32) { self.x += d; } }\n\
             var p = P::new(1, 2); println(p.sum()); p.shift(10); println(p.x); p.y = 100; println(p.sum()); println(p);"),
        "3\n11\n111\nP { x: 11, y: 100 }\n"
    );
}

#[test]
fn enums_and_match() {
    assert_eq!(
        run("enum Shape { Circle(f64), Rect(f64, f64), Empty }\n\
             fn area(s: Shape) -> f64 { match s { Shape::Circle(r) => 3.0 * r * r, Shape::Rect(w, h) => w * h, Shape::Empty => 0.0 } }\n\
             println(area(Shape::Circle(2.0))); println(area(Shape::Rect(2.0, 3.5))); println(area(Shape::Empty));\n\
             println(Shape::Rect(1.0, 2.0)); println(Shape::Empty);"),
        "12.0\n7.0\n0.0\nShape::Rect(1.0, 2.0)\nShape::Empty\n"
    );
}

#[test]
fn generic_enum_and_nested_patterns() {
    assert_eq!(
        run("enum Option<T> { Some(T), None }\n\
             fn unwrap_or<T>(o: Option<T>, d: T) -> T { match o { Option::Some(x) => x, Option::None => d } }\n\
             fn f(o: Option<Option<i32>>) -> i32 { match o { Option::Some(Option::Some(x)) if x > 0 => x, Option::Some(Option::Some(_)) => 0, Option::Some(Option::None) => -1, Option::None => -2 } }\n\
             println(unwrap_or(Option::Some(5), 0)); println(unwrap_or(Option::None, \"dflt\"));\n\
             println(f(Option::Some(Option::Some(7)))); println(f(Option::Some(Option::Some(-7)))); println(f(Option::Some(Option::None))); println(f(Option::None));"),
        "5\ndflt\n7\n0\n-1\n-2\n"
    );
}

#[test]
fn literal_range_and_or_patterns() {
    assert_eq!(
        run("fn g(n: i32) -> string { match n { 0 | 1 => \"small\", 2..=9 => \"mid\", -5 => \"neg\", _ => \"big\" } }\n\
             println(g(0)); println(g(1)); println(g(5)); println(g(9)); println(g(10)); println(g(-5));\n\
             fn c(ch: char) -> i32 { match ch { 'a'..='z' => 1, 'A'..='Z' => 2, _ => 0 } }\n\
             println(c('q')); println(c('Q')); println(c('1'));\n\
             fn s(x: string) -> i32 { match x { \"one\" => 1, \"two\" => 2, _ => 0 } } println(s(\"two\"));"),
        "small\nsmall\nmid\nmid\nbig\nneg\n1\n2\n0\n2\n"
    );
}

#[test]
fn closures_capture_and_mutate() {
    assert_eq!(
        run("fn make_counter() -> fn() -> i32 { var n = 0; || => { n += 1; n } }\n\
             let c = make_counter(); println(c()); println(c()); println(c());\n\
             let d = make_counter(); println(d()); println(c());"),
        "1\n2\n3\n1\n4\n"
    );
}

#[test]
fn closures_share_one_captured_variable() {
    assert_eq!(
        run("fn pair() -> [fn() -> i32] { var x = 0; let inc = || => { x += 1; x }; let get = || => x; [inc, get] }\n\
             let fs = pair(); let inc = fs[0]; let get = fs[1]; inc(); inc(); println(get()); inc(); println(get());"),
        "2\n3\n"
    );
}

#[test]
fn closures_in_loops_capture_per_iteration() {
    assert_eq!(
        run("var fs: [fn() -> i32] = []; for i in 0..3 { let k = i * 10; fs.push(|| => k + i); }\n\
             for f in fs { println(f()); }"),
        "0\n11\n22\n"
    );
}

#[test]
fn nested_closures() {
    assert_eq!(
        run("fn f() -> i32 { var x = 1; let g = || => { let h = || => { x += 10; x }; h() + x }; g() }\nprintln(f());"),
        "22\n"
    );
}

#[test]
fn higher_order_functions() {
    assert_eq!(
        run("fn apply(f: fn(i32) -> i32, v: i32) -> i32 { f(v) }\nfn double(x: i32) -> i32 { x * 2 }\n\
             fn compose(f: fn(i32) -> i32, g: fn(i32) -> i32) -> fn(i32) -> i32 { |x| => g(f(x)) }\n\
             println(apply(double, 21)); println(apply(|x| => x + 1, 1)); let h = compose(double, |x| => x - 1); println(h(5));"),
        "42\n2\n9\n"
    );
}

#[test]
fn collections() {
    assert_eq!(
        run("var a = [3, 1, 2]; a.push(4); println(a.len()); println(a); a.sort(); println(a); println(a.pop()); a.reverse(); println(a); println(a[0] + a[1]);\n\
             var m = { \"b\": 2, \"a\": 1 }; m.insert(\"c\", 3); m[\"a\"] = 10; println(m[\"a\"]); println(m.len()); println(m.contains_key(\"z\")); println(m.get_or(\"z\", -1)); println(m);\n\
             println(m.keys()); println(m.values());\n\
             var s = set { 1, 2, 2, 3 }; println(s.len()); println(s.insert(2)); println(s.insert(9)); println(s.contains(9)); println(s.remove(1)); println(s);\n\
             var total = 0; for x in a { total += x; } for k in m.keys() { total += m[k]; } for e in s { total += e; } println(total);"),
        "4\n[3, 1, 2, 4]\n[1, 2, 3, 4]\n4\n[3, 2, 1]\n5\n10\n3\nfalse\n-1\n{\"b\": 2, \"a\": 10, \"c\": 3}\n[\"b\", \"a\", \"c\"]\n[2, 10, 3]\n3\nfalse\ntrue\ntrue\ntrue\nset{2, 3, 9}\n35\n"
    );
}

#[test]
fn strings() {
    assert_eq!(
        run("let s = \"Hello, World\"; println(s.len()); println(s.to_upper()); println(s.to_lower()); println(s.contains(\"World\")); println(s.starts_with(\"Hell\")); println(s.find(\"o\"));\n\
             println(s.substring(7, 12)); println(s.char_at(1)); println(\"a,b,c\".split(\",\")); println(\"  hi  \".trim()); println(\"x\" + \"y\" + \"z\"); println(\"abc\" < \"abd\");\n\
             println(\"12\".parse_i64() + 1); println(\"2.5\".parse_f64() * 2.0); println(\"héllo\".len()); println(\"ab\".chars()); println([\"x\", \"y\"].join(\"-\"));"),
        "12\nHELLO, WORLD\nhello, world\ntrue\ntrue\n4\nWorld\ne\n[\"a\", \"b\", \"c\"]\nhi\nxyz\ntrue\n13\n5.0\n5\n['a', 'b']\nx-y\n"
    );
}

#[test]
fn numeric_builtins_and_constants() {
    assert_eq!(
        run("println(i32::MAX); println(i8::MIN); println(u64::MAX); println(2.0.sqrt()); println((-7).abs()); println(2.pow(10)); println(3.max(9)); println(1.5.min(0.5));\n\
             println(7.5.floor()); println(f64::INFINITY); println(5.to_string() + \"!\"); println('x'.to_upper()); println('7'.is_digit());"),
        "2147483647\n-128\n18446744073709551615\n1.4142135623730951\n7\n1024\n9\n0.5\n7.0\ninf\n5!\nX\ntrue\n"
    );
}

#[test]
fn casts_across_types() {
    assert_eq!(
        run("println(300 as u8); println(-1 as u8); println(3.99 as i32); println(-3.99 as i32); println(1e10 as i8); println(65u8 as char); println('a' as u32); println(true as i32); println(5 as f64 / 2.0); println(255u8 as i8);"),
        "44\n255\n3\n-3\n127\nA\n97\n1\n2.5\n-1\n"
    );
}

#[test]
fn integer_boundaries_trap() {
    assert_eq!(trap_of("let x: i8 = 127; println(x + 1);"), Trap::Overflow);
    assert_eq!(trap_of("let x: u8 = 0; println(x - 1);"), Trap::Overflow);
    assert_eq!(trap_of("let x = i32::MIN; println(x / -1);"), Trap::Overflow);
    assert_eq!(trap_of("let x = 1; let z = 0; println(x / z);"), Trap::DivideByZero);
    assert_eq!(trap_of("let x = 1u8; let n = 8u8; println(x << n);"), Trap::ShiftOverflow);
    assert_eq!(trap_of("let x = i64::MIN; println(-x);"), Trap::Overflow);
    assert_eq!(run("let a: i8 = -128; let b: u64 = 18446744073709551615; println(a); println(b); println(127i8 + 0); println(1u64 << 63);"), "-128\n18446744073709551615\n127\n9223372036854775808\n");
}

#[test]
fn runtime_traps() {
    assert!(matches!(trap_of("let a = [1, 2]; println(a[5]);"), Trap::IndexOutOfBounds { index: 5, len: 2 }));
    assert_eq!(trap_of("let m = {\"a\": 1}; println(m[\"b\"]);"), Trap::KeyNotFound);
    assert_eq!(trap_of("var a: [i32] = []; a.pop();"), Trap::EmptyArray);
    assert_eq!(trap_of("panic(\"boom\");"), Trap::Panic("boom".into()));
    assert_eq!(trap_of("assert(1 > 2, \"math\");"), Trap::Panic("assertion failed: math".into()));
    assert!(matches!(trap_of("println(\"x\".parse_i64());"), Trap::ParseError(_)));
}

#[test]
fn traits_and_operator_overloading() {
    assert_eq!(
        run("trait Shape { fn area(self) -> f64; fn name(self) -> string { \"shape\" } }\n\
             struct Sq { s: f64 } struct Circle { r: f64 }\n\
             impl Shape for Sq { fn area(self) -> f64 { self.s * self.s } }\n\
             impl Shape for Circle { fn area(self) -> f64 { 3.0 * self.r * self.r } fn name(self) -> string { \"circle\" } }\n\
             fn describe<T: Shape>(x: T) -> string { x.name() + \":\" + x.area().to_string() }\n\
             println(describe(Sq { s: 3.0 })); println(describe(Circle { r: 1.0 }));\n\
             struct V { x: f64, y: f64 }\n\
             impl Add<V> for V { fn add(self, o: V) -> V { V { x: self.x + o.x, y: self.y + o.y } } }\n\
             impl Mul<f64> for V { fn mul(self, k: f64) -> V { V { x: self.x * k, y: self.y * k } } }\n\
             impl Neg for V { fn neg(self) -> V { V { x: -self.x, y: -self.y } } }\n\
             impl Eq for V { fn eq(self, o: V) -> bool { self.x == o.x && self.y == o.y } }\n\
             impl Ord for V { fn lt(self, o: V) -> bool { self.x < o.x } }\n\
             let a = V { x: 1.0, y: 2.0 }; let b = a + a * 3.0; println(b); println(-a); println(a == b); println(a != b); println(a < b); println(a >= b); println(a <= a);\n\
             var c = a; c += b; println(c);"),
        "shape:9.0\ncircle:3.0\nV { x: 4.0, y: 8.0 }\nV { x: -1.0, y: -2.0 }\nfalse\ntrue\ntrue\nfalse\ntrue\nV { x: 5.0, y: 10.0 }\n"
    );
}

#[test]
fn generic_functions_with_operator_bounds_instantiate_per_type() {
    let src = "fn sum<T: Add<T>>(a: T, b: T) -> T { a + b }\n\
               println(sum(1, 2)); println(sum(1.5, 2.25)); println(sum(\"a\", \"b\")); println(sum(250u8, 5u8));";
    assert_eq!(run(src), "3\n3.75\nab\n255\n");
    let ir = compile(src);
    let names: Vec<&str> = ir.funcs.iter().map(|f| f.name.as_str()).collect();
    for want in ["sum<i64>", "sum<f64>", "sum<string>", "sum<u8>"] {
        assert!(names.contains(&want), "missing instance {want}: {names:?}");
    }
    assert!(trap_of("fn sum<T: Add<T>>(a: T, b: T) -> T { a + b }\nprintln(sum(250u8, 6u8));") == Trap::Overflow);
}

#[test]
fn generic_struct_methods() {
    assert_eq!(
        run("struct Stack<T> { items: [T] }\n\
             impl<T> Stack<T> { fn new() -> Stack<T> { Stack { items: [] } } fn push(var self, x: T) { self.items.push(x); } fn pop(var self) -> T { self.items.pop() } fn size(self) -> i64 { self.items.len() } }\n\
             var a = Stack::new(); a.push(1); a.push(2); println(a.pop()); println(a.size());\n\
             var b = Stack::<string>::new(); b.push(\"x\"); println(b.pop() + \"y\");"),
        "2\n1\nxy\n"
    );
}

#[test]
fn display_trait_is_used_by_println() {
    assert_eq!(
        run("struct P { x: i32 }\nimpl Display for P { fn to_string(self) -> string { \"P<\" + self.x.to_string() + \">\" } }\nprintln(P { x: 5 }); println(P { x: 6 }.to_string());"),
        "P<5>\nP<6>\n"
    );
}

#[test]
fn globals_and_top_level_state() {
    assert_eq!(
        run("var counter: i32 = 0; const LIMIT: i32 = 3;\nfn bump() -> i32 { counter += 1; counter }\nbump(); bump(); println(bump()); println(LIMIT * 2); println(counter);"),
        "3\n6\n3\n"
    );
}

#[test]
fn modules_run_in_dependency_order() {
    let ir = compile_with(
        "import { greet, Counter } from \"lib\";\nprintln(\"main\"); println(greet(\"x\")); let c = Counter { n: 1 }; println(c.next());",
        &[("lib.neon", "println(\"lib init\");\nexport fn greet(s: string) -> string { \"hi \" + s }\nexport struct Counter { n: i32 }\nimpl Counter { fn next(self) -> i32 { self.n + 1 } }")],
    );
    let r = run_ir(&ir, &[]);
    assert!(r.trap.is_none());
    assert_eq!(r.output, "lib init\nmain\nhi x\n2\n");
}

#[test]
fn main_function_runs_after_top_level_code() {
    assert_eq!(run("println(\"top\"); fn main() { println(\"main\"); }"), "top\nmain\n");
}

#[test]
fn input_builtin() {
    let ir = compile("let a = input(); let b = input(); println(a + \"+\" + b); println(input().len());");
    let r = run_ir(&ir, &["foo", "bar"]);
    assert_eq!(r.output, "foo+bar\n0\n");
}

#[test]
fn short_circuit_evaluation() {
    assert_eq!(
        run("fn t(s: string) -> bool { println(s); true } fn f(s: string) -> bool { println(s); false }\n\
             println(f(\"a\") && t(\"b\")); println(t(\"c\") || f(\"d\")); println(t(\"e\") && t(\"f\"));"),
        "a\nfalse\nc\ntrue\ne\nf\ntrue\n"
    );
}

#[test]
fn evaluation_order_and_aliasing() {
    // an argument that mutates a variable must not change an earlier operand
    assert_eq!(run("var x = 1; fn f(a: i32, b: i32) -> i32 { a * 100 + b } fn bump() -> i32 { x += 1; x }\nprintln(f(x, bump())); println(x);"), "102\n2\n");
}

#[test]
fn stack_overflow_is_a_trap_not_a_crash() {
    assert_eq!(trap_of("fn f(n: i32) -> i32 { f(n + 1) + 1 }\nprintln(f(0));"), Trap::StackOverflow);
}

#[test]
fn unit_and_never_flow() {
    assert_eq!(
        run(
            "fn f(x: i32) -> i32 { let y = if x > 0 { x } else { return 0 }; y * 2 }\nprintln(f(3)); println(f(-3));\n\
             fn g(x: i32) -> i32 { match x { 0 => panic(\"zero\"), n => n } } println(g(4));\nlet u = (); println(u);"
        ),
        "6\n0\n4\n()\n"
    );
}

// ---- optimizer effects ---------------------------------------------------------------------

fn optimized(src: &str, cfg: OptConfig) -> (IrProgram, super::OptStats) {
    let mut ir = compile(src);
    let stats = optimize(&mut ir, &cfg);
    verify_program(&ir).unwrap_or_else(|e| panic!("invalid IR: {e}\n{ir}"));
    (ir, stats)
}

fn count(ir: &IrProgram, pred: impl Fn(&super::Inst) -> bool) -> usize {
    ir.funcs.iter().flat_map(|f| &f.blocks).flat_map(|b| &b.insts).filter(|i| pred(i)).count()
}

#[allow(dead_code)]
fn fn_named<'a>(ir: &'a IrProgram, name: &str) -> &'a super::IrFunc {
    ir.funcs.iter().find(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"))
}

#[test]
fn constants_are_folded_across_all_types() {
    let src = "println(2 + 3 * 4); println(10u8 - 3u8); println(7.5 * 2.0); println(1.5f32 + 1.5f32); println(100i64 << 3); println(-5 / 2); println(7 % 4 == 3); println(!true); println(255u8 as i8); println(2.9 as u8); println(\"a\" < \"b\");";
    let (ir, stats) = optimized(src, OptConfig::default());
    assert!(stats.folded > 0);
    assert_eq!(count(&ir, |i| matches!(i, super::Inst::Arith { .. })), 0, "{ir}");
    assert_eq!(count(&ir, |i| matches!(i, super::Inst::Cast { .. })), 0, "{ir}");
    // only the string comparison survives
    assert_eq!(count(&ir, |i| matches!(i, super::Inst::Cmp { .. })), 1, "{ir}");
    assert_eq!(run(src), "14\n7\n15.0\n3.0\n800\n-2\ntrue\nfalse\n-1\n2\ntrue\n");
}

#[test]
fn trapping_constant_expressions_are_not_folded_away() {
    for src in [
        "let x: i8 = 127; println(x + 1);",
        "println(1 / 0);",
        "let a: u8 = 0; println(a - 1);",
        "println(1 << 64);",
        "println(i32::MIN / -1);",
        "println(i64::MAX * 2);",
    ] {
        let (ir, _) = optimized(src, OptConfig::default());
        let r = interp::run(&ir, &[]);
        assert!(r.trap.is_some(), "the trap in `{src}` was optimized away:\n{ir}");
        assert_eq!(r.trap, interp::run(&compile(src), &[]).trap);
    }
}

#[test]
fn constant_branches_prune_dead_code() {
    let src = "fn f() { if 1 < 2 { println(\"yes\"); } else { println(\"no\"); }\nlet n = 3; while n > 5 { println(\"never\"); }\nprintln(if 2 > 1 { 10 } else { 20 }); }\nf();";
    let (ir, stats) = optimized(src, OptConfig::default());
    assert!(stats.removed_blocks > 0);
    let printlns = count(&ir, |i| matches!(i, super::Inst::Intrinsic { b: crate::builtin::Builtin::Println, .. }));
    assert_eq!(printlns, 2, "{ir}");
    assert_eq!(run(src), "yes\n10\n");
}

#[test]
fn dead_pure_computation_is_removed() {
    let src = "fn f() -> i32 { let unused = 1.5 * 2.0; let unused2 = unused + 1.0; let keep = 3; keep }\nprintln(f());";
    let (ir, _) = optimized(src, OptConfig::default());
    assert_eq!(count(&ir, |i| matches!(i, super::Inst::Arith { .. })), 0, "{ir}");
    // integer arithmetic may trap, so an unused one is kept
    let (kept, _) =
        optimized("fn f() -> i32 { let a = 1; let u = a / (a - 1); 3 }\nprintln(f());", OptConfig::default());
    assert_eq!(count(&kept, |i| matches!(i, super::Inst::Arith { .. })), 1, "{kept}");
    assert_eq!(run(src), "3\n");
}

#[test]
fn inlining_removes_calls_to_small_functions() {
    let src = "fn sq(x: i32) -> i32 { x * x }\nfn add(a: i32, b: i32) -> i32 { a + b }\nlet v = 7; println(add(sq(v), sq(3)));";
    let (ir, stats) = optimized(src, OptConfig::default());
    assert!(stats.inlined_calls >= 3, "{stats:?}");
    assert_eq!(ir.funcs.len(), 1, "everything was inlined and pruned:\n{ir}");
    let calls = count(&ir, |i| matches!(i, super::Inst::Call { .. }));
    assert_eq!(calls, 0, "calls remain:\n{ir}");
    assert_eq!(run(src), "58\n");
}

#[test]
fn recursive_and_large_functions_are_not_inlined() {
    let src = "fn fact(n: u64) -> u64 { if n <= 1 { 1 } else { n * fact(n - 1) } }\nprintln(fact(5));";
    let (ir, _) = optimized(src, OptConfig::default());
    assert!(count(&ir, |i| matches!(i, super::Inst::Call { func, .. } if ir.funcs[func.index()].name == "fact")) >= 2);
    assert_eq!(run(src), "120\n");
    let (_, stats) = optimized(
        "fn f(x: i32) -> i32 { x + 1 }\nprintln(f(1));",
        OptConfig { inline_threshold: 1, ..OptConfig::default() },
    );
    assert_eq!(stats.inlined_calls, 0);
}

#[test]
fn closure_modified_variables_are_never_folded() {
    // `x` has a single `let` definition; only the closure (via its upvalue) changes it
    let src = "var x = 1; let f = || => { x = 5; }; f(); println(x);\n\
               fn g() -> i32 { var y = 10; let h = || => { y += 1; }; h(); h(); y }\nprintln(g());";
    assert_eq!(run(src), "5\n12\n");
    let (ir, _) = optimized(src, OptConfig::default());
    let r = interp::run(&ir, &[]);
    assert_eq!(r.output, "5\n12\n", "{ir}");
}

#[test]
fn copy_propagation_respects_later_assignment() {
    // `b` is a copy of `a`, but `a` changes afterwards
    assert_eq!(run("var a = 1; let b = a; a = 2; println(b); println(a);"), "1\n2\n");
    assert_eq!(run("fn f() -> i32 { var a = 1; let b = a; a += 10; b + a }\nprintln(f());"), "12\n");
}

#[test]
fn loops_survive_optimization() {
    let src = "var s = 0; for i in 0..100 { if i % 3 == 0 { continue; } s += i; if s > 1000 { break; } } println(s);\n\
               var n = 27u64; var steps = 0; while n != 1 { n = if n % 2 == 0 { n / 2 } else { 3 * n + 1 }; steps += 1; } println(steps);";
    assert_eq!(run(src), "1027\n111\n");
}

#[test]
fn optimization_reduces_work() {
    let src = "fn sq(x: i32) -> i32 { x * x }\nfn run() -> i32 { var t = 0; for i in 0..10 { t += sq(i) + 2 * 3; } t }\nprintln(run());";
    let plain = compile(src);
    let (opt, stats) = optimized(src, OptConfig::default());
    let size = |p: &IrProgram| p.funcs.iter().map(|f| f.size()).sum::<usize>();
    assert!(size(&opt) < size(&plain), "{} !< {}\n{opt}", size(&opt), size(&plain));
    assert!(stats.removed_funcs > 0);
    assert_eq!(run(src), "345\n");
}

use super::{Inst, Term};

// ---- simplify pass ---------------------------------------------------------------------------

fn only_simplify() -> OptConfig {
    OptConfig { simplify: true, dce: true, fold: true, ..OptConfig::none() }
}

fn is_intrinsic(i: &Inst, which: crate::builtin::Builtin) -> bool {
    matches!(i, Inst::Intrinsic { b, .. } if *b == which)
}

#[test]
fn common_subexpressions_are_computed_once() {
    let src = "fn f(a: i64, b: i64) -> i64 { a * b + a * b }\nprintln(f(3, 4));";
    let (ir, _) = optimized(src, only_simplify());
    let muls = fn_named(&ir, "f")
        .blocks
        .iter()
        .flat_map(|b| &b.insts)
        .filter(|i| matches!(i, Inst::Arith { op: crate::runtime::numeric::ArithOp::Mul, .. }))
        .count();
    assert_eq!(muls, 1);
    assert_eq!(run(src), "24\n");
}

#[test]
fn a_repeated_trapping_operation_still_traps() {
    let src = "fn f(a: i64, b: i64) -> i64 { let x = a / b; let y = a / b; x + y }\nprintln(f(1, 0));";
    assert_eq!(trap_of(src), Trap::DivideByZero);
    assert_eq!(trap_of("fn f(a: i64) -> i64 { a * 2 }\nprintln(f(9223372036854775807));"), Trap::Overflow);
}

#[test]
fn memory_reads_are_not_reused_across_writes() {
    // the second read must see the write
    let src = "struct P { x: i64 }\nfn f(var p: P) -> i64 { let a = p.x; p.x = 7; let b = p.x; a + b }\nprintln(f(P { x: 1 }));";
    assert_eq!(run(src), "8\n");
    let src = "fn f(var xs: [i64]) -> i64 { let a = xs.len(); xs.push(1); let b = xs.len(); a * 10 + b }\nvar v = [5]; println(f(v));";
    assert_eq!(run(src), "12\n");
    // aliasing through a call
    let src = "struct P { x: i64 }\nfn bump(var p: P) { p.x += 1; }\nfn f(var p: P) -> i64 { let a = p.x; bump(p); a * 10 + p.x }\nprintln(f(P { x: 1 }));";
    assert_eq!(run(src), "12\n");
}

#[test]
fn fields_of_fresh_structs_and_enums_are_forwarded() {
    let src = "struct P { x: i64, y: i64 }\nfn f(a: i64, b: i64) -> i64 { let p = P { x: a, y: b }; p.x + p.y }\nprintln(f(1, 2));";
    let (ir, _) = optimized(src, only_simplify());
    let f = fn_named(&ir, "f");
    assert!(
        f.blocks.iter().flat_map(|b| &b.insts).all(|i| !matches!(i, Inst::NewStruct { .. } | Inst::GetField { .. })),
        "{ir}"
    );
    assert_eq!(run(src), "3\n");
    // a write to the struct is seen
    let src = "struct P { x: i64 }\nfn f() -> i64 { var p = P { x: 1 }; p.x = 5; p.x }\nprintln(f());";
    assert_eq!(run(src), "5\n");
    // the forwarded register changes after the construction
    let src =
        "struct P { x: i64 }\nfn f() -> i64 { var a = 1; let p = P { x: a }; a = 9; p.x * 10 + a }\nprintln(f());";
    assert_eq!(run(src), "19\n");
}

#[test]
fn matching_on_a_fresh_enum_is_decided_statically() {
    let src = "fn f(a: i64) -> i64 { match Option::Some(a) { Option::Some(x) => x + 1, Option::None => 0 } }\nprintln(f(4)); println(f(5));";
    let (ir, _) = optimized(src, OptConfig::default());
    assert_eq!(count(&ir, |i| matches!(i, Inst::NewEnum { .. })), 0, "{ir}");
    assert_eq!(run(src), "5\n6\n");
}

#[test]
fn integer_identities() {
    let src = "fn f(x: i64) -> i64 { (x + 0) * 1 - 0 + (x * 0) + (x % 1) + x / 1 }\nprintln(f(7)); println(f(-3));";
    let (ir, _) = optimized(src, only_simplify());
    let f = fn_named(&ir, "f");
    let arith = f.blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Arith { .. })).count();
    assert!(arith <= 1, "{ir}");
    assert_eq!(run(src), "14\n-6\n");
    // floats are left alone (-0.0)
    assert_eq!(run("let z = -0.0; println(z + 0.0); println(z * 1.0);"), "0.0\n-0.0\n");
}

#[test]
fn print_of_a_concatenation_prints_the_pieces() {
    let src = "let who = \"world\"; print(\"hello \" + who + \"!\"); println(\"\");";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| is_intrinsic(i, crate::builtin::Builtin::StrConcat)), 0, "{ir}");
    assert_eq!(run(src), "hello world!\n");
    // a piece that changes in between keeps its old value
    let src = "var s = \"a\"; let t = s + \"b\"; s = \"c\"; println(t); println(s);";
    assert_eq!(run(src), "ab\nc\n");
    // used twice: not split
    let src = "let t = \"x\" + \"y\"; println(t); println(t);";
    assert_eq!(run(src), "xy\nxy\n");
}

#[test]
fn unused_elements_of_for_loops_are_not_loaded() {
    let src = "var n = 0; for x in [1, 2, 3] { n += 1; } println(n);";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| matches!(i, Inst::ArrayGet { .. })), 0, "{ir}");
    assert_eq!(run(src), "3\n");
    // used elements stay, and so does their bounds check
    let src = "var n = 0; for x in [1, 2, 3] { n += x; } println(n);";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| matches!(i, Inst::ArrayGet { .. })), 1, "{ir}");
    // an array that shrinks while iterating is still handled by the loop condition
    assert_eq!(run("var a = [1, 2, 3, 4]; var n = 0; for x in a { a.pop(); n += 1; } println(n);"), "2\n");
    // a user-written index loop with a possibly negative start must keep its trap
    assert_eq!(
        trap_of("fn f(i: i64) { let a = [1]; if i < a.len() { let x = a[i]; } }\nf(-1);"),
        Trap::IndexOutOfBounds { index: -1, len: 1 }
    );
}

// ---- loop-invariant code motion ---------------------------------------------------------------

#[test]
fn invariants_leave_the_loop() {
    let src = "fn f(a: [i64]) -> i64 { var s = 0; var i = 0; while i < a.len() { s += a[i] * 3; i += 1; } s }\nprintln(f([1, 2, 3]));";
    let (ir, stats) = optimized(src, OptConfig { licm: true, ..only_simplify() });
    assert!(stats.hoisted > 0, "{ir}");
    assert_eq!(run(src), "18\n");
    // `len` is read once, before the loop
    let f = fn_named(&ir, "f");
    let in_loop_len = f
        .blocks
        .iter()
        .skip(1)
        .filter(|b| matches!(b.term, Term::Branch { .. }))
        .flat_map(|b| &b.insts)
        .filter(|i| is_intrinsic(i, crate::builtin::Builtin::ArrLen))
        .count();
    assert_eq!(in_loop_len, 0, "{ir}");
}

#[test]
fn a_loop_that_changes_the_array_keeps_reading_its_length() {
    let src = "var a = [1]; var i = 0; while i < a.len() { if i < 4 { a.push(0); } i += 1; } println(a.len());";
    assert_eq!(run(src), "5\n");
    // calls may change anything, too
    let src = "var a = [1];\nfn grow() { a.push(0); }\nvar i = 0; while i < a.len() { if i < 3 { grow(); } i += 1; } println(a.len());";
    assert_eq!(run(src), "4\n");
    // field reads in loops that write fields
    let src = "struct C { n: i64 }\nvar c = C { n: 0 }; var i = 0; while c.n < 5 { c.n += 1; i += 1; } println(i);";
    assert_eq!(run(src), "5\n");
}

#[test]
fn trapping_work_is_not_hoisted_out_of_loops_that_may_not_run() {
    let src = "fn f(n: i64, k: i64) -> i64 { var i = 0; var s = 0; while i < n { s += 100 / k; i += 1; } s }\nprintln(f(0, 0));";
    assert_eq!(run(src), "0\n");
    assert_eq!(trap_of("fn f(n: i64, k: i64) -> i64 { var i = 0; var s = 0; while i < n { s += 100 / k; i += 1; } s }\nprintln(f(1, 0));"), Trap::DivideByZero);
    // a payload read guarded by the tag check must stay behind the check
    let src = "fn f(o: Option<i64>, n: i64) -> i64 { var s = 0; var i = 0; while i < n { match o { Option::Some(x) => { s += x; } Option::None => {} } i += 1; } s }\nprintln(f(Option::None, 3)); println(f(Option::Some(2), 3));";
    assert_eq!(run(src), "0\n6\n");
}

#[test]
fn nested_loops_hoist_outward() {
    let src = "fn f(a: [i64]) -> i64 { var s = 0; var i = 0; while i < a.len() { var j = 0; while j < a.len() { s += a[i] * a[j]; j += 1; } i += 1; } s }\nprintln(f([1, 2, 3]));";
    assert_eq!(run(src), "36\n");
}

// ---- more simplifications ------------------------------------------------------------------------

fn arith_count(ir: &IrProgram, op: crate::runtime::numeric::ArithOp) -> usize {
    count(ir, |i| matches!(i, Inst::Arith { op: o, .. } if *o == op))
}

#[test]
fn stored_values_are_forwarded_to_later_loads() {
    let src = "struct P { x: i64 }\nfn f(var p: P) -> i64 { p.x = 5; p.x * 2 }\nprintln(f(P { x: 1 }));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| matches!(i, Inst::GetField { .. })), 0, "{ir}");
    assert_eq!(run(src), "10\n");
    // two names for one object: the second store is the one that counts
    let src = "struct P { x: i64 }\nfn g(var a: P, var b: P) -> i64 { a.x = 1; b.x = 2; a.x }\nlet p = P { x: 0 };\nprintln(g(p, p)); println(g(P { x: 0 }, P { x: 0 }));";
    assert_eq!(run(src), "2\n1\n");
    // arrays: forwarded, aliasing respected, bounds still checked
    let src = "fn h(var a: [i64]) -> i64 { a[0] = 7; a[0] }\nprintln(h([1]));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| matches!(i, Inst::ArrayGet { .. })), 0, "{ir}");
    assert_eq!(run(src), "7\n");
    let src =
        "fn k(var a: [i64], var b: [i64]) -> i64 { a[1] = 1; b[1] = 2; a[1] }\nlet x = [0, 0];\nprintln(k(x, x));";
    assert_eq!(run(src), "2\n");
    assert_eq!(
        trap_of("fn h(var a: [i64]) -> i64 { a[5] = 1; a[5] }\nprintln(h([1]));"),
        Trap::IndexOutOfBounds { index: 5, len: 1 }
    );
    // a push between the store and the load
    let src = "fn m(var a: [i64]) -> i64 { a[0] = 3; a.push(4); a[0] + a.len() }\nprintln(m([1]));";
    assert_eq!(run(src), "5\n");
}

#[test]
fn copies_are_read_through() {
    let src = "fn f(a: i64) -> i64 { var b = a; var c = b; b = 5; c + b + a }\nprintln(f(10));";
    assert_eq!(run(src), "25\n");
    let src = "fn f(a: i64) -> i64 { var t = a; var u = t; t += 1; u }\nprintln(f(3));";
    assert_eq!(run(src), "3\n");
}

#[test]
fn self_operations() {
    let src = "fn f(x: i64, y: u8) -> i64 { (x - x) + (x ^ x) + (x & x) + (x | x) + (y - y) as i64 + (x == x) as i64 + (x < x) as i64 + (x <= x) as i64 }\nprintln(f(7, 3));";
    // x & x + x | x = 14; x == x and x <= x are 1 each
    assert_eq!(run(src), "16\n");
    // NaN is not equal to itself
    assert_eq!(run("fn f(x: f64) -> bool { x == x }\nprintln(f(0.0 / 0.0)); println(f(1.0));"), "false\ntrue\n");
    // overflow checks are not dropped by the identities
    assert_eq!(trap_of("fn f(x: i8) -> i8 { (x + 1) - (x + 1) }\nprintln(f(127i8));"), Trap::Overflow);
}

#[test]
fn unsigned_power_of_two_division_becomes_shifts() {
    use crate::runtime::numeric::ArithOp;
    let src =
        "fn f(x: u64) -> u64 { x / 8 + x % 8 }\nprintln(f(100)); println(f(7)); println(f(18446744073709551615));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(arith_count(&ir, ArithOp::Div) + arith_count(&ir, ArithOp::Rem), 0, "{ir}");
    assert_eq!(run(src), "16\n7\n2305843009213693958\n");
    assert_eq!(run("fn f(x: u8) -> u8 { x / 128 + x % 128 }\nprintln(f(255u8)); println(f(3u8));"), "128\n3\n");
    assert_eq!(run("fn f(x: u32) -> u32 { x / 2 }\nprintln(f(4294967295u32));"), "2147483647\n");
    // signed values keep rounding toward zero
    assert_eq!(run("fn f(x: i64) -> i64 { x / 2 * 10 + x % 2 }\nprintln(f(-7)); println(f(7));"), "-31\n31\n");
    let (ir, _) = optimized("fn f(x: i64) -> i64 { x / 2 }\nprintln(f(-7));", only_simplify());
    assert!(arith_count(&ir, ArithOp::Div) >= 1, "{ir}");
}

#[test]
fn branching_on_a_negation_swaps_the_targets() {
    let src = "fn f(a: i64) -> i64 { if !(a > 3) { 1 } else { 2 } }\nprintln(f(1)); println(f(9));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| matches!(i, Inst::BoolNot { .. })), 0, "{ir}");
    assert_eq!(run(src), "1\n2\n");
    let src = "fn f(a: bool, b: bool) -> i64 { if !a && !b { 1 } elif !a { 2 } else { 3 } }\nprintln(f(false, false)); println(f(false, true)); println(f(true, false));";
    assert_eq!(run(src), "1\n2\n3\n");
}

#[test]
fn constant_strings_are_joined_at_compile_time() {
    use crate::builtin::Builtin;
    let src = "println(\"a\" + \"b\" + \"c\");";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| is_intrinsic(i, Builtin::StrConcat)), 0, "{ir}");
    assert_eq!(run(src), "abc\n");
    // adjacent constant prints are one print
    let src = "print(\"x = \"); print(\"1\"); println(\"!\"); println(\"next\");";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count(&ir, |i| is_intrinsic(i, Builtin::Print) || is_intrinsic(i, Builtin::Println)), 1, "{ir}");
    assert_eq!(run(src), "x = 1!\nnext\n");
    // a non-constant argument in between keeps the order
    let src = "let n = 5; print(\"a\"); print(n); print(\"b\"); println(\"c\");";
    assert_eq!(run(src), "a5bc\n");
    assert_eq!(run("print(\"\"); println(\"\"); print(\"\");"), "\n");
}
