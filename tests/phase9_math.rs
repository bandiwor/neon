//! Boundary-value arithmetic through the whole pipeline: for every numeric
//! type and operator, programs built from values at and around the limits must
//! behave identically in the reference interpreter and in the VM (output and
//! trap), with and without optimization.

use neon::compiler::CompileOptions;
use neon::driver::{compile_source, Options};
use neon::ir::{interp, OptConfig};
use neon::runtime::numeric::{arith, encode_int, ArithOp, NumTy};
use neon::runtime::Trap;
use neon::vfs::MemVfs;
use neon::vm::{self, BufferIo, VmConfig};

const INT_TYPES: [NumTy; 8] =
    [NumTy::I8, NumTy::I16, NumTy::I32, NumTy::I64, NumTy::U8, NumTy::U16, NumTy::U32, NumTy::U64];

fn run_both(src: &str, what: &str) -> (String, Option<Trap>) {
    let c0 = compile_source(
        "main.neon",
        src,
        &MemVfs::new(),
        &Options { opt: OptConfig::none(), compile: CompileOptions { peephole: false } },
    );
    assert!(!c0.diags.has_errors(), "{what}: {}\n{src}", c0.render_diagnostics());
    let oracle = interp::run(c0.ir.as_ref().unwrap(), &[]);
    for (label, opts) in [
        ("O0", Options { opt: OptConfig::none(), compile: CompileOptions { peephole: false } }),
        ("O1+peep", Options::default()),
    ] {
        let c = compile_source("main.neon", src, &MemVfs::new(), &opts);
        assert!(!c.diags.has_errors(), "{what} [{label}]: {}", c.render_diagnostics());
        let mut io = BufferIo::new();
        let report = vm::run(&c.bytecode.unwrap(), &mut io, &VmConfig::default());
        let trap = report.result.err().map(|e| e.trap);
        assert_eq!(io.output, oracle.output, "{what} [{label}] output\n{src}");
        assert_eq!(trap, oracle.trap, "{what} [{label}] trap\n{src}");
    }
    (oracle.output, oracle.trap)
}

/// Values at and near the limits of an integer type, as `(value, source text)`.
fn int_values(ty: NumTy) -> Vec<(i128, String)> {
    let name = ty.name();
    let (min, max) = (ty.min_value(), ty.max_value());
    let lit = |v: i128| -> String {
        if v == min && min != 0 {
            format!("{name}::MIN")
        } else if v == max {
            format!("{name}::MAX")
        } else if v < 0 {
            format!("(-{}{name})", -v)
        } else {
            format!("{v}{name}")
        }
    };
    let mut vals = vec![min, min + 1, -1, 0, 1, 2, 3, 7, max / 2, max - 1, max];
    vals.retain(|&v| v >= min && v <= max);
    vals.dedup();
    let mut seen = std::collections::BTreeSet::new();
    vals.retain(|v| seen.insert(*v));
    vals.into_iter().map(|v| (v, lit(v))).collect()
}

fn bits(ty: NumTy, v: i128) -> u64 {
    encode_int(ty, v).unwrap()
}

#[test]
fn integer_arithmetic_at_the_boundaries() {
    let ops = [
        (ArithOp::Add, "+"),
        (ArithOp::Sub, "-"),
        (ArithOp::Mul, "*"),
        (ArithOp::Div, "/"),
        (ArithOp::Rem, "%"),
        (ArithOp::Shl, "<<"),
        (ArithOp::Shr, ">>"),
        (ArithOp::And, "&"),
        (ArithOp::Or, "|"),
        (ArithOp::Xor, "^"),
    ];
    let mut traps_seen = std::collections::BTreeSet::new();
    for ty in INT_TYPES {
        let vals = int_values(ty);
        let name = ty.name();
        for (op, sym) in ops {
            let header = format!("fn f(a: {name}, b: {name}) -> {name} {{ a {sym} b }}\n");
            let mut safe = header.clone();
            let mut first_trap: Option<String> = None;
            for (a, at) in &vals {
                for (b, bt) in &vals {
                    let line = format!("println(f({at}, {bt}));\n");
                    match arith(op, ty, bits(ty, *a), bits(ty, *b)) {
                        Ok(_) => safe += &line,
                        Err(t) => {
                            traps_seen.insert(format!("{t:?}"));
                            first_trap.get_or_insert(header.clone() + &line);
                        }
                    }
                }
            }
            let (out, trap) = run_both(&safe, &format!("{name} {sym} (safe)"));
            assert!(trap.is_none(), "{name} {sym}: {trap:?}");
            assert!(out.lines().count() > 10);
            if let Some(src) = first_trap {
                let (_, trap) = run_both(&src, &format!("{name} {sym} (trapping)"));
                assert!(trap.is_some(), "{name} {sym} should trap\n{src}");
            }
        }
    }
    for t in ["Overflow", "DivideByZero", "ShiftOverflow"] {
        assert!(traps_seen.contains(t), "no case produced {t}: {traps_seen:?}");
    }
}

