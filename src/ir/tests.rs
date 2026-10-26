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

#[test]
fn length_and_elements_of_a_fresh_array_are_known() {
    use crate::builtin::Builtin;
    let src = "fn f() -> i64 { let a = [10, 20, 30]; a.len() * 100 + a[1] }\nprintln(f());";
    let (ir, _) = optimized(src, OptConfig::default());
    assert_eq!(count(&ir, |i| is_intrinsic(i, Builtin::ArrLen)), 0, "{ir}");
    assert_eq!(count(&ir, |i| matches!(i, Inst::ArrayGet { .. })), 0, "{ir}");
    assert_eq!(run(src), "320\n");
    // a write, a push or a call may change them
    assert_eq!(run("fn f() -> i64 { var a = [1, 2]; a.push(3); a.len() }\nprintln(f());"), "3\n");
    assert_eq!(run("fn f() -> i64 { var a = [1, 2]; a[1] = 9; a[1] }\nprintln(f());"), "9\n");
    assert_eq!(
        run("fn grow(var a: [i64]) { a.push(0); }\nfn f() -> i64 { var a = [1]; grow(a); a.len() }\nprintln(f());"),
        "2\n"
    );
    // an index past the end still traps
    assert_eq!(
        trap_of("fn f() -> i64 { let a = [1, 2]; a[2] }\nprintln(f());"),
        Trap::IndexOutOfBounds { index: 2, len: 2 }
    );
    assert_eq!(run("fn f() -> bool { let a: [i64] = []; a.is_empty() }\nprintln(f());"), "true\n");
}

#[test]
fn loop_counters_stay_immediates() {
    // `i += 1` is an `addi` in the bytecode: its constant must not be hoisted into a register
    let src = "fn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { s += i * 3; i += 1; } s }\nprintln(f(5));";
    let (ir, stats) = optimized(src, OptConfig { licm: true, ..only_simplify() });
    let f = fn_named(&ir, "f");
    let entry_consts_one =
        f.blocks[0].insts.iter().filter(|i| matches!(i, Inst::Const { val: crate::ir::Const::Bits(1), .. })).count();
    assert_eq!(entry_consts_one, 0, "{ir}");
    let _ = stats;
    assert_eq!(run(src), "30\n");
}

// ---- effect analysis -------------------------------------------------------------------------------

use crate::effects::{Effects, Purity};

fn effects_of(ir: &IrProgram, fx: &super::FuncEffects, name: &str) -> Effects {
    let i = ir.funcs.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}"));
    fx.of(super::FuncId(i as u32))
}

#[test]
fn function_effects_are_computed_from_the_call_graph() {
    let src = "struct P { x: i64 }\n\
        fn sq(x: f64) -> f64 { x * x }\n\
        fn add(a: i64, b: i64) -> i64 { a + b }\n\
        fn len_of(a: [i64]) -> i64 { a.len() }\n\
        fn show(x: i64) { println(x); }\n\
        fn push(var a: [i64]) { a.push(1); }\n\
        fn mk() -> [i64] { [1, 2] }\n\
        fn field(p: P) -> i64 { p.x }\n\
        fn indirect() -> f64 { sq(2.0) }\n\
        fn noisy() { show(1); }\n\
        fn fact(n: i64) -> i64 { if n < 2 { 1 } else { n * fact(n - 1) } }\n\
        fn even(n: i64) -> bool { if n == 0 { true } else { odd(n - 1) } }\n\
        fn odd(n: i64) -> bool { if n == 0 { false } else { even(n - 1) } }\n\
        fn spin(n: i64) -> i64 { var i = 0; while i < n { i += 1; } i }\n\
        fn via(f: fn(i64) -> i64) -> i64 { f(1) }\n\
        fn main() {\n\
            sq(1.0); add(1, 2); len_of([1]); show(1); var a = [1]; push(a); mk(); field(P { x: 1 });\n\
            indirect(); noisy(); fact(3); even(2); odd(2); spin(2); via(|x| => x);\n\
        }";
    let ir = compile(src);
    let fx = super::analyze_effects(&ir);
    let e = |name| effects_of(&ir, &fx, name);

    assert_eq!(e("sq"), Effects::NONE);
    assert_eq!(e("sq").purity(), Purity::Const);
    assert_eq!(e("add"), Effects::TRAPS, "integer addition may overflow");
    assert_eq!(e("add").purity(), Purity::Const);
    assert_eq!(e("len_of"), Effects::READS);
    assert_eq!(e("len_of").purity(), Purity::Pure);
    assert_eq!(e("field"), Effects::READS);
    assert_eq!(e("show"), Effects::IO | Effects::READS, "printing looks at what it prints");
    assert_eq!(e("show").purity(), Purity::Impure);
    assert_eq!(e("push"), Effects::WRITES);
    assert_eq!(e("mk"), Effects::ALLOCS);
    assert_eq!(e("mk").purity(), Purity::Const);
    // calls take on what the callee does
    assert_eq!(e("indirect"), Effects::NONE);
    assert_eq!(e("noisy"), Effects::IO | Effects::READS);
    // recursion and loops may not terminate
    for name in ["fact", "even", "odd", "spin"] {
        assert!(e(name).diverges(), "{name}: {}", e(name));
    }
    assert!(e("fact").traps() && !e("fact").writes());
    assert_eq!(e("even"), e("odd"), "members of a cycle share their effects");
    assert!(!e("add").diverges() && !e("sq").diverges());
    // an unknown callee can do anything
    assert_eq!(e("via"), Effects::ALL);
    assert_eq!(e("via").purity(), Purity::Impure);
}

