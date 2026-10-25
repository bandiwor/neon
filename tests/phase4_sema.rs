//! Phase 4 end-to-end: multi-module programs through the Vfs, type checking,
//! exhaustiveness, closures, traits, and rendered diagnostics.

use neon::diag::{DiagBag, Emitter, SourceMap};
use neon::sema::{analyze, Callee, Program, Res};
use neon::syntax::Interner;
use neon::vfs::MemVfs;

struct Run {
    prog: Program,
    interner: Interner,
    diags: DiagBag,
    map: SourceMap,
}

fn run(vfs: &MemVfs, entry: &str) -> Run {
    let mut map = SourceMap::new();
    let mut interner = Interner::new();
    let mut diags = DiagBag::new();
    let prog = analyze(entry, vfs, &mut map, &mut interner, &mut diags);
    Run { prog, interner, diags, map }
}

impl Run {
    fn render(&self) -> String {
        Emitter::new(&self.map).render_all(&self.diags)
    }
}

#[test]
fn headline_programs_type_check() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        r#"
fn fact(n: u64) -> u64 { if n <= 1 { 1 } else { n * fact(n - 1) } }
fn fib(n: i32) -> i32 { if n < 2 { n } else { fib(n - 1) + fib(n - 2) } }

enum Shape { Circle(f64), Rect(f64, f64), Empty }
fn area(s: Shape) -> f64 {
    match s {
        Shape::Circle(r) => 3.14159 * r * r,
        Shape::Rect(w, h) => w * h,
        Shape::Empty => 0.0,
    }
}

fn make_counter() -> fn() -> i32 {
    let mut count = 0;
    return || => { count += 1; count };
}

let next = make_counter();
let a = next();
let b = next();
println(fact(20));
println(fib(15));
println(area(Shape::Rect(2.0, 3.0)));
"#,
    );
    let r = run(&vfs, "main.neon");
    assert!(r.diags.is_empty(), "{}", r.render());

    // the closure captured exactly `count`
    let entry = r.prog.entry().unwrap();
    assert_eq!(entry.info.closures.len(), 1);
    let closure = entry.info.closures.values().next().unwrap();
    assert_eq!(closure.captures.len(), 1);
    assert_eq!(r.interner.resolve(r.prog.local(closure.captures[0]).name), "count");
    assert!(r.prog.local(closure.captures[0]).captured);
}

#[test]
fn module_dag_with_shared_dependency() {
    let vfs = MemVfs::new()
        .with_file(
            "app/main.neon",
            "import { Shape, area } from \"../geo/shapes\";\nimport { scale } from \"../geo/ops\";\nlet s = scale(Shape::Square(2.0), 3.0);\nlet a = area(s);",
        )
        .with_file(
            "geo/shapes.neon",
            "export enum Shape { Square(f64), Disc(f64) }\nexport fn area(s: Shape) -> f64 { match s { Shape::Square(x) => x * x, Shape::Disc(r) => 3.14 * r * r } }",
        )
        .with_file(
            "geo/ops.neon",
            "import { Shape } from \"shapes\";\nexport fn scale(s: Shape, k: f64) -> Shape { match s { Shape::Square(x) => Shape::Square(x * k), Shape::Disc(r) => Shape::Disc(r * k) } }",
        );
    let r = run(&vfs, "app/main.neon");
    assert!(r.diags.is_empty(), "{}", r.render());
    // prelude, shapes, ops, main: dependencies first
    let paths: Vec<&str> = r.prog.modules.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, ["<prelude>", "geo/shapes.neon", "geo/ops.neon", "app/main.neon"]);
}

#[test]
fn errors_in_several_modules_are_all_reported_with_locations() {
    let vfs = MemVfs::new()
        .with_file("main.neon", "import { f } from \"lib\";\nlet x: i32 = f();\nlet y = missing;\n")
        .with_file("lib.neon", "export fn f() -> string { 1 }\n");
    let r = run(&vfs, "main.neon");
    let out = r.render();
    assert_eq!(r.diags.error_count(), 3, "{out}");
    assert!(out.contains("--> lib.neon:1:27"), "{out}");
    assert!(out.contains("--> main.neon:2:14"), "{out}");
    assert!(out.contains("--> main.neon:3:9"), "{out}");
    assert!(out.contains("error[E0211]: mismatched types: expected `string`, found `{integer}`"), "{out}");
    assert!(out.contains("error[E0210]: cannot find value `missing`"), "{out}");
}

#[test]
fn analysis_survives_syntax_errors_in_partially_parsed_modules() {
    let vfs = MemVfs::new().with_file("main.neon", "fn ok() -> i32 { 1 }\nlet broken = ;\nlet fine = ok();\n");
    let r = run(&vfs, "main.neon");
    // the syntax error is reported once; the valid code is still analyzed
    assert_eq!(r.diags.error_count(), 1, "{}", r.render());
    let entry = r.prog.entry().unwrap();
    assert!(entry.scope.items.contains_key(&r.interner.get("fine").unwrap()));
}

#[test]
fn resolutions_and_callees_are_recorded_for_lowering() {
    let vfs = MemVfs::new().with_file(
        "main.neon",
        "struct P { x: i32 }\nimpl P { fn get(self) -> i32 { self.x } }\nfn id<T>(v: T) -> T { v }\nlet p = P { x: 1 };\nlet a = p.get();\nlet b = id(2u8);\nlet c = [1, 2].len();",
    );
    let r = run(&vfs, "main.neon");
    assert!(r.diags.is_empty(), "{}", r.render());
    let info = &r.prog.entry().unwrap().info;
    let mut kinds = info.callees.values().map(|c| matches!(c, Callee::Fn(..))).collect::<Vec<_>>();
    kinds.sort();
    assert_eq!(kinds, [false, true]); // builtin len, user method get
                                      // `id(2u8)` resolved to the generic fn instantiated at u8
    let id_call = info.res.values().find_map(|res| match res {
        Res::Callee(Callee::Fn(f, args)) if r.interner.resolve(r.prog.func(*f).name) == "id" => Some(args.clone()),
        _ => None,
    });
    let args = id_call.expect("call of id recorded");
    assert_eq!(args.len(), 1);
    assert_eq!(r.prog.show_ty(&r.interner, args[0], &Default::default()), "u8");
}