#[test]
fn integer_comparisons_negation_and_casts() {
    for ty in INT_TYPES {
        let vals = int_values(ty);
        let name = ty.name();
        let mut src = String::new();
        for (sym, fname) in [("<", "lt"), ("<=", "le"), (">", "gt"), (">=", "ge"), ("==", "eq"), ("!=", "ne")] {
            src += &format!("fn {fname}(a: {name}, b: {name}) -> bool {{ a {sym} b }}\n");
        }
        for (_, at) in &vals {
            for (_, bt) in &vals {
                for fname in ["lt", "le", "gt", "ge", "eq", "ne"] {
                    src += &format!("println({fname}({at}, {bt}));\n");
                }
            }
        }
        let (out, trap) = run_both(&src, &format!("{name} comparisons"));
        assert!(trap.is_none(), "{trap:?}");
        assert!(out.contains("true") && out.contains("false"));
    }
    // negation: every signed type, values where it does not overflow (and MIN, which does)
    for ty in INT_TYPES.iter().copied().filter(|t| t.is_signed()) {
        let name = ty.name();
        let mut src = format!("fn neg(a: {name}) -> {name} {{ -a }}\n");
        for (v, t) in int_values(ty) {
            if v != ty.min_value() {
                src += &format!("println(neg({t}));\n");
            }
        }
        let (_, trap) = run_both(&src, &format!("{name} negation"));
        assert!(trap.is_none());
        let src = format!("fn neg(a: {name}) -> {name} {{ -a }}\nprintln(neg({name}::MIN));\n");
        let (_, trap) = run_both(&src, &format!("{name}::MIN negation"));
        assert_eq!(trap, Some(Trap::Overflow));
    }
    // every numeric cast of every boundary value
    let targets = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64"];
    for ty in INT_TYPES {
        let name = ty.name();
        let mut src = String::new();
        for to in targets {
            src += &format!("fn to_{to}(a: {name}) -> {to} {{ a as {to} }}\n");
        }
        for (_, t) in int_values(ty) {
            for to in targets {
                src += &format!("println(to_{to}({t}));\n");
            }
        }
        let (out, trap) = run_both(&src, &format!("casts from {name}"));
        assert!(trap.is_none(), "{trap:?}");
        assert!(out.lines().count() >= 10 * 5);
    }
}

fn float_values(name: &str) -> Vec<String> {
    let f = |s: &str| format!("{s}{name}");
    vec![
        f("0.0"),
        format!("(-{})", f("0.0")),
        f("1.0"),
        format!("(-{})", f("1.0")),
        f("0.5"),
        f("1.5"),
        f("3.25"),
        f("100000.0"),
        format!("{name}::MAX"),
        format!("{name}::INFINITY"),
        format!("{name}::NEG_INFINITY"),
        format!("{name}::NAN"),
        format!("{name}::EPSILON"),
    ]
}

#[test]
fn float_arithmetic_comparisons_and_casts() {
    for name in ["f32", "f64"] {
        let vals = float_values(name);
        let mut src = String::new();
        for (sym, fname) in [("+", "add"), ("-", "sub"), ("*", "mul"), ("/", "div"), ("%", "rem")] {
            src += &format!("fn {fname}(a: {name}, b: {name}) -> {name} {{ a {sym} b }}\n");
        }
        for (sym, fname) in [("<", "lt"), ("<=", "le"), (">", "gt"), (">=", "ge"), ("==", "eq"), ("!=", "ne")] {
            src += &format!("fn {fname}(a: {name}, b: {name}) -> bool {{ a {sym} b }}\n");
        }
        src += &format!("fn neg(a: {name}) -> {name} {{ -a }}\n");
        for a in &vals {
            src += &format!("println(neg({a}));\n");
            for b in &vals {
                for fname in ["add", "sub", "mul", "div", "rem", "lt", "le", "gt", "ge", "eq", "ne"] {
                    src += &format!("println({fname}({a}, {b}));\n");
                }
            }
        }
        let (out, trap) = run_both(&src, &format!("{name} arithmetic"));
        assert!(trap.is_none(), "{trap:?}");
        assert!(out.contains("NaN") && out.contains("inf"));
        // float -> int casts saturate and map NaN to zero
        let targets = ["i8", "i16", "i32", "i64", "u8", "u16", "u32", "u64", "f32", "f64"];
        let mut src = String::new();
        for to in targets {
            src += &format!("fn to_{to}(a: {name}) -> {to} {{ a as {to} }}\n");
        }
        for a in &vals {
            for to in targets {
                src += &format!("println(to_{to}({a}));\n");
            }
        }
        let (_, trap) = run_both(&src, &format!("casts from {name}"));
        assert!(trap.is_none(), "{trap:?}");
    }
}