fn only_dce() -> OptConfig {
    OptConfig { dce: true, ..OptConfig::none() }
}

fn calls_in(ir: &IrProgram, name: &str) -> usize {
    fn_named(ir, name).blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::Call { .. })).count()
}

#[test]
fn unused_calls_to_harmless_functions_are_removed() {
    let prelude = "fn sq(x: f64) -> f64 { x * x }\n\
        fn mk() -> [i64] { [1, 2] }\n\
        fn add(a: i64, b: i64) -> i64 { a + b }\n\
        fn show(x: i64) { println(x); }\n\
        fn spin(n: i64) -> i64 { var i = 0; while i < n { i += 1; } i }\n\
        fn fact(n: i64) -> i64 { if n < 2 { 1 } else { n * fact(n - 1) } }\n";
    let src = format!(
        "{prelude}fn f(a: f64) -> f64 {{ sq(a); mk(); a }}\nfn g(a: i64) -> i64 {{ add(a, 1); show(a); spin(a); fact(a); a }}\nfn main() {{ println(f(1.0)); println(g(3)); }}"
    );
    let (ir, _) = optimized(&src, only_dce());
    // const functions that cannot trap or loop and merely allocate: the calls vanish
    assert_eq!(calls_in(&ir, "f"), 0, "{ir}");
    // may trap, print, loop forever or recurse: they stay
    assert_eq!(calls_in(&ir, "g"), 4, "{ir}");
    assert_eq!(run(&src), "1.0\n3\n3\n");
}

#[test]
fn removing_a_call_never_hides_a_trap_or_output() {
    assert_eq!(
        trap_of("fn add(a: i64, b: i64) -> i64 { a + b }\nfn main() { add(9223372036854775807, 1); }"),
        Trap::Overflow
    );
    assert_eq!(run("fn show(x: i64) { println(x); }\nfn main() { show(7); show(8); }"), "7\n8\n");
}

#[test]
fn identical_calls_to_const_and_pure_functions_are_computed_once() {
    let src = "fn sq(x: f64) -> f64 { x * x }\nfn f(a: f64) -> f64 { sq(a) + sq(a) }\nprintln(f(3.0));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(calls_in(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "18.0\n");
    // a pure reader, until something writes
    let src = "fn len_of(a: [i64]) -> i64 { a.len() }\n\
        fn f(var a: [i64]) -> i64 { let x = len_of(a); let y = len_of(a); a.push(1); let z = len_of(a); x * 100 + y * 10 + z }\n\
        println(f([5]));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(calls_in(&ir, "f"), 2, "{ir}");
    assert_eq!(run(src), "112\n");
    // a call that writes (through a helper) forgets what was read before it
    let src = "fn len_of(a: [i64]) -> i64 { a.len() }\nfn grow(var a: [i64]) { a.push(0); }\n\
        fn f(var a: [i64]) -> i64 { let x = len_of(a); grow(a); let y = len_of(a); x * 10 + y }\n\
        println(f([5]));";
    assert_eq!(run(src), "12\n");
}

#[test]
fn calls_that_allocate_or_print_are_never_merged() {
    // two calls, two different arrays
    let src = "fn mk() -> [i64] { [1] }\nfn f() -> i64 { var a = mk(); var b = mk(); a.push(2); b.len() * 10 + a.len() }\nprintln(f());";
    assert_eq!(run(src), "12\n");
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(calls_in(&ir, "f"), 2, "{ir}");
    // printing twice prints twice
    assert_eq!(run("fn show(x: i64) { println(x); }\nfn f() { show(1); show(1); }\nf();"), "1\n1\n");
    // a repeated trapping call still traps
    assert_eq!(
        trap_of("fn d(a: i64, b: i64) -> i64 { a / b }\nfn f(z: i64) -> i64 { d(1, z) + d(1, z) }\nprintln(f(0));"),
        Trap::DivideByZero
    );
}

#[test]
fn calls_that_do_not_write_keep_what_was_known_about_memory() {
    let src = "struct P { x: i64 }\nfn read(p: P) -> i64 { p.x }\n\
        fn f(p: P) -> i64 { let a = p.x; let r = read(p); let b = p.x; a + r + b }\nprintln(f(P { x: 2 }));";
    let (ir, _) = optimized(src, only_simplify());
    let loads =
        fn_named(&ir, "f").blocks.iter().flat_map(|b| &b.insts).filter(|i| matches!(i, Inst::GetField { .. })).count();
    assert_eq!(loads, 1, "{ir}");
    assert_eq!(run(src), "6\n");
    // ... but a callee that writes the field is seen
    let src = "struct P { x: i64 }\nfn bump(var p: P) { p.x += 1; }\n\
        fn f(var p: P) -> i64 { let a = p.x; bump(p); let b = p.x; a * 10 + b }\nprintln(f(P { x: 2 }));";
    assert_eq!(run(src), "23\n");
}

/// Whether block `b` can reach itself.
fn in_cycle(f: &super::IrFunc, b: usize) -> bool {
    let mut seen = vec![false; f.blocks.len()];
    let mut stack: Vec<usize> = f.blocks[b].term.successors().iter().map(|s| s.index()).collect();
    while let Some(x) = stack.pop() {
        if x == b {
            return true;
        }
        if !std::mem::replace(&mut seen[x], true) {
            stack.extend(f.blocks[x].term.successors().iter().map(|s| s.index()));
        }
    }
    false
}

fn loop_calls(ir: &IrProgram, name: &str) -> usize {
    let f = fn_named(ir, name);
    (0..f.blocks.len())
        .filter(|&b| in_cycle(f, b))
        .map(|b| f.blocks[b].insts.iter().filter(|i| matches!(i, Inst::Call { .. })).count())
        .sum()
}