#[test]
fn builtin_math_functions() {
    let mut src = String::new();
    for ty in ["i8", "i32", "i64"] {
        src += &format!(
            "fn ab_{ty}(a: {ty}) -> {ty} {{ a.abs() }}\nfn pw_{ty}(a: {ty}, b: u32) -> {ty} {{ a.pow(b) }}\nfn mn_{ty}(a: {ty}, b: {ty}) -> {ty} {{ a.min(b) }}\nfn mx_{ty}(a: {ty}, b: {ty}) -> {ty} {{ a.max(b) }}\n"
        );
    }
    src += "println(ab_i8(-5i8)); println(pw_i32(2i32, 10u32)); println(pw_i64(3i64, 20u32)); println(mn_i32(3i32, -9i32)); println(mx_i8(3i8, 9i8));\n";
    src += "println(2.0.sqrt()); println(0.0.sin()); println(1.0.exp()); println(1.0.ln()); println(7.5.floor()); println(7.5.ceil()); println(7.5.round()); println((-7.5).round()); println(f64::NAN.is_nan());\n";
    let (_, trap) = run_both(&src, "math builtins");
    assert!(trap.is_none(), "{trap:?}");
    for (what, src) in [
        ("abs overflow", "println((i8::MIN).abs());"),
        ("pow overflow", "println(10i32.pow(10u32));"),
        ("pow zero exponent", "println(7i64.pow(0u32));"),
        ("ln of negative", "println((-1.0).ln());"),
    ] {
        run_both(src, what);
    }
}

/// `checked_*` must return `Some(r)` exactly when the plain operator would
/// produce `r`, and `None` exactly when it would trap.
#[test]
fn checked_operations_agree_with_the_trapping_operators() {
    use neon::runtime::format::format_num;
    let ops = [
        (ArithOp::Add, "checked_add"),
        (ArithOp::Sub, "checked_sub"),
        (ArithOp::Mul, "checked_mul"),
        (ArithOp::Div, "checked_div"),
        (ArithOp::Rem, "checked_rem"),
        (ArithOp::Shl, "checked_shl"),
        (ArithOp::Shr, "checked_shr"),
    ];
    for ty in INT_TYPES {
        let vals = int_values(ty);
        for (op, method) in ops {
            let mut src = String::new();
            let mut expected = String::new();
            for (a, at) in &vals {
                for (b, bt) in &vals {
                    src += &format!("println({at}.{method}({bt}));\n");
                    match arith(op, ty, bits(ty, *a), bits(ty, *b)) {
                        Ok(r) => expected += &format!("Option::Some({})\n", format_num(ty, r)),
                        Err(_) => expected += "Option::None\n",
                    }
                }
            }
            let (out, trap) = run_both(&src, &format!("{} {method}", ty.name()));
            assert!(trap.is_none(), "{trap:?}");
            assert_eq!(out, expected, "{} {method}", ty.name());
        }
        if ty.is_signed() {
            let mut src = String::new();
            let mut expected = String::new();
            for (a, at) in &vals {
                src += &format!("println({at}.checked_neg());\n");
                match neon::runtime::numeric::neg(ty, bits(ty, *a)) {
                    Ok(r) => expected += &format!("Option::Some({})\n", format_num(ty, r)),
                    Err(_) => expected += "Option::None\n",
                }
            }
            let (out, _) = run_both(&src, "checked_neg");
            assert_eq!(out, expected, "{} checked_neg", ty.name());
        }
        // pow with a few exponents
        let mut src = String::new();
        let mut expected = String::new();
        for (a, at) in &vals {
            for e in [0u32, 1, 2, 7, 40, 70] {
                src += &format!("println({at}.checked_pow({e}u32));\n");
                match neon::runtime::mathfn::pow(ty, bits(ty, *a), e as u64) {
                    Ok(r) => expected += &format!("Option::Some({})\n", format_num(ty, r)),
                    Err(_) => expected += "Option::None\n",
                }
            }
        }
        let (out, _) = run_both(&src, "checked_pow");
        assert_eq!(out, expected, "{} checked_pow", ty.name());
    }
}