#[test]
fn calls_to_pure_functions_leave_loops() {
    let src = "fn sq(x: f64) -> f64 { x * x }\n\
        fn f(n: i64, k: f64) -> f64 { var s = 0.0; var i = 0; while i < n { s += sq(k); i += 1; } s }\nprintln(f(3, 2.0)); println(f(0, 2.0));";
    let (ir, stats) = optimized(src, OptConfig { licm: true, ..only_simplify() });
    assert!(stats.hoisted > 0, "{ir}");
    assert_eq!(loop_calls(&ir, "f"), 0, "{ir}");
    assert_eq!(run(src), "12.0\n0.0\n");
    // a reader moves out only if the loop writes nothing
    let src = "fn len_of(a: [i64]) -> i64 { a.len() }\n\
        fn f(a: [i64], n: i64) -> i64 { var s = 0; var i = 0; while i < n { s += len_of(a); i += 1; } s }\nprintln(f([1, 2], 3));";
    let (ir, _) = optimized(src, OptConfig { licm: true, ..only_simplify() });
    assert_eq!(loop_calls(&ir, "f"), 0, "{ir}");
    assert_eq!(run(src), "6\n");
    let src = "fn len_of(a: [i64]) -> i64 { a.len() }\n\
        fn f(var a: [i64]) -> i64 { var s = 0; var i = 0; while i < 3 { s += len_of(a); a.push(i); i += 1; } s }\nprintln(f([1]));";
    let (ir, _) = optimized(src, OptConfig { licm: true, ..only_simplify() });
    assert_eq!(loop_calls(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "6\n");
}

#[test]
fn calls_that_could_trap_print_or_allocate_stay_in_loops() {
    for (name, src, out) in [
        ("trap", "fn d(a: i64, b: i64) -> i64 { a / b }\nfn f(n: i64, z: i64) -> i64 { var s = 0; var i = 0; while i < n { s += d(1, z); i += 1; } s }\nprintln(f(0, 0));", "0\n"),
        ("print", "fn show(x: i64) { println(x); }\nfn f(n: i64) { var i = 0; while i < n { show(7); i += 1; } }\nf(2);", "7\n7\n"),
        ("alloc", "fn mk() -> [i64] { [1] }\nfn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { var a = mk(); a.push(2); s += a.len(); i += 1; } s }\nprintln(f(3));", "6\n"),
        ("loop", "fn spin(n: i64) -> i64 { var i = 0; while i < n { i += 1; } i }\nfn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { s += spin(0); i += 1; } s }\nprintln(f(2));", "0\n"),
    ] {
        let (ir, _) = optimized(src, OptConfig { licm: true, ..only_simplify() });
        assert!(loop_calls(&ir, "f") >= 1, "{name}: {ir}");
        assert_eq!(run(src), out, "{name}");
    }
}

// ---- escape analysis and scalar replacement ------------------------------------------------------

fn func_id(ir: &IrProgram, name: &str) -> super::FuncId {
    super::FuncId(ir.funcs.iter().position(|f| f.name == name).unwrap_or_else(|| panic!("no function {name}")) as u32)
}

#[test]
fn parameter_escape_summaries() {
    let src = "struct P { x: i64 }\nvar holder = P { x: 0 };\n\
        fn read(p: P) -> i64 { p.x }\n\
        fn store(p: P) { holder = p; }\n\
        fn give(p: P) -> P { p }\n\
        fn put(var a: [P], p: P) { a.push(p); }\n\
        fn pass(p: P) -> i64 { read(p) }\n\
        fn pass_on(p: P) { store(p); }\n\
        fn count(p: P, n: i64) -> i64 { if n == 0 { p.x } else { count(p, n - 1) } }\n\
        fn len_of(a: [P]) -> i64 { a.len() }\n\
        fn main() {\n\
            let p = P { x: 1 }; var a: [P] = [];\n\
            read(p); store(p); give(p); put(a, p); pass(p); pass_on(p); count(p, 2); len_of(a);\n\
        }";
    let ir = compile(src);
    let fx = super::analyze_effects(&ir);
    let esc = |name: &str, i: usize| fx.param_escapes(func_id(&ir, name), i);
    assert!(!esc("read", 0) && !esc("pass", 0) && !esc("count", 0) && !esc("len_of", 0));
    assert!(esc("store", 0), "stored in a global");
    assert!(esc("give", 0), "returned: the result aliases it");
    assert!(esc("put", 1), "stored in an array");
    assert!(!esc("put", 0), "the array itself is only written to");
    assert!(esc("pass_on", 0), "passed on to something that keeps it");
}

#[test]
fn writes_to_private_objects_do_not_make_a_function_impure() {
    let src = "struct P { x: i64 }\n\
        fn bump(var p: P) { p.x += 1; }\n\
        fn local_struct() -> i64 { var p = P { x: 1 }; p.x += 1; p.x }\n\
        fn local_array(n: i64) -> i64 { var a: [i64] = []; var i = 0; while i < n { a.push(i); i += 1; } a.len() }\n\
        fn build(n: i64) -> [i64] { var a: [i64] = []; a.push(n); a }\n\
        fn leaky(var a: [i64]) -> i64 { var b = a; b.push(1); b.len() }\n\
        fn main() { var p = P { x: 0 }; bump(p); println(local_struct()); println(local_array(3)); println(build(1).len()); println(leaky([1])); }";
    let ir = compile(src);
    let fx = super::analyze_effects(&ir);
    let e = |name: &str| effects_of(&ir, &fx, name);
    assert!(e("bump").writes(), "a parameter is not private");
    assert!(!e("local_struct").writes() && !e("local_struct").reads(), "{}", e("local_struct"));
    assert_eq!(e("local_struct").purity(), Purity::Const);
    assert!(!e("local_array").writes() && !e("local_array").reads(), "{}", e("local_array"));
    assert_eq!(e("build"), Effects::ALLOCS, "writes to the fresh result are invisible");
    assert!(e("leaky").writes(), "it changes the caller's array through an alias");
}

fn count_in(ir: &IrProgram, name: &str, pred: impl Fn(&Inst) -> bool) -> usize {
    fn_named(ir, name).blocks.iter().flat_map(|b| &b.insts).filter(|i| pred(i)).count()
}

#[test]
fn private_structs_become_registers() {
    let src = "struct P { x: i64, y: i64 }\n\
        fn dist(a: i64, b: i64) -> i64 { let p = P { x: a, y: b }; var q = P { x: p.y, y: p.x }; q.x += 1; q.x * q.y }\n\
        fn main() { println(dist(2, 3)); println(dist(5, 1)); }";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(
        count_in(&ir, "dist", |i| matches!(i, Inst::NewStruct { .. } | Inst::GetField { .. } | Inst::SetField { .. })),
        0,
        "{ir}"
    );
    assert_eq!(run(src), "8\n10\n");
    // one allocation per iteration of a loop: the fields are rewritten each time
    let src = "struct P { x: i64, y: i64 }\nfn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { let p = P { x: i, y: 2 }; s += p.x * p.y; i += 1; } s }\nprintln(f(5)); println(f(0));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::NewStruct { .. })), 0, "{ir}");
    assert_eq!(run(src), "20\n0\n");
}

#[test]
fn escaping_structs_keep_their_allocation() {
    let keep = |src: &str, name: &str, out: &str| {
        let (ir, _) = optimized(src, only_simplify());
        assert_eq!(count_in(&ir, name, |i| matches!(i, Inst::NewStruct { .. })), 1, "{name}: {ir}");
        assert_eq!(run(src), out, "{name}");
    };
    keep("struct P { x: i64 }\nfn mk(a: i64) -> P { P { x: a } }\nprintln(mk(3).x);", "mk", "3\n");
    keep(
        "struct P { x: i64 }\nvar g = P { x: 0 };\nfn f(a: i64) { let p = P { x: a }; g = p; }\nf(4); println(g.x);",
        "f",
        "4\n",
    );
    keep(
        "struct P { x: i64 }\nfn f(a: i64) -> i64 { var v: [P] = []; v.push(P { x: a }); v[0].x }\nprintln(f(6));",
        "f",
        "6\n",
    );
    keep(
        "struct P { x: i64 }\nfn read(p: P) -> i64 { p.x }\nfn f(a: i64) -> i64 { read(P { x: a }) }\nprintln(f(7));",
        "f",
        "7\n",
    );
    // a closure that captures it
    keep(
        "struct P { x: i64 }\nfn f(a: i64) -> i64 { let p = P { x: a }; let g = || => p.x; g() }\nprintln(f(8));",
        "f",
        "8\n",
    );
}

#[test]
fn aliases_are_not_split() {
    // `q` and `p` are one object
    let src = "struct P { x: i64 }\nfn f() -> i64 { var p = P { x: 1 }; var q = p; q.x = 5; p.x }\nprintln(f());";
    assert_eq!(run(src), "5\n");
    let src = "struct P { x: i64 }\nfn f(c: bool) -> i64 { var p = P { x: 1 }; var q = P { x: 2 }; if c { q = p; } q.x = 9; p.x * 10 + q.x }\nprintln(f(true)); println(f(false));";
    assert_eq!(run(src), "99\n19\n");
}

#[test]
fn matches_on_private_enums_are_decided_at_compile_time() {
    let src = "fn f(a: i64) -> i64 { let o = Option::Some(a); match o { Option::Some(x) => x + 1, Option::None => 0 } }\n\
        fn g(a: i64) -> i64 { let o: Option<i64> = if a > 0 { Option::Some(a) } else { Option::None }; match o { Option::Some(x) => x, Option::None => -1 } }\n\
        fn main() { println(f(4)); println(g(3)); println(g(-3)); }";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(
        count_in(&ir, "f", |i| matches!(i, Inst::NewEnum { .. } | Inst::EnumTag { .. } | Inst::EnumField { .. })),
        0,
        "{ir}"
    );
    assert!(fn_named(&ir, "f").blocks.iter().all(|b| !matches!(b.term, super::Term::BranchTag { .. })), "{ir}");
    assert_eq!(run(src), "5\n3\n-1\n");
}

#[test]
fn write_only_private_objects_disappear_with_their_writes() {
    use crate::builtin::Builtin;
    let src = "fn build(n: i64) -> i64 { var log: [i64] = []; var i = 0; while i < n { log.push(i); i += 1; } n * 2 }\nfn main() { println(build(3)); }";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(
        count_in(&ir, "build", |i| matches!(i, Inst::NewArray { .. }) || is_intrinsic(i, Builtin::ArrPush)),
        0,
        "{ir}"
    );
    assert_eq!(run(src), "6\n");
    // read, returned or handed on: it stays
    let src = "fn f(n: i64) -> i64 { var a: [i64] = []; a.push(n); a.len() }\nprintln(f(2));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count_in(&ir, "f", |i| is_intrinsic(i, Builtin::ArrPush)), 1, "{ir}");
    assert_eq!(run(src), "1\n");
    assert_eq!(
        run("fn f(var a: [i64]) { var b: [i64] = []; b.push(1); a.push(2); }\nvar x = [0]; f(x); println(x.len());"),
        "2\n"
    );
    // popping can trap on an empty array: not a write-only use
    assert_eq!(trap_of("fn f() { var a: [i64] = []; a.pop(); }\nf();"), Trap::EmptyArray);
}

// ---- control-flow shaping, copies, globals ----------------------------------------------------------

fn only_tail() -> OptConfig {
    OptConfig { simplify: true, dce: true, fold: true, ..OptConfig::none() }
}

#[test]
fn self_tail_calls_become_loops() {
    let src =
        "fn sum(n: i64, acc: i64) -> i64 { if n == 0 { acc } else { sum(n - 1, acc + n) } }\nprintln(sum(30, 0));";
    let (ir, _) = optimized(src, only_tail());
    assert_eq!(calls_in(&ir, "sum"), 0, "{ir}");
    assert_eq!(run(src), "465\n");
    // 100000 frames would not fit; as a loop it runs in constant stack
    let deep =
        "fn sum(n: i64, acc: i64) -> i64 { if n == 0 { acc } else { sum(n - 1, acc + n) } }\nprintln(sum(100000, 0));";
    let (ir, _) = optimized(deep, OptConfig::default());
    let r = interp::run(&ir, &[]);
    assert!(r.trap.is_none(), "{:?}", r.trap);
    assert_eq!(r.output, "5000050000\n");
    let unoptimized = interp::run(&compile(deep), &[]);
    assert_eq!(unoptimized.trap, Some(Trap::StackOverflow));
    // the arguments swap: they must be read before any parameter changes
    let src = "fn f(a: i64, b: i64, n: i64) -> i64 { if n == 0 { a * 100 + b } else { f(b, a, n - 1) } }\nprintln(f(1, 2, 3)); println(f(1, 2, 4));";
    assert_eq!(run(src), "201\n102\n");
    // not a tail call: the result is used afterwards
    let src = "fn g(n: i64) -> i64 { if n == 0 { 0 } else { g(n - 1) + 1 } }\nprintln(g(10));";
    let (ir, _) = optimized(src, only_tail());
    assert_eq!(calls_in(&ir, "g"), 1, "{ir}");
    assert_eq!(run(src), "10\n");
    // a parameter captured by a closure keeps its own cell per call
    let src = "fn h(n: i64, fs: [fn() -> i64]) -> i64 { var fs2 = fs; fs2.push(|| => n); if n == 0 { var s = 0; for f in fs2 { s += f(); } s } else { h(n - 1, fs2) } }\nprintln(h(3, []));";
    assert_eq!(run(src), "6\n");
    // unbounded tail recursion is an endless loop now, not a stack overflow: bounded by the step limit only
}

#[test]
fn returns_are_duplicated_into_the_jumps_that_reach_them() {
    let src = "fn f(a: i64) -> i64 { var r = 0; if a > 0 { r = 1; } elif a < 0 { r = 2; } r }\nprintln(f(5)); println(f(-5)); println(f(0));";
    assert_eq!(run(src), "1\n2\n0\n");
    let (ir, _) = optimized(src, only_tail());
    let returns = fn_named(&ir, "f").blocks.iter().filter(|b| matches!(b.term, super::Term::Return(_))).count();
    assert!(returns >= 2, "{ir}");
}

#[test]
fn branches_on_constants_set_by_the_jumping_block_are_threaded() {
    let src = "fn f(a: bool, b: bool) -> i64 { if a && b { 1 } else { 2 } }\nprintln(f(true, true)); println(f(true, false)); println(f(false, true));";
    assert_eq!(run(src), "1\n2\n2\n");
    let src = "fn f(a: bool, b: bool, c: bool) -> i64 { if (a || b) && c { 1 } else { 2 } }\n\
        println(f(false, false, true)); println(f(false, true, true)); println(f(true, false, false)); println(f(true, true, true));";
    assert_eq!(run(src), "2\n1\n2\n1\n");
}

#[test]
fn boolean_diamonds_are_plain_assignments() {
    let src = "fn f(c: bool) -> bool { if c { true } else { false } }\nfn g(c: bool) -> bool { if c { false } else { true } }\n\
        println(f(true)); println(f(false)); println(g(true)); println(g(false));";
    assert_eq!(run(src), "true\nfalse\nfalse\ntrue\n");
    let (ir, _) = optimized(src, only_tail());
    assert!(fn_named(&ir, "f").blocks.len() <= 2, "{ir}");
    assert!(fn_named(&ir, "g").blocks.len() <= 2, "{ir}");
}

#[test]
fn copies_are_propagated_across_blocks_and_loops() {
    let src = "fn f(a: i64, n: i64) -> i64 { var b = a; var s = 0; var i = 0; while i < n { s += b; i += 1; } s }\nprintln(f(3, 4));";
    assert_eq!(run(src), "12\n");
    // a copy that is overwritten on one path must not be used after the join
    let src = "fn f(a: i64, c: bool) -> i64 { var b = a; if c { b = 100; } b + a }\nprintln(f(1, true)); println(f(1, false));";
    assert_eq!(run(src), "101\n2\n");
    // the source changes inside the loop
    let src = "fn f(n: i64) -> i64 { var a = 1; var b = a; var i = 0; while i < n { a = a + 1; i += 1; } b * 100 + a }\nprintln(f(3));";
    assert_eq!(run(src), "104\n");
}

#[test]
fn global_values_are_forwarded_and_unread_globals_disappear() {
    let src = "var counter = 0;\nvar unused = 5;\nfn bump() { counter += 1; }\nbump(); bump(); println(counter);";
    assert_eq!(run(src), "2\n");
    let (ir, _) = optimized(src, OptConfig::default());
    let names: Vec<&str> = ir.globals.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, ["counter"], "{names:?}");
    // a store, then a load: no load
    let src = "var g = 0;\nfn f(x: i64) -> i64 { g = x; g + g }\nprintln(f(4)); println(g);";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::GetGlobal { .. })), 0, "{ir}");
    assert_eq!(run(src), "8\n4\n");
    // a call may change the global
    let src =
        "var g = 1;\nfn set() { g = 9; }\nfn f() -> i64 { let a = g; set(); let b = g; a * 10 + b }\nprintln(f());";
    assert_eq!(run(src), "19\n");
    // a pure call does not
    let src = "var g = 1;\nfn read() -> i64 { g }\nfn f() -> i64 { let a = g; let r = read(); let b = g; a + r + b }\nprintln(f());";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::GetGlobal { .. })), 1, "{ir}");
    assert_eq!(run(src), "3\n");
    // globals read in closures
    assert_eq!(run("var g = 0;\nlet inc = || => { g += 1; g };\ninc(); inc(); println(g);"), "2\n");
}

#[test]
fn constants_propagate_through_joins_and_untaken_branches() {
    let only_fold = || OptConfig { fold: true, dce: true, ..OptConfig::none() };
    // both arms store the same constant
    let src = "fn f(c: bool) -> i64 { var x = 1; if c { x = 2; } else { x = 2; } x + 1 }\nprintln(f(true)); println(f(false));";
    let (ir, _) = optimized(src, only_fold());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. })), 0, "{ir}");
    assert_eq!(run(src), "3\n3\n");
    // only one arm is ever taken, so only its value reaches the join
    let src = "fn f() -> i64 { var x = 5; var y = 0; if x > 3 { y = 1; } else { y = 2; } y * 10 }\nprintln(f());";
    let (ir, _) = optimized(src, only_fold());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. } | Inst::Cmp { .. })), 0, "{ir}");
    assert!(fn_named(&ir, "f").blocks.len() <= 2, "{ir}");
    assert_eq!(run(src), "10\n");
    // different constants on the two arms, a parameter, and a loop-carried value stay computed
    let src = "fn f(c: bool) -> i64 { var x = 1; if c { x = 2; } x + 1 }\nprintln(f(true)); println(f(false));";
    let (ir, _) = optimized(src, only_fold());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. })), 1, "{ir}");
    assert_eq!(run(src), "3\n2\n");
    let src = "fn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { s += i; i += 1; } s }\nprintln(f(5));";
    let (ir, _) = optimized(src, only_fold());
    assert!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. })) >= 2, "{ir}");
    assert_eq!(run(src), "10\n");
    // a loop whose exit test is decided by constants collapses to its result
    let src = "fn f() -> i64 { var i = 0; while i < 0 { i += 1; } i + 7 }\nprintln(f());";
    let (ir, _) = optimized(src, only_fold());
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. })), 0, "{ir}");
    assert_eq!(run(src), "7\n");
    // an operation that would trap stays and traps
    let src = "fn f(z: i64) -> i64 { var d = 0; if z > 100 { d = 1; } 10 / d }\nprintln(f(1));";
    assert_eq!(trap_of(src), Trap::DivideByZero);
    // captured variables change behind the analysis
    let src = "fn f() -> i64 { var x = 1; let bump = || => { x += 1; }; bump(); bump(); x }\nprintln(f());";
    assert_eq!(run(src), "3\n");
}

#[test]
fn overwritten_stores_are_dropped_but_observed_ones_stay() {
    let cfg = only_simplify;
    let sets = |ir: &IrProgram, name: &str| {
        count_in(ir, name, |i| matches!(i, Inst::SetField { .. } | Inst::ArraySet { .. } | Inst::SetGlobal { .. }))
    };
    // consecutive stores to a global
    let src = "var g = 0;\nfn f() { g = 1; g = 2; }\nf(); println(g);";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(sets(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "2\n");
    // a read in between keeps the first store
    let src = "var g = 0;\nfn f() { g = 1; println(g); g = 2; }\nf(); println(g);";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(sets(&ir, "f"), 2, "{ir}");
    assert_eq!(run(src), "1\n2\n");
    // a call in between may read it
    let src = "var g = 0;\nfn show() { println(g); }\nfn f() { g = 1; show(); g = 2; }\nf(); show();";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(sets(&ir, "f"), 2, "{ir}");
    assert_eq!(run(src), "1\n2\n");
    // fields and elements
    let src = "struct P { x: i64, y: i64 }\nfn f(var p: P, var a: [i64]) { p.x = 1; p.y = 5; p.x = 2; a[0] = 7; a[0] = 8; }\nvar p = P { x: 0, y: 0 }; var a = [0, 0];\nf(p, a); println(p.x); println(p.y); println(a[0]);";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(sets(&ir, "f"), 3, "{ir}");
    assert_eq!(run(src), "2\n5\n8\n");
    // a different index may alias: the first store survives a read of another element
    let src = "fn f(var a: [i64], i: i64, j: i64) -> i64 { a[i] = 1; let v = a[j]; a[i] = 2; v }\nvar a = [0, 0]; println(f(a, 0, 0)); println(a[0]);";
    assert_eq!(run(src), "1\n2\n");
    // an out-of-range store still traps even when the same slot is stored again
    assert_eq!(
        trap_of("fn f(var a: [i64]) { a[5] = 1; a[5] = 2; }\nf([1]);"),
        Trap::IndexOutOfBounds { index: 5, len: 1 }
    );
}

#[test]
fn small_constant_loops_are_unrolled() {
    let cfg = || OptConfig { unroll: true, ..only_simplify() };
    let branches = |ir: &IrProgram, name: &str| {
        fn_named(ir, name).blocks.iter().filter(|b| matches!(b.term, Term::Branch { .. })).count()
    };
    // everything is known: no loop and no arithmetic left
    let src = "fn f() -> i64 { var s = 0; var i = 0; while i < 4 { s += i * 2; i += 1; } s }\nprintln(f());";
    let (ir, stats) = optimized(src, cfg());
    assert_eq!(stats.unrolled, 1, "{ir}");
    assert_eq!(branches(&ir, "f"), 0, "{ir}");
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Arith { .. })), 0, "{ir}");
    assert_eq!(run(src), "12\n");
    // the body keeps its effects, in order
    let src = "fn f() { var i = 0; while i < 3 { println(i); i += 1; } }\nf();";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(branches(&ir, "f"), 0, "{ir}");
    assert_eq!(count_in(&ir, "f", |i| matches!(i, Inst::Intrinsic { .. })), 3, "{ir}");
    assert_eq!(run(src), "0\n1\n2\n");
    // a break inside
    let src = "fn f() -> i64 { var i = 0; while i < 6 { if i == 3 { break; } i += 1; } i }\nprintln(f());";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(branches(&ir, "f"), 0, "{ir}");
    assert_eq!(run(src), "3\n");
    // nested loops: the inner one goes first, then the outer one is small enough
    let src = "fn f() -> i64 { var s = 0; var i = 0; while i < 3 { var j = 0; while j < 2 { s += 1; j += 1; } i += 1; } s }\nprintln(f());";
    let (ir, stats) = optimized(src, cfg());
    assert_eq!(stats.unrolled, 2, "{ir}");
    assert_eq!(branches(&ir, "f"), 0, "{ir}");
    assert_eq!(run(src), "6\n");
    // exactly as many iterations as there are copies; one more is left as a loop
    let src = "fn f() -> i64 { var s = 0; var i = 0; while i < 8 { s += 1; i += 1; } s }\nprintln(f());";
    let (ir, _) = optimized(src, cfg());
    assert_eq!(branches(&ir, "f"), 0, "{ir}");
    assert_eq!(run(src), "8\n");
    let src = "fn f() -> i64 { var s = 0; var i = 0; while i < 9 { s += 1; i += 1; } s }\nprintln(f());";
    let (ir, stats) = optimized(src, cfg());
    assert_eq!(stats.unrolled, 0, "{ir}");
    assert_eq!(branches(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "9\n");
    // a bound that is not known stays a loop
    let src = "fn f(n: i64) -> i64 { var s = 0; var i = 0; while i < n { s += i; i += 1; } s }\nprintln(f(5));";
    let (ir, stats) = optimized(src, cfg());
    assert_eq!(stats.unrolled, 0, "{ir}");
    assert_eq!(branches(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "10\n");
    // never entered
    assert_eq!(run("fn f() -> i64 { var i = 5; while i < 3 { i += 1; } i }\nprintln(f());"), "5\n");
}

#[test]
fn values_are_shared_across_joins_when_every_path_agrees() {
    let muls = |ir: &IrProgram, name: &str| {
        count_in(ir, name, |i| matches!(i, Inst::Arith { op: crate::runtime::numeric::ArithOp::Mul, .. }))
    };
    // computed before the branch, needed again after the join
    let src = "fn f(a: i64, b: i64, c: bool) -> i64 { let x = a * b; var y = 0; if c { y = 1; } else { y = 2; } x + a * b + y }\nprintln(f(3, 4, true)); println(f(3, 4, false));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(muls(&ir, "f"), 1, "{ir}");
    assert_eq!(run(src), "25\n26\n");
    // computed on both arms of the diamond
    let src = "fn f(a: i64, b: i64, c: bool) -> i64 { var y = 0; if c { y = a * b; } else { y = a * b + 1; } y + a * b }\nprintln(f(3, 4, true)); println(f(3, 4, false));";
    assert_eq!(run(src), "24\n25\n");
    // one arm changes an operand: the product is not available after the join
    let src = "fn f(a: i64, b: i64, c: bool) -> i64 { var p = a; let x = p * b; if c { p = p + 1; } x + p * b }\nprintln(f(3, 4, true)); println(f(3, 4, false));";
    let (ir, _) = optimized(src, only_simplify());
    assert_eq!(muls(&ir, "f"), 2, "{ir}");
    assert_eq!(run(src), "28\n24\n");
    // one arm writes memory that the load depends on
    let src = "fn f(var a: [i64], c: bool) -> i64 { let x = a[0]; if c { a[0] = 9; } x + a[0] }\nvar a = [1]; println(f(a, true)); var b = [1]; println(f(b, false));";
    assert_eq!(run(src), "10\n2\n");
    // loop headers start empty: a value changed by the loop body is not reused
    let src = "fn f(n: i64, b: i64) -> i64 { var p = 1; var s = 0; var i = 0; let x = p * b; while i < n { p = p + 1; s += p * b; i += 1; } s + x + p * b }\nprintln(f(3, 2));";
    assert_eq!(run(src), "28\n");
}
