//! Translation of IR functions to C.
//!
//! Registers become typed C locals (`int64_t v3`, `Obj *v7`); containers store 64-bit words, so a
//! value is converted with `to_w` / `from_w` at the boundary. Control flow is `goto`, with the
//! blocks in the compiler's layout order so most jumps are fall-throughs. References live in the
//! locals the collector learns about through a per-call `Frame`; a variable captured by a closure
//! lives in a heap `Cell` shared with the closure.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

use crate::builtin::Builtin;
use crate::compiler::block_layout;
use crate::effects::Effects;
use crate::ir::effects::{analyze, FuncEffects};
use crate::ir::*;
use crate::runtime::{ArithOp, CmpOp, NumTy};

/// A runtime function generated for one type (checked arithmetic, conversions).
struct SysFn {
    src: String,
    /// Present in release builds too (otherwise it only exists under `NEON_CHECKS`).
    release: bool,
}

pub struct Gen<'p> {
    pub prog: &'p IrProgram,
    fx: FuncEffects,
    /// Runtime pieces the program asks for directly.
    needs: BTreeSet<String>,
    /// Pieces only the checks of a debug build use (overflow traps, the recursion guard).
    checked: BTreeSet<String>,
    sys: BTreeMap<String, SysFn>,
    live: Vec<bool>,
    names: Vec<String>,
    guarded: Vec<bool>,
    strs: Vec<u32>,
    str_idx: HashMap<u32, usize>,
    globals: BTreeSet<usize>,
    gnames: Vec<String>,
    printers: BTreeMap<usize, (String, String)>,
    printer_todo: Vec<usize>,
    fnptrs: Vec<(String, String)>,
    /// Plain functions used as closure values: they get a wrapper taking the (unused) cell array.
    wrappers: BTreeSet<usize>,
}

/// The program's code, the runtime pieces it asks for, and the generated runtime functions.
pub type Generated = (Output, Needs, BTreeMap<String, (String, bool)>);

/// Runtime pieces the program asks for: those every build needs, and those only a checked (debug)
/// build needs.
#[derive(Debug, Default)]
pub struct Needs {
    pub always: BTreeSet<String>,
    pub checked: BTreeSet<String>,
}

/// The pieces of the generated program, before they are put in order.
pub struct Output {
    pub fn_protos: Vec<String>,
    pub fn_bodies: Vec<String>,
    pub typedefs: Vec<String>,
    pub globals: Vec<String>,
    pub strs: Vec<u32>,
    pub ref_globals: Vec<String>,
    pub entry: String,
}

fn sanitize(s: &str) -> String {
    let mut out = String::new();
    let mut last_us = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_us = false;
        } else if !last_us {
            out.push('_');
            last_us = true;
        }
    }
    while out.ends_with('_') {
        out.pop();
    }
    out
}

/// A C string literal (with explicit escapes, so any bytes are fine).
pub fn c_string(s: &str) -> String {
    let mut out = String::from("\"");
    for &b in s.as_bytes() {
        match b {
            b'"' => out.push_str("\\\""),
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\t' => out.push_str("\\t"),
            b'\r' => out.push_str("\\r"),
            b'?' => out.push_str("\\?"),
            0x20..=0x7e => out.push(b as char),
            _ => {
                let _ = write!(out, "\\{b:03o}");
            }
        }
    }
    out.push('"');
    out
}

fn num_cty(n: NumTy) -> &'static str {
    match n {
        NumTy::I8 => "int8_t",
        NumTy::I16 => "int16_t",
        NumTy::I32 => "int32_t",
        NumTy::I64 => "int64_t",
        NumTy::U8 => "uint8_t",
        NumTy::U16 => "uint16_t",
        NumTy::U32 => "uint32_t",
        NumTy::U64 => "uint64_t",
        NumTy::F32 => "float",
        NumTy::F64 => "double",
    }
}

fn cty(t: IrTy) -> &'static str {
    match t {
        IrTy::Num(n) => num_cty(n),
        IrTy::Bool => "uint8_t",
        IrTy::Char => "uint32_t",
        IrTy::Ref => "Obj *",
        IrTy::Unit => "unit_t",
    }
}

fn unsigned_cty(n: NumTy) -> &'static str {
    match n.bits() {
        8 => "uint8_t",
        16 => "uint16_t",
        32 => "uint32_t",
        _ => "uint64_t",
    }
}

fn limits(n: NumTy) -> (String, String) {
    let b = n.bits();
    if n.is_signed() {
        (format!("INT{b}_MIN"), format!("INT{b}_MAX"))
    } else {
        ("0".into(), format!("UINT{b}_MAX"))
    }
}

fn paren(e: &str) -> String {
    if e.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        e.to_string()
    } else {
        format!("({e})")
    }
}

fn float_lit(v: f64, single: bool) -> String {
    if v.is_nan() {
        return "NAN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "INFINITY".into() } else { "(-INFINITY)".into() };
    }
    let text = if single { format!("{:?}", v as f32) } else { format!("{v:?}") };
    let text = if single { format!("{text}f") } else { text };
    if text.starts_with('-') {
        format!("({text})")
    } else {
        text
    }
}

fn lit(t: IrTy, bits: u64) -> String {
    match t {
        IrTy::Num(NumTy::F64) => float_lit(f64::from_bits(bits), false),
        IrTy::Num(NumTy::F32) => float_lit(f32::from_bits(bits as u32) as f64, true),
        IrTy::Num(NumTy::I8 | NumTy::I16 | NumTy::I32) => {
            let v = bits as i64;
            if v < 0 {
                format!("({v})")
            } else {
                v.to_string()
            }
        }
        IrTy::Num(NumTy::I64) => {
            let v = bits as i64;
            if v == i64::MIN {
                "INT64_MIN".into()
            } else if v >= 0 && v <= i32::MAX as i64 {
                v.to_string()
            } else if v < 0 && v > i32::MIN as i64 {
                format!("({v})")
            } else {
                format!("INT64_C({v})")
            }
        }
        IrTy::Num(NumTy::U8 | NumTy::U16) => bits.to_string(),
        IrTy::Num(NumTy::U32) => format!("{bits}u"),
        IrTy::Num(NumTy::U64) => {
            if bits <= u32::MAX as u64 {
                format!("{bits}u")
            } else {
                format!("UINT64_C({bits})")
            }
        }
        IrTy::Bool => ((bits != 0) as u8).to_string(),
        IrTy::Char => format!("{}u", bits as u32),
        IrTy::Ref | IrTy::Unit => "0".into(),
    }
}

fn cmp_op(op: CmpOp) -> &'static str {
    match op {
        CmpOp::Eq => "==",
        CmpOp::Ne => "!=",
        CmpOp::Lt => "<",
        CmpOp::Le => "<=",
        CmpOp::Gt => ">",
        CmpOp::Ge => ">=",
    }
}

impl<'p> Gen<'p> {
    pub fn new(prog: &'p IrProgram) -> Gen<'p> {
        Gen {
            prog,
            fx: analyze(prog),
            needs: BTreeSet::new(),
            checked: BTreeSet::new(),
            sys: BTreeMap::new(),
            live: vec![false; prog.funcs.len()],
            names: vec![String::new(); prog.funcs.len()],
            guarded: vec![false; prog.funcs.len()],
            strs: Vec::new(),
            str_idx: HashMap::new(),
            globals: BTreeSet::new(),
            gnames: Vec::new(),
            printers: BTreeMap::new(),
            printer_todo: Vec::new(),
            fnptrs: Vec::new(),
            wrappers: BTreeSet::new(),
        }
    }

    pub fn need(&mut self, name: &str) {
        self.needs.insert(name.to_string());
    }

    fn needs_all(&mut self, names: &[&str]) {
        for n in names {
            self.need(n);
        }
    }

    /// A piece that only a build with `NEON_CHECKS` uses.
    fn need_checked(&mut self, names: &[&str]) {
        self.need("checks");
        for n in names {
            self.checked.insert(n.to_string());
        }
    }

    /// A generated runtime function. `deps` are what every build needs, `checked_deps` what only a
    /// checked build does; `release` says whether release builds contain the function at all.
    fn sysfn(&mut self, name: &str, deps: &[&str], checked_deps: &[&str], release: bool, src: String) {
        self.needs_all(deps);
        if checked_deps.is_empty() {
            self.need("checks");
        } else {
            self.need_checked(checked_deps);
        }
        let e = self.sys.entry(name.to_string()).or_insert(SysFn { src, release });
        e.release |= release;
    }

    // ---- program-wide analyses -----------------------------------------------------------------

    fn find_live(&mut self) {
        let mut work = vec![self.prog.entry];
        self.live[self.prog.entry.index()] = true;
        while let Some(f) = work.pop() {
            for b in &self.prog.func(f).blocks {
                for inst in &b.insts {
                    let callee = match inst {
                        Inst::Call { func, .. } | Inst::MakeClosure { func, .. } => Some(*func),
                        _ => None,
                    };
                    if let Some(c) = callee {
                        if !self.live[c.index()] {
                            self.live[c.index()] = true;
                            work.push(c);
                        }
                    }
                }
            }
        }
    }

    fn assign_names(&mut self) {
        let mut taken: BTreeSet<String> = BTreeSet::new();
        for (i, f) in self.prog.funcs.iter().enumerate() {
            if !self.live[i] {
                continue;
            }
            let base = {
                let s = sanitize(&f.name);
                if s.is_empty() {
                    "fn".to_string()
                } else {
                    s
                }
            };
            let mut name = format!("n_{base}");
            if !taken.insert(name.clone()) {
                name = format!("n_{base}_{i}");
                taken.insert(name.clone());
            }
            self.names[i] = name;
        }
        let mut gtaken: BTreeSet<String> = BTreeSet::new();
        for (i, g) in self.prog.globals.iter().enumerate() {
            let base = sanitize(&g.name);
            let mut name = format!("g_{}", if base.is_empty() { "global".into() } else { base });
            if !gtaken.insert(name.clone()) {
                name = format!("{name}_{i}");
                gtaken.insert(name.clone());
            }
            self.gnames.push(name);
        }
    }

    /// Functions that can end up calling themselves (or call closures) get a recursion guard.
    fn find_guarded(&mut self) {
        let n = self.prog.funcs.len();
        let mut edges: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, f) in self.prog.funcs.iter().enumerate() {
            if !self.live[i] {
                continue;
            }
            if f.is_closure {
                self.guarded[i] = true;
            }
            for b in &f.blocks {
                for inst in &b.insts {
                    match inst {
                        Inst::Call { func, .. } => edges[i].push(func.index()),
                        Inst::CallClosure { .. } => self.guarded[i] = true,
                        _ => {}
                    }
                }
            }
        }
        for i in 0..n {
            if !self.live[i] || self.guarded[i] {
                continue;
            }
            let mut seen = vec![false; n];
            let mut work = edges[i].clone();
            while let Some(j) = work.pop() {
                if j == i {
                    self.guarded[i] = true;
                    break;
                }
                if !seen[j] {
                    seen[j] = true;
                    work.extend(edges[j].iter().copied());
                }
            }
        }
    }

    // ---- conversions ---------------------------------------------------------------------------

    fn as_word(&mut self, t: IrTy, e: &str) -> String {
        match t {
            IrTy::Num(n) if n.is_signed() => format!("(W)(int64_t){}", paren(e)),
            IrTy::Num(NumTy::F64) => {
                self.need("f64_bits");
                format!("f64_bits({e})")
            }
            IrTy::Num(NumTy::F32) => {
                self.need("f32_bits");
                format!("f32_bits({e})")
            }
            IrTy::Num(_) | IrTy::Bool | IrTy::Char => format!("(W){}", paren(e)),
            IrTy::Ref => {
                self.need("objects");
                format!("O2W({e})")
            }
            IrTy::Unit => "0".into(),
        }
    }

    fn of_word(&mut self, t: IrTy, e: &str) -> String {
        match t {
            IrTy::Num(n) if n.is_signed() => format!("({})(int64_t){}", num_cty(n), paren(e)),
            IrTy::Num(NumTy::F64) => {
                self.need("bits_f64");
                format!("bits_f64({e})")
            }
            IrTy::Num(NumTy::F32) => {
                self.need("bits_f32");
                format!("bits_f32({e})")
            }
            IrTy::Num(n) => format!("({}){}", num_cty(n), paren(e)),
            IrTy::Bool => format!("(uint8_t){}", paren(e)),
            IrTy::Char => format!("(uint32_t){}", paren(e)),
            IrTy::Ref => {
                self.need("objects");
                format!("W2O({e})")
            }
            IrTy::Unit => "0".into(),
        }
    }

    // ---- generated runtime functions --------------------------------------------------------------

    /// The arithmetic function for `op` on `ty`. A checked (debug) build traps on overflow, division by
    /// zero and bad shifts; a release build wraps around and leaves the rest to C.
    fn arith_fn(&mut self, op: ArithOp, ty: NumTy) -> String {
        let c = num_cty(ty);
        let n = ty.name();
        let (min, _) = limits(ty);
        let name = format!("{}_{n}", op.name());
        let signed = ty.is_signed();
        let bits = ty.bits();
        let wide = if bits <= 32 { "uint32_t" } else { "uint64_t" };
        let (src, deps): (String, &[&str]) = match op {
            ArithOp::Add | ArithOp::Sub | ArithOp::Mul => {
                let m = op.name().to_uppercase();
                let sym = match op {
                    ArithOp::Add => '+',
                    ArithOp::Sub => '-',
                    _ => '*',
                };
                (
                    format!(
                        "static {c} {name}({c} a, {c} b) {{\n#if NEON_CHECKS\n    {c} r;\n    if (UNLIKELY(OVF_{m}(a, b, &r))) trap_overflow();\n    return r;\n#else\n    return ({c})(({wide})a {sym} ({wide})b);\n#endif\n}}"
                    ),
                    &["ovf", "trap_overflow", "hint_expect"],
                )
            }
            ArithOp::Div => {
                let ovf = if signed {
                    format!("    if (UNLIKELY(a == {min} && b == -1)) trap_overflow();\n")
                } else {
                    String::new()
                };
                (
                    format!(
                        "static {c} {name}({c} a, {c} b) {{\n#if NEON_CHECKS\n    if (UNLIKELY(b == 0)) trap_div0();\n{ovf}#endif\n    return ({c})(a / b);\n}}"
                    ),
                    &["trap_div0", "trap_overflow", "hint_expect"],
                )
            }
            ArithOp::Rem => {
                let minus =
                    if signed { "    if (b == -1) return 0;  /* MIN % -1 is 0, not an overflow */\n" } else { "" };
                (
                    format!(
                        "static {c} {name}({c} a, {c} b) {{\n#if NEON_CHECKS\n    if (UNLIKELY(b == 0)) trap_div0();\n{minus}#endif\n    return ({c})(a % b);\n}}"
                    ),
                    &["trap_div0", "hint_expect"],
                )
            }
            ArithOp::Shl | ArithOp::Shr => {
                let u = unsigned_cty(ty);
                let body = if op == ArithOp::Shl {
                    format!("({c})(({u})a << (n & {}))", bits - 1)
                } else {
                    format!("({c})(a >> (n & {}))", bits - 1)
                };
                (
                    format!(
                        "static {c} {name}({c} a, int64_t n) {{\n#if NEON_CHECKS\n    if (UNLIKELY((uint64_t)n >= {bits})) trap_shift();\n#endif\n    return {body};\n}}"
                    ),
                    &["trap_shift", "hint_expect"],
                )
            }
            ArithOp::And | ArithOp::Or | ArithOp::Xor => unreachable!("bitwise operators are inline"),
        };
        self.sysfn(&name, &[], deps, true, src);
        name
    }

    fn neg_fn(&mut self, ty: NumTy) -> String {
        let (c, n) = (num_cty(ty), ty.name());
        let name = format!("neg_{n}");
        let wide = if ty.bits() <= 32 { "uint32_t" } else { "uint64_t" };
        let check = if ty.is_signed() {
            let (min, _) = limits(ty);
            format!("    if (UNLIKELY(a == {min})) trap_overflow();\n")
        } else {
            "    if (UNLIKELY(a != 0)) trap_overflow();\n".to_string()
        };
        let src = format!(
            "static {c} {name}({c} a) {{\n#if NEON_CHECKS\n{check}#endif\n    return ({c})(0 - ({wide})a);\n}}"
        );
        self.sysfn(&name, &[], &["trap_overflow", "hint_expect"], true, src);
        name
    }

    fn abs_fn(&mut self, ty: NumTy) -> String {
        let (c, n) = (num_cty(ty), ty.name());
        let name = format!("abs_{n}");
        let (min, _) = limits(ty);
        let wide = if ty.bits() <= 32 { "uint32_t" } else { "uint64_t" };
        let src = format!(
            "static {c} {name}({c} a) {{\n#if NEON_CHECKS\n    if (UNLIKELY(a == {min})) trap_overflow();\n#endif\n    return a < 0 ? ({c})(0 - ({wide})a) : a;\n}}"
        );
        self.sysfn(&name, &[], &["trap_overflow", "hint_expect"], true, src);
        name
    }

    /// `chk_<op>_<ty>(a, b, &r)`: stores the result and returns 1, or returns 0 where the plain
    /// operation would trap. `release`: used by `checked_*` calls, which every build needs.
    fn chk_fn(&mut self, b: Builtin, ty: NumTy, release: bool) -> String {
        use Builtin::*;
        let (c, n) = (num_cty(ty), ty.name());
        let signed = ty.is_signed();
        let bits = ty.bits();
        let (min, _) = limits(ty);
        let (op, params, body) = match b {
            CheckedAdd => ("add", format!("{c} a, {c} b"), "    return !OVF_ADD(a, b, r);".to_string()),
            CheckedSub => ("sub", format!("{c} a, {c} b"), "    return !OVF_SUB(a, b, r);".to_string()),
            CheckedMul => ("mul", format!("{c} a, {c} b"), "    return !OVF_MUL(a, b, r);".to_string()),
            CheckedDiv => {
                let ovf = if signed { format!("    if (a == {min} && b == -1) return 0;\n") } else { String::new() };
                ("div", format!("{c} a, {c} b"), format!("    if (b == 0) return 0;\n{ovf}    *r = ({c})(a / b);\n    return 1;"))
            }
            CheckedRem => {
                let minus = if signed { "    if (b == -1) { *r = 0; return 1; }\n" } else { "" };
                ("rem", format!("{c} a, {c} b"), format!("    if (b == 0) return 0;\n{minus}    *r = ({c})(a % b);\n    return 1;"))
            }
            CheckedShl => {
                let u = unsigned_cty(ty);
                ("shl", format!("{c} a, int64_t n"), format!("    if ((uint64_t)n >= {bits}) return 0;\n    *r = ({c})(({u})a << n);\n    return 1;"))
            }
            CheckedShr => (
                "shr",
                format!("{c} a, int64_t n"),
                format!("    if ((uint64_t)n >= {bits}) return 0;\n    *r = ({c})(a >> n);\n    return 1;"),
            ),
            CheckedNeg => {
                let body = if signed {
                    format!("    if (a == {min}) return 0;\n    *r = ({c})-a;\n    return 1;")
                } else {
                    "    if (a != 0) return 0;\n    *r = 0;\n    return 1;".to_string()
                };
                ("neg", format!("{c} a"), body)
            }
            CheckedPow => (
                "pow",
                format!("{c} base, uint64_t exp"),
                format!(
                    "    {c} result = 1, square = base;\n    while (exp > 0) {{\n        if (exp & 1) {{\n            if (OVF_MUL(result, square, &result)) return 0;\n        }}\n        exp >>= 1;\n        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;\n    }}\n    *r = result;\n    return 1;"
                ),
            ),
            _ => unreachable!("not a checked operation"),
        };
        let name = format!("chk_{op}_{n}");
        let src = format!("static int {name}({params}, {c} *r) {{\n{body}\n}}");
        if release {
            self.sysfn(&name, &["ovf"], &[], true, src);
        } else {
            self.sysfn(&name, &[], &["ovf"], false, src);
        }
        name
    }

    /// `pow_<ty>(base, exp)` for integers: a checked build traps on overflow, a release build wraps.
    fn pow_fn(&mut self, ty: NumTy) -> String {
        let chk = self.chk_fn(Builtin::CheckedPow, ty, false);
        let (c, n) = (num_cty(ty), ty.name());
        let name = format!("pow_{n}");
        let wide = if ty.bits() <= 32 { "uint32_t" } else { "uint64_t" };
        let src = format!(
            "static {c} {name}({c} base, uint64_t exp) {{\n#if NEON_CHECKS\n    {c} r;\n    if (UNLIKELY(!{chk}(base, exp, &r))) trap_overflow();\n    return r;\n#else\n    {c} result = 1, square = base;\n    while (exp > 0) {{\n        if (exp & 1) result = ({c})(({wide})result * ({wide})square);\n        exp >>= 1;\n        square = ({c})(({wide})square * ({wide})square);\n    }}\n    return result;\n#endif\n}}"
        );
        self.sysfn(&name, &[], &["trap_overflow", "hint_expect"], true, src);
        name
    }

    /// Float to integer conversion: saturating, NaN becomes 0.
    fn f2i_fn(&mut self, to: NumTy) -> String {
        let name = format!("f2i_{}", to.name());
        let src = match to {
            NumTy::I64 => "static int64_t f2i_i64(double v) {\n    if (v != v) return 0;\n    if (v >= 9223372036854775808.0) return INT64_MAX;\n    if (v <= -9223372036854775808.0) return INT64_MIN;\n    return (int64_t)v;\n}".to_string(),
            NumTy::U64 => "static uint64_t f2i_u64(double v) {\n    if (!(v > 0)) return 0;\n    if (v >= 18446744073709551616.0) return UINT64_MAX;\n    return (uint64_t)v;\n}".to_string(),
            _ => {
                self.f2i_fn(NumTy::I64);
                let (min, max) = limits(to);
                let c = num_cty(to);
                format!(
                    "static {c} {name}(double v) {{\n    int64_t w = f2i_i64(v);\n    return w < {min} ? ({c}){min} : w > (int64_t){max} ? ({c}){max} : ({c})w;\n}}"
                )
            }
        };
        self.sysfn(&name, &[], &[], true, src);
        name
    }

    // ---- function pointer types ------------------------------------------------------------------

    fn fnptr(&mut self, ret: IrTy, args: &[IrTy]) -> String {
        if ret == IrTy::Unit || args.contains(&IrTy::Unit) {
            self.need("unit");
        }
        let mut sig = format!("{} (*)(Obj **", cty(ret));
        for a in args {
            sig.push_str(", ");
            sig.push_str(cty(*a));
        }
        sig.push(')');
        if let Some(i) = self.fnptrs.iter().position(|(s, _)| *s == sig) {
            return format!("fp{i}");
        }
        let i = self.fnptrs.len();
        let ptr = sig.replace("(*)", &format!("(*fp{i})"));
        self.fnptrs.push((sig, format!("typedef {ptr};")));
        self.need("objects");
        format!("fp{i}")
    }

    // ---- constants, printers -----------------------------------------------------------------------

    fn str_index(&mut self, i: u32) -> usize {
        if let Some(&k) = self.str_idx.get(&i) {
            return k;
        }
        let k = self.strs.len();
        self.strs.push(i);
        self.str_idx.insert(i, k);
        self.needs_all(&["str_new", "hint_expect"]);
        k
    }

    fn printer(&mut self, d: DescId) -> String {
        if let std::collections::btree_map::Entry::Vacant(e) = self.printers.entry(d.index()) {
            e.insert((String::new(), String::new()));
            self.printer_todo.push(d.index());
        }
        format!("pr_{}", d.index())
    }

    /// A statement that appends the text of the value `w` (a word) of type `d` to the scratch buffer.
    fn put_word(&mut self, d: DescId, w: &str, nested: bool) -> String {
        self.need("sbtype");
        let prog = self.prog;
        match prog.desc(d) {
            TypeDesc::Unit => {
                self.need("sb_puts");
                "sb_puts(&rt_sb, \"()\");".into()
            }
            TypeDesc::Num(NumTy::F64) => {
                self.needs_all(&["sb_float", "bits_f64"]);
                format!("sb_float(&rt_sb, bits_f64({w}), 0);")
            }
            TypeDesc::Num(NumTy::F32) => {
                self.needs_all(&["sb_float", "bits_f32"]);
                format!("sb_float(&rt_sb, (double)bits_f32({w}), 1);")
            }
            TypeDesc::Num(n) if n.is_signed() => {
                self.need("sb_i64");
                format!("sb_i64(&rt_sb, (int64_t){w});")
            }
            TypeDesc::Num(_) => {
                self.need("sb_u64");
                format!("sb_u64(&rt_sb, {w});")
            }
            TypeDesc::Bool => {
                self.need("sb_puts");
                format!("sb_puts(&rt_sb, {w} ? \"true\" : \"false\");")
            }
            TypeDesc::Char if nested => {
                self.need("sb_dbg_char");
                format!("sb_dbg_char(&rt_sb, (uint32_t){w});")
            }
            TypeDesc::Char => {
                self.need("sb_cp");
                format!("sb_cp(&rt_sb, (uint32_t){w});")
            }
            TypeDesc::Str if nested => {
                self.needs_all(&["sb_dbg_str", "objects"]);
                format!("sb_dbg_str(&rt_sb, W2O({w}));")
            }
            TypeDesc::Str => {
                self.needs_all(&["sb_put", "objects"]);
                format!("sb_put(&rt_sb, STR(W2O({w}))->s, STR(W2O({w}))->len);")
            }
            _ => {
                self.need("objects");
                format!("{}(W2O({w}));", self.printer(d))
            }
        }
    }

    fn put_str(&mut self, text: &str) -> String {
        self.need("sb_puts");
        format!("sb_puts(&rt_sb, {});", c_string(text))
    }

    fn gen_printer(&mut self, d: usize) {
        let prog = self.prog;
        let name = format!("pr_{d}");
        let mut body = String::new();
        match prog.desc(DescId(d as u32)) {
            TypeDesc::Array(e) | TypeDesc::Set(e) => {
                let set = matches!(prog.desc(DescId(d as u32)), TypeDesc::Set(_));
                let open = if set { "set{" } else { "[" };
                let close = if set { "}" } else { "]" };
                let put = self.put_word(*e, if set { "m->k[i]" } else { "a->a[i]" }, true);
                let sep = self.put_str(", ");
                let (o, c) = (self.put_str(open), self.put_str(close));
                if set {
                    let _ = write!(
                        body,
                        "    const Map *m = MAP(o);\n    size_t i;\n    {o}\n    for (i = 0; i < m->len; i++) {{\n        if (i) {sep}\n        {put}\n    }}\n    {c}\n"
                    );
                } else {
                    let _ = write!(
                        body,
                        "    const Arr *a = ARR(o);\n    size_t i;\n    {o}\n    for (i = 0; i < a->len; i++) {{\n        if (i) {sep}\n        {put}\n    }}\n    {c}\n"
                    );
                }
            }
            TypeDesc::Map(k, v) => {
                let (pk, pv) = (self.put_word(*k, "m->k[i]", true), self.put_word(*v, "m->v[i]", true));
                let (sep, colon) = (self.put_str(", "), self.put_str(": "));
                let (o, c) = (self.put_str("{"), self.put_str("}"));
                let _ = write!(
                    body,
                    "    const Map *m = MAP(o);\n    size_t i;\n    {o}\n    for (i = 0; i < m->len; i++) {{\n        if (i) {sep}\n        {pk}\n        {colon}\n        {pv}\n    }}\n    {c}\n"
                );
            }
            TypeDesc::Struct { name: sname, fields } => {
                if fields.is_empty() {
                    let _ = writeln!(body, "    (void)o;\n    {}", self.put_str(sname));
                } else {
                    let _ = writeln!(body, "    const Agg *g = AGG(o);");
                    let _ = writeln!(body, "    {}", self.put_str(&format!("{sname} {{ ")));
                    for (i, (fname, fd)) in fields.iter().enumerate() {
                        let lead = if i == 0 { format!("{fname}: ") } else { format!(", {fname}: ") };
                        let _ = writeln!(body, "    {}", self.put_str(&lead));
                        let w = format!("g->f[{i}]");
                        let _ = writeln!(body, "    {}", self.put_word(*fd, &w, true));
                    }
                    let _ = writeln!(body, "    {}", self.put_str(" }"));
                }
            }
            TypeDesc::Enum { name: ename, variants } => {
                let _ = writeln!(body, "    const Agg *g = AGG(o);\n    switch (g->h.flags) {{");
                for (t, (vname, fds)) in variants.iter().enumerate() {
                    let _ = writeln!(body, "    case {t}:");
                    let _ = writeln!(body, "        {}", self.put_str(&format!("{ename}::{vname}")));
                    if !fds.is_empty() {
                        let _ = writeln!(body, "        {}", self.put_str("("));
                        for (i, fd) in fds.iter().enumerate() {
                            if i > 0 {
                                let _ = writeln!(body, "        {}", self.put_str(", "));
                            }
                            let w = format!("g->f[{i}]");
                            let _ = writeln!(body, "        {}", self.put_word(*fd, &w, true));
                        }
                        let _ = writeln!(body, "        {}", self.put_str(")"));
                    }
                    let _ = writeln!(body, "        break;");
                }
                let _ = writeln!(body, "    default:\n        break;\n    }}");
            }
            TypeDesc::Fn => {
                let _ = writeln!(body, "    (void)o;\n    {}", self.put_str("<fn>"));
            }
            other => unreachable!("no printer function for {other:?}"),
        }
        let header = format!("static void {name}(Obj *o)");
        self.printers.insert(d, (format!("{header};"), format!("{header} {{\n{body}}}")));
    }

    // ---- driver -------------------------------------------------------------------------------------

    pub fn generate(mut self) -> Result<Generated, String> {
        self.find_live();
        self.assign_names();
        self.find_guarded();
        let mut fn_protos = Vec::new();
        let mut fn_bodies = Vec::new();
        for i in 0..self.prog.funcs.len() {
            if !self.live[i] {
                continue;
            }
            let (proto, body) = self.emit_func(FuncId(i as u32))?;
            fn_protos.push(proto);
            fn_bodies.push(body);
        }
        while let Some(d) = self.printer_todo.pop() {
            self.gen_printer(d);
        }
        for (proto, body) in self.printers.values() {
            fn_protos.push(proto.clone());
            fn_bodies.insert(0, body.clone());
        }
        for w in std::mem::take(&mut self.wrappers) {
            let (proto, body) = self.gen_wrapper(w);
            fn_protos.push(proto);
            fn_bodies.push(body);
        }
        let mut globals = Vec::new();
        let mut ref_globals = Vec::new();
        let used_globals: Vec<usize> = self.globals.iter().copied().collect();
        for g in used_globals {
            let info = &self.prog.globals[g];
            let t = cty(info.ty);
            globals.push(format!("static {t}{}{};", if t.ends_with('*') { "" } else { " " }, self.gnames[g]));
            if info.ty == IrTy::Unit {
                self.need("unit");
            }
            if info.ty == IrTy::Ref {
                self.need("objects");
            }
            if info.ty == IrTy::Ref {
                ref_globals.push(self.gnames[g].clone());
            }
        }
        let typedefs = self.fnptrs.iter().map(|(_, t)| t.clone()).collect();
        let sys = self.sys.iter().map(|(n, s)| (n.clone(), (s.src.clone(), s.release))).collect();
        let out = Output {
            fn_protos,
            fn_bodies,
            typedefs,
            globals,
            strs: self.strs.clone(),
            ref_globals,
            entry: self.names[self.prog.entry.index()].clone(),
        };
        Ok((out, Needs { always: self.needs, checked: self.checked }, sys))
    }

    /// `n_f_clo(up, args)` calls the plain function `n_f(args)`, so it can sit in a closure object.
    fn gen_wrapper(&mut self, id: usize) -> (String, String) {
        let f = &self.prog.funcs[id];
        let name = &self.names[id];
        let mut params = vec!["Obj **up".to_string()];
        let mut args = Vec::new();
        for i in 0..f.params as usize {
            let t = cty(f.regs[i]);
            params.push(format!("{t}{}a{i}", if t.ends_with('*') { "" } else { " " }));
            args.push(format!("a{i}"));
        }
        let ret = cty(f.ret);
        let sep = if ret.ends_with('*') { "" } else { " " };
        let header = format!("static {ret}{sep}{name}_clo({})", params.join(", "));
        let body = format!("{header} {{\n    (void)up;\n    return {name}({});\n}}", args.join(", "));
        (format!("{header};"), body)
    }

    // ---- one function ---------------------------------------------------------------------------------

    fn emit_func(&mut self, id: FuncId) -> Result<(String, String), String> {
        let prog = self.prog;
        let f = prog.func(id);
        let name = self.names[id.index()].clone();
        let order = block_layout(f);
        let nregs = f.regs.len();
        let mut used = vec![false; nregs];
        let mut defined = vec![false; nregs];
        let mut uses_up = false;
        let mut may_gc = false;
        for &b in &order {
            let blk = &f.blocks[b.index()];
            for inst in &blk.insts {
                inst.for_each_use(|r| used[r.index()] = true);
                if let Some(d) = inst.def() {
                    defined[d.index()] = true;
                }
                match inst {
                    Inst::GetUpval { .. } | Inst::SetUpval { .. } => uses_up = true,
                    Inst::MakeClosure { captures, .. } => {
                        uses_up |= captures.iter().any(|c| matches!(c, Capture::Upvalue(_)));
                        may_gc = true;
                    }
                    Inst::CallClosure { .. } | Inst::CloseUpvals { .. } => may_gc = true,
                    Inst::Call { func, .. } => may_gc |= self.fx.of(*func).allocates(),
                    _ => may_gc |= allocates(inst),
                }
            }
            blk.term.for_each_use(|r| used[r.index()] = true);
        }
        let params = f.params as usize;
        let pinned: Vec<bool> = (0..nregs).map(|i| f.pinned.get(i).copied().unwrap_or(false)).collect();

        let mut defs = vec![0u32; nregs];
        let mut const_bits: Vec<Option<u64>> = vec![None; nregs];
        for d in defs.iter_mut().take(params) {
            *d = 1;
        }
        for &b in &order {
            for inst in &f.blocks[b.index()].insts {
                if let Some(d) = inst.def() {
                    defs[d.index()] += 1;
                    if let Inst::Const { val: Const::Bits(bits), .. } = inst {
                        const_bits[d.index()] = Some(*bits);
                    }
                }
            }
        }
        let consts: Vec<Option<u64>> = (0..nregs)
            .map(|i| {
                let numeric = matches!(f.regs[i], IrTy::Num(_) | IrTy::Bool | IrTy::Char);
                (defs[i] == 1 && i >= params && !pinned[i] && numeric).then(|| const_bits[i]).flatten()
            })
            .collect();

        // signature
        let mut sig_params = Vec::new();
        if f.is_closure {
            sig_params.push("Obj **up".to_string());
        }
        for (i, &is_pinned) in pinned.iter().enumerate().take(params) {
            let pn = if is_pinned { format!("p{i}") } else { format!("v{i}") };
            sig_params.push(format!("{}{}{pn}", cty(f.regs[i]), if f.regs[i] == IrTy::Ref { "" } else { " " }));
        }
        if sig_params.is_empty() {
            sig_params.push("void".into());
        }
        let eff = self.fx.of(id);
        let attr = if f.is_closure || self.guarded[id.index()] {
            ""
        } else if eff == Effects::NONE {
            self.need("hint_const");
            "CONST_FN "
        } else if eff == Effects::READS {
            self.need("hint_const");
            "PURE_FN "
        } else {
            ""
        };
        let ret = cty(f.ret);
        let sep = if ret.ends_with('*') { "" } else { " " };
        if f.ret == IrTy::Unit {
            self.need("unit");
        }
        if f.is_closure || f.regs.contains(&IrTy::Ref) {
            self.need("objects");
        }
        let header = format!("static {attr}{ret}{sep}{name}({})", sig_params.join(", "));
        let proto = format!("{header};");

        // locals
        let mut locals: BTreeMap<&'static str, Vec<String>> = BTreeMap::new();
        let mut roots: Vec<String> = Vec::new();
        let mut cells: Vec<usize> = Vec::new();
        for i in 0..nregs {
            if !(used[i] || i < params || (pinned[i] && defined[i])) {
                continue;
            }
            if f.regs[i] == IrTy::Unit {
                self.need("unit");
            }
            if pinned[i] {
                cells.push(i);
                roots.push(format!("&c{i}"));
                continue;
            }
            if i < params {
                if f.regs[i] == IrTy::Ref {
                    roots.push(format!("&v{i}"));
                }
                continue;
            }
            if consts[i].is_some() {
                continue;
            }
            if f.regs[i] == IrTy::Unit {
                continue; // the unit value is the constant 0 and needs no variable
            }
            let (t, init) = if f.regs[i] == IrTy::Ref {
                ("Obj", format!("*v{i} = NULL"))
            } else {
                (cty(f.regs[i]), format!("v{i} = 0"))
            };
            locals.entry(t).or_default().push(init);
            if f.regs[i] == IrTy::Ref {
                roots.push(format!("&v{i}"));
            }
        }
        let need_frame = may_gc && !roots.is_empty();
        let guarded = self.guarded[id.index()];

        let mut fe = FnEmit {
            g: self,
            f,
            used: used.clone(),
            pinned: pinned.clone(),
            consts: consts.clone(),
            out: Vec::new(),
            frame: need_frame,
            guarded,
            name: name.clone(),
        };
        // layout / labels
        let mut targets = vec![false; f.blocks.len()];
        for (k, &b) in order.iter().enumerate() {
            let next = order.get(k + 1).copied();
            for s in f.blocks[b.index()].term.successors() {
                if Some(s) != next {
                    targets[s.index()] = true;
                }
            }
        }
        for (k, &b) in order.iter().enumerate() {
            if targets[b.index()] {
                fe.out.push(format!("B{}:", b.index()));
            }
            let next = order.get(k + 1).copied();
            fe.block(b, next)?;
        }
        let body_lines = fe.out;

        let mut text = String::new();
        let _ = writeln!(text, "{header} {{");
        for (t, vars) in &locals {
            let _ = writeln!(text, "    {t} {};", vars.join(", "));
        }
        if !cells.is_empty() {
            let list: Vec<String> = cells.iter().map(|i| format!("*c{i} = NULL")).collect();
            let _ = writeln!(text, "    Obj {};", list.join(", "));
        }
        if need_frame {
            self.needs_all(&["gcstate", "gc_collect"]);
            let _ = writeln!(text, "    Obj **fr_roots[{}] = {{{}}};", roots.len(), roots.join(", "));
            let _ = writeln!(text, "    Frame fr = {{NULL, fr_roots, {}}};", roots.len());
        }
        if guarded {
            self.need("depth");
            self.need_checked(&["trap_stack", "hint_expect"]);
            let _ = writeln!(text, "    DEPTH_ENTER();");
        }
        if need_frame {
            let _ = writeln!(text, "    fr.up = gc_top;\n    gc_top = &fr;");
        }
        if f.is_closure && !uses_up {
            let _ = writeln!(text, "    (void)up;");
        }
        for (i, &is_pinned) in pinned.iter().enumerate().take(params) {
            if (!used[i] || f.regs[i] == IrTy::Unit) && !is_pinned {
                let _ = writeln!(text, "    (void)v{i};");
            }
        }
        for &i in &cells {
            let ty = f.regs[i];
            self.need("cell_new");
            let init = if i < params {
                let w = self.as_word(ty, &format!("p{i}"));
                format!("cell_new({}, {w})", (ty == IrTy::Ref) as u8)
            } else {
                format!("cell_new({}, 0)", (ty == IrTy::Ref) as u8)
            };
            let _ = writeln!(text, "    c{i} = {init};");
        }
        for l in &body_lines {
            if l.ends_with(':') && l.starts_with('B') && !l.contains(' ') {
                let _ = writeln!(text, "{l}");
            } else {
                let _ = writeln!(text, "    {l}");
            }
        }
        text.push('}');
        Ok((proto, text))
    }
}

/// Does this instruction allocate heap objects (so the collector may run just before it)?
fn allocates(inst: &Inst) -> bool {
    match inst {
        Inst::NewStruct { .. }
        | Inst::NewEnum { .. }
        | Inst::NewArray { .. }
        | Inst::NewMap { .. }
        | Inst::NewSet { .. }
        | Inst::MakeClosure { .. }
        | Inst::CloseUpvals { .. } => true,
        Inst::Intrinsic { b, .. } => b.effects().allocates(),
        _ => false,
    }
}

struct FnEmit<'a, 'p> {
    g: &'a mut Gen<'p>,
    f: &'p IrFunc,
    used: Vec<bool>,
    pinned: Vec<bool>,
    /// Registers assigned once, a numeric constant: their uses are replaced by the literal.
    consts: Vec<Option<u64>>,
    out: Vec<String>,
    frame: bool,
    guarded: bool,
    name: String,
}

impl FnEmit<'_, '_> {
    fn line(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    fn ty(&self, r: VReg) -> IrTy {
        self.f.regs[r.index()]
    }

    fn rd(&mut self, r: VReg) -> String {
        if self.ty(r) == IrTy::Unit && !self.pinned[r.index()] {
            return "0".into();
        }
        if let Some(bits) = self.consts[r.index()] {
            let l = lit(self.ty(r), bits);
            if l.contains(['N', 'I']) {
                self.g.need("hdr_math");
            }
            return l;
        }
        if self.pinned[r.index()] {
            let ty = self.ty(r);
            self.g.need("objects");
            self.g.of_word(ty, &format!("CELL(c{})->v", r.index()))
        } else {
            format!("v{}", r.index())
        }
    }

    /// The word form of a register, for storing in a container.
    fn w(&mut self, r: VReg) -> String {
        let ty = self.ty(r);
        let e = self.rd(r);
        self.g.as_word(ty, &e)
    }

    /// The statement assigning `expr` (of the register's C type) to `dst`.
    fn set(&mut self, dst: VReg, expr: &str) -> String {
        if self.pinned[dst.index()] {
            let ty = self.ty(dst);
            self.g.need("objects");
            let w = self.g.as_word(ty, expr);
            return format!("CELL(c{})->v = {w};", dst.index());
        }
        if !self.used[dst.index()] || self.ty(dst) == IrTy::Unit {
            // nothing reads the result: keep the call, drop plain expressions
            return if expr.contains('(') { format!("(void){};", paren(expr)) } else { String::new() };
        }
        format!("v{} = {expr};", dst.index())
    }

    fn emit_set(&mut self, dst: VReg, expr: &str) {
        let s = self.set(dst, expr);
        if !s.is_empty() {
            self.line(s);
        }
    }

    /// Assigns a word read from a container.
    fn set_word(&mut self, dst: VReg, wexpr: &str) {
        let ty = self.ty(dst);
        let e = self.g.of_word(ty, wexpr);
        self.emit_set(dst, &e);
    }

    fn gc_point(&mut self) {
        self.g.needs_all(&["gcstate", "gc_collect", "hint_expect"]);
        self.line("GC_POINT();");
    }

    fn epilogue(&mut self) {
        if self.guarded {
            self.line("DEPTH_LEAVE();");
        }
        if self.frame {
            self.line("gc_top = fr.up;");
        }
    }

    /// Runs `f` with the name of a C variable holding a freshly allocated object; when `dst` lives in a
    /// cell the object is built in a temporary first.
    fn with_obj(&mut self, dst: VReg, alloc: &str, f: impl FnOnce(&mut Self, &str)) {
        if self.pinned[dst.index()] || !self.used[dst.index()] {
            self.line("{");
            self.line(format!("    Obj *t = {alloc};"));
            let start = self.out.len();
            f(self, "t");
            for l in &mut self.out[start..] {
                *l = format!("    {l}");
            }
            let s = self.set(dst, "t");
            self.line(format!("    {s}"));
            self.line("}");
        } else {
            let v = format!("v{}", dst.index());
            self.line(format!("{v} = {alloc};"));
            f(self, &v);
        }
    }

    fn mask_of(&self, regs: &[VReg]) -> Result<u64, String> {
        if regs.len() > 64 {
            return Err(format!("`{}`: a struct or enum variant with more than 64 fields is not supported", self.name));
        }
        Ok(regs.iter().enumerate().filter(|(_, &r)| self.ty(r) == IrTy::Ref).fold(0u64, |m, (i, _)| m | (1 << i)))
    }

    fn block(&mut self, b: BlockId, next: Option<BlockId>) -> Result<(), String> {
        let f = self.f;
        let blk = &f.blocks[b.index()];
        for inst in &blk.insts {
            self.inst(inst)?;
        }
        match &blk.term {
            Term::Jump(t) => {
                if Some(*t) != next {
                    self.line(format!("goto B{};", t.index()));
                }
            }
            Term::Branch { cond, then, els } => {
                let c = self.rd(*cond);
                self.branch(c, *then, *els, next);
            }
            Term::BranchTag { obj, tag, then, els } => {
                let o = self.rd(*obj);
                self.g.need("objects");
                self.branch(format!("TAG({o}) == {tag}"), *then, *els, next);
            }
            Term::Return(r) => {
                let e = self.rd(*r);
                self.epilogue();
                self.line(format!("return {e};"));
            }
            Term::Unreachable => {
                self.g.need("trap_unreach");
                let n = self.f.name.clone();
                self.line(format!("trap_unreach({});", c_string(&n)));
            }
        }
        Ok(())
    }

    fn branch(&mut self, cond: String, then: BlockId, els: BlockId, next: Option<BlockId>) {
        if then == els {
            if Some(then) != next {
                self.line(format!("goto B{};", then.index()));
            }
        } else if Some(then) == next {
            self.line(format!("if (!({cond})) goto B{};", els.index()));
        } else if Some(els) == next {
            self.line(format!("if ({cond}) goto B{};", then.index()));
        } else {
            self.line(format!("if ({cond}) goto B{};", then.index()));
            self.line(format!("goto B{};", els.index()));
        }
    }

    fn inst(&mut self, inst: &Inst) -> Result<(), String> {
        let prog = self.g.prog;
        match inst {
            Inst::Const { dst, .. } if self.consts[dst.index()].is_some() => {}
            Inst::Const { dst, val } => {
                let ty = self.ty(*dst);
                match val {
                    Const::Bits(b) => {
                        let l = lit(ty, *b);
                        if matches!(ty, IrTy::Num(NumTy::F32 | NumTy::F64)) && l.contains(['N', 'I']) {
                            self.g.need("hdr_math");
                        }
                        self.emit_set(*dst, &l)
                    }
                    Const::Unit => self.emit_set(*dst, "0"),
                    Const::Str(i) => {
                        let k = self.g.str_index(*i);
                        self.emit_set(*dst, &format!("kstr({k})"));
                    }
                }
            }
            Inst::Move { dst, src } => {
                let e = self.rd(*src);
                self.emit_set(*dst, &e);
            }
            Inst::Arith { dst, op, ty, lhs, rhs } => {
                let (a, b) = (self.rd(*lhs), self.rd(*rhs));
                let c = num_cty(*ty);
                let e = if ty.is_float() {
                    match op {
                        ArithOp::Add => format!("{a} + {b}"),
                        ArithOp::Sub => format!("{a} - {b}"),
                        ArithOp::Mul => format!("{a} * {b}"),
                        ArithOp::Div => format!("{a} / {b}"),
                        ArithOp::Rem => {
                            self.g.need("hdr_math");
                            format!("{}({a}, {b})", if *ty == NumTy::F32 { "fmodf" } else { "fmod" })
                        }
                        _ => return Err(format!("`{}`: operator {} on a float", self.name, op.name())),
                    }
                } else {
                    match op {
                        ArithOp::And => format!("({c})({a} & {b})"),
                        ArithOp::Or => format!("({c})({a} | {b})"),
                        ArithOp::Xor => format!("({c})({a} ^ {b})"),
                        ArithOp::Shl | ArithOp::Shr => {
                            let n = self.g.arith_fn(*op, *ty);
                            format!("{n}({a}, (int64_t){b})")
                        }
                        _ => {
                            let n = self.g.arith_fn(*op, *ty);
                            format!("{n}({a}, {b})")
                        }
                    }
                };
                self.emit_set(*dst, &e);
            }
            Inst::Cmp { dst, op, ty, lhs, rhs } => {
                let (a, b) = (self.rd(*lhs), self.rd(*rhs));
                let e = match ty {
                    CmpTy::Num(_) => format!("{a} {} {b}", cmp_op(*op)),
                    CmpTy::Str => match op {
                        CmpOp::Eq => {
                            self.g.need("str_eq");
                            format!("str_eq({a}, {b})")
                        }
                        CmpOp::Ne => {
                            self.g.need("str_eq");
                            format!("!str_eq({a}, {b})")
                        }
                        _ => {
                            self.g.need("str_cmp");
                            format!("str_cmp({a}, {b}) {} 0", cmp_op(*op))
                        }
                    },
                };
                self.emit_set(*dst, &e);
            }
            Inst::Neg { dst, ty, src } => {
                let a = self.rd(*src);
                let e = if ty.is_float() {
                    format!("-{a}")
                } else {
                    let n = self.g.neg_fn(*ty);
                    format!("{n}({a})")
                };
                self.emit_set(*dst, &e);
            }
            Inst::BitNot { dst, ty, src } => {
                let a = self.rd(*src);
                self.emit_set(*dst, &format!("({})~{a}", num_cty(*ty)));
            }
            Inst::BoolNot { dst, src } => {
                let a = self.rd(*src);
                self.emit_set(*dst, &format!("!{a}"));
            }
            Inst::Cast { dst, from, to, src } => {
                let a = self.rd(*src);
                let c = num_cty(*to);
                let e = if from == to {
                    a
                } else if from.is_float() && !to.is_float() {
                    let n = self.g.f2i_fn(*to);
                    format!("{n}({a})")
                } else {
                    format!("({c}){a}")
                };
                self.emit_set(*dst, &e);
            }

            Inst::NewStruct { dst, fields, .. } => {
                self.alloc_agg(*dst, 0, fields)?;
            }
            Inst::NewEnum { dst, tag, fields, .. } => {
                self.alloc_agg(*dst, *tag, fields)?;
            }
            Inst::GetField { dst, obj, idx } | Inst::EnumField { dst, obj, idx } => {
                self.g.need("objects");
                let o = self.rd(*obj);
                self.set_word(*dst, &format!("AGG({o})->f[{idx}]"));
            }
            Inst::SetField { obj, idx, src } => {
                self.g.need("objects");
                let o = self.rd(*obj);
                let w = self.w(*src);
                self.line(format!("AGG({o})->f[{idx}] = {w};"));
            }
            Inst::EnumTag { dst, obj } => {
                self.g.need("objects");
                let o = self.rd(*obj);
                let c = cty(self.ty(*dst));
                self.emit_set(*dst, &format!("({c})TAG({o})"));
            }
            Inst::NewArray { dst, desc, elems } => {
                self.gc_point();
                let TypeDesc::Array(e) = prog.desc(*desc) else {
                    return Err("NewArray with a non-array descriptor".into());
                };
                let is_ref = prog.desc(*e).is_ref() as u8;
                self.g.need("arr_new");
                let ws: Vec<String> = elems.iter().map(|&r| self.w(r)).collect();
                self.with_obj(*dst, &format!("arr_new({}, {is_ref})", elems.len()), |me, v| {
                    for (i, w) in ws.iter().enumerate() {
                        me.line(format!("ARR({v})->a[{i}] = {w};"));
                    }
                    if !ws.is_empty() {
                        me.line(format!("ARR({v})->len = {};", ws.len()));
                    }
                });
            }
            Inst::ArrayGet { dst, arr, idx, .. } => {
                self.g.need("arr_get");
                let (a, i) = (self.rd(*arr), self.rd(*idx));
                self.set_word(*dst, &format!("arr_get({a}, (int64_t){i})"));
            }
            Inst::ArraySet { arr, idx, src, .. } => {
                self.g.need("arr_set");
                let (a, i, w) = (self.rd(*arr), self.rd(*idx), self.w(*src));
                self.line(format!("arr_set({a}, (int64_t){i}, {w});"));
            }
            Inst::NewMap { dst, desc, entries } => {
                self.gc_point();
                let TypeDesc::Map(k, v) = prog.desc(*desc) else {
                    return Err("NewMap with a non-map descriptor".into());
                };
                let flags = map_flags(prog, *k, Some(*v));
                self.g.needs_all(&["map_new", "map_put"]);
                let ws: Vec<(String, String)> = entries.iter().map(|&(k, v)| (self.w(k), self.w(v))).collect();
                self.with_obj(*dst, &format!("map_new({flags})"), |me, m| {
                    for (k, v) in &ws {
                        me.line(format!("map_put({m}, {k}, {v});"));
                    }
                });
            }
            Inst::MapGet { dst, map, key } => {
                self.g.need("map_get");
                let (m, k) = (self.rd(*map), self.w(*key));
                self.set_word(*dst, &format!("map_get({m}, {k})"));
            }
            Inst::MapSet { map, key, val } => {
                self.g.need("map_put");
                let (m, k, v) = (self.rd(*map), self.w(*key), self.w(*val));
                self.line(format!("map_put({m}, {k}, {v});"));
            }
            Inst::NewSet { dst, desc, elems } => {
                self.gc_point();
                let TypeDesc::Set(e) = prog.desc(*desc) else { return Err("NewSet with a non-set descriptor".into()) };
                let flags = map_flags(prog, *e, None);
                self.g.needs_all(&["map_new", "map_put"]);
                let ws: Vec<String> = elems.iter().map(|&r| self.w(r)).collect();
                self.with_obj(*dst, &format!("map_new({flags})"), |me, m| {
                    for w in &ws {
                        me.line(format!("map_put({m}, {w}, 0);"));
                    }
                });
            }

            Inst::Call { dst, func, args } => {
                let callee = prog.func(*func);
                let a: Vec<String> = args.iter().map(|&r| self.rd(r)).collect();
                let mut call_args = a;
                if callee.is_closure {
                    call_args.insert(0, "NULL".into());
                }
                let e = format!("{}({})", self.g.names[func.index()], call_args.join(", "));
                self.emit_set(*dst, &e);
            }
            Inst::CallClosure { dst, callee, args } => {
                self.g.need("objects");
                let c = self.rd(*callee);
                let tys: Vec<IrTy> = args.iter().map(|&r| self.ty(r)).collect();
                let fp = self.g.fnptr(self.ty(*dst), &tys);
                let a: Vec<String> = args.iter().map(|&r| self.rd(r)).collect();
                let mut call_args = vec![format!("CLO({c})->c")];
                call_args.extend(a);
                self.emit_set(*dst, &format!("(({fp})CLO({c})->fn)({})", call_args.join(", ")));
            }
            Inst::MakeClosure { dst, func, captures } => {
                self.gc_point();
                self.g.needs_all(&["clo_new", "objects"]);
                let target = prog.func(*func);
                let mut fname = self.g.names[func.index()].clone();
                if !target.is_closure {
                    self.g.wrappers.insert(func.index());
                    fname.push_str("_clo");
                }
                let mut caps = Vec::new();
                for c in captures {
                    caps.push(match c {
                        Capture::Local(r) => {
                            if !self.pinned[r.index()] {
                                return Err(format!("`{}`: captured register v{} is not pinned", self.name, r.index()));
                            }
                            format!("c{}", r.index())
                        }
                        Capture::Upvalue(i) => format!("up[{i}]"),
                    });
                }
                self.with_obj(*dst, &format!("clo_new((void (*)(void)){fname}, {})", caps.len()), |me, v| {
                    for (i, c) in caps.iter().enumerate() {
                        me.line(format!("CLO({v})->c[{i}] = {c};"));
                    }
                });
            }
            Inst::GetUpval { dst, idx } => {
                self.g.need("objects");
                self.set_word(*dst, &format!("CELL(up[{idx}])->v"));
            }
            Inst::SetUpval { idx, src } => {
                self.g.need("objects");
                let w = self.w(*src);
                self.line(format!("CELL(up[{idx}])->v = {w};"));
            }
            Inst::CloseUpvals { regs } => {
                self.gc_point();
                self.g.needs_all(&["cell_new", "objects"]);
                for r in regs {
                    if !self.pinned[r.index()] {
                        continue;
                    }
                    let is_ref = (self.ty(*r) == IrTy::Ref) as u8;
                    // closures keep the old cell; the function continues with a fresh one
                    self.line(format!("c{0} = cell_new({is_ref}, CELL(c{0})->v);", r.index()));
                }
            }
            Inst::GetGlobal { dst, slot } => {
                self.g.globals.insert(slot.index());
                let g = self.g.gnames[slot.index()].clone();
                self.emit_set(*dst, &g);
            }
            Inst::SetGlobal { slot, src } => {
                self.g.globals.insert(slot.index());
                let g = self.g.gnames[slot.index()].clone();
                let e = self.rd(*src);
                self.line(format!("{g} = {e};"));
            }
            Inst::Intrinsic { dst, b, desc, args } => self.intrinsic(*dst, *b, *desc, args)?,
        }
        Ok(())
    }

    fn alloc_agg(&mut self, dst: VReg, tag: u32, fields: &[VReg]) -> Result<(), String> {
        self.gc_point();
        self.g.needs_all(&["agg_new", "objects"]);
        let mask = self.mask_of(fields)?;
        let ws: Vec<String> = fields.iter().map(|&r| self.w(r)).collect();
        let alloc = format!("agg_new({tag}, {}, {mask})", fields.len());
        self.with_obj(dst, &alloc, |me, v| {
            for (i, w) in ws.iter().enumerate() {
                me.line(format!("AGG({v})->f[{i}] = {w};"));
            }
        });
        Ok(())
    }

    // ---- builtins -------------------------------------------------------------------------------------

    fn num_of(&self, desc: Option<DescId>) -> Result<NumTy, String> {
        match desc.map(|d| self.g.prog.desc(d)) {
            Some(TypeDesc::Num(n)) => Ok(*n),
            other => Err(format!("`{}`: expected a numeric descriptor, got {other:?}", self.name)),
        }
    }

    /// The element descriptor of an array, set or map receiver.
    fn elem_desc(&self, desc: Option<DescId>) -> Result<DescId, String> {
        match desc.map(|d| self.g.prog.desc(d)) {
            Some(TypeDesc::Array(e)) | Some(TypeDesc::Set(e)) => Ok(*e),
            Some(TypeDesc::Map(k, _)) => Ok(*k),
            other => Err(format!("`{}`: expected a collection descriptor, got {other:?}", self.name)),
        }
    }

    fn eq_fn(&mut self, d: DescId) -> &'static str {
        let name = match self.g.prog.desc(d) {
            TypeDesc::Num(NumTy::F64) => "eq_f64",
            TypeDesc::Num(NumTy::F32) => "eq_f32",
            TypeDesc::Str => "eq_str",
            _ => "eq_bits",
        };
        self.g.need(name);
        name
    }

    fn cmp_fn(&mut self, d: DescId) -> &'static str {
        let name = match self.g.prog.desc(d) {
            TypeDesc::Num(NumTy::F64) => "cmp_f64",
            TypeDesc::Num(NumTy::F32) => "cmp_f32",
            TypeDesc::Num(n) if n.is_signed() => "cmp_s64",
            TypeDesc::Str => "cmp_str",
            _ => "cmp_u64",
        };
        self.g.need(name);
        name
    }

    fn fmt_kind(&self, d: DescId) -> Result<&'static str, String> {
        Ok(match self.g.prog.desc(d) {
            TypeDesc::Num(NumTy::F64) => "FK_F64",
            TypeDesc::Num(NumTy::F32) => "FK_F32",
            TypeDesc::Num(n) if n.is_signed() => "FK_SINT",
            TypeDesc::Num(_) => "FK_UINT",
            TypeDesc::Str => "FK_STR",
            TypeDesc::Bool => "FK_BOOL",
            TypeDesc::Char => "FK_CHAR",
            other => return Err(format!("`{}`: a value of type {other:?} cannot take a format specifier", self.name)),
        })
    }

    /// Appends the text of register `r` (of type `d`) to the scratch buffer.
    fn put_reg(&mut self, d: DescId, r: VReg) -> String {
        let e = self.rd(r);
        self.g.need("sbtype");
        // top-level strings and characters print raw
        match self.g.prog.desc(d) {
            TypeDesc::Num(NumTy::F64) => {
                self.g.need("sb_float");
                format!("sb_float(&rt_sb, {e}, 0);")
            }
            TypeDesc::Num(NumTy::F32) => {
                self.g.need("sb_float");
                format!("sb_float(&rt_sb, (double){e}, 1);")
            }
            TypeDesc::Num(n) if n.is_signed() => {
                self.g.need("sb_i64");
                format!("sb_i64(&rt_sb, {e});")
            }
            TypeDesc::Num(_) => {
                self.g.need("sb_u64");
                format!("sb_u64(&rt_sb, {e});")
            }
            TypeDesc::Bool => {
                self.g.need("sb_puts");
                format!("sb_puts(&rt_sb, {e} ? \"true\" : \"false\");")
            }
            TypeDesc::Char => {
                self.g.need("sb_cp");
                format!("sb_cp(&rt_sb, {e});")
            }
            TypeDesc::Str => {
                self.g.needs_all(&["sb_put", "objects"]);
                format!("sb_put(&rt_sb, STR({e})->s, STR({e})->len);")
            }
            _ => {
                let w = self.w(r);
                self.g.put_word(d, &w, false)
            }
        }
    }

    fn intrinsic(&mut self, dst: VReg, b: Builtin, desc: Option<DescId>, args: &[VReg]) -> Result<(), String> {
        use Builtin::*;
        if b.effects().allocates() {
            self.gc_point();
        }
        let a: Vec<String> = args.iter().map(|&r| self.rd(r)).collect();
        let aw = |me: &mut Self, i: usize| me.w(args[i]);
        let dst_ty = self.ty(dst);
        let int64 = |e: &str| format!("(int64_t){}", paren(e));
        macro_rules! call {
            ($piece:expr, $e:expr) => {{
                self.g.need($piece);
                let e: String = $e;
                self.emit_set(dst, &e);
            }};
        }
        macro_rules! stmt {
            ($piece:expr, $s:expr) => {{
                self.g.need($piece);
                let s: String = $s;
                self.line(s);
            }};
        }
        macro_rules! result_cast {
            ($e:expr) => {{
                let c = cty(dst_ty);
                let e: String = $e;
                self.emit_set(dst, &format!("({c}){}", paren(&e)));
            }};
        }
        let unit_result = |me: &mut Self| {
            if me.used[dst.index()] || me.pinned[dst.index()] {
                me.emit_set(dst, "0");
            }
        };
        match b {
            Print | Println => {
                let nl = (b == Println) as u8;
                self.g.need("objects");
                match (args.first(), desc) {
                    (Some(&r), Some(d)) => {
                        if matches!(self.g.prog.desc(d), TypeDesc::Str) {
                            self.g.need("out_str");
                            let s = self.rd(r);
                            self.line(format!("out_str({s}, {nl});"));
                        } else {
                            self.g.need("out_sb");
                            let put = self.put_reg(d, r);
                            self.line(put);
                            self.line(format!("out_sb({nl});"));
                        }
                    }
                    _ => {
                        if nl == 1 {
                            self.g.need("out_nl");
                            self.line("out_nl();");
                        }
                    }
                }
                unit_result(self);
            }
            Input => call!("rt_input", "rt_input()".into()),
            ReadLine => call!("rt_read_line", "rt_read_line()".into()),
            Args => call!("rt_args", "rt_args()".into()),
            Panic => {
                self.g.need("trap_panic");
                self.line(format!("trap_panic({});", a[0]));
                unit_result(self);
            }
            Assert => {
                self.g.needs_all(&["trap_assert", "hint_expect"]);
                let msg = a.get(1).cloned().unwrap_or_else(|| "NULL".into());
                self.line(format!("if (UNLIKELY(!{})) trap_assert({msg});", a[0]));
                unit_result(self);
            }
            ToString => {
                let d = desc.ok_or("to_string without a descriptor")?;
                let put = self.put_reg(d, args[0]);
                self.line(put);
                call!("sb_take", "sb_take()".into());
            }
            Format => {
                let d = desc.ok_or("format without a descriptor")?;
                let kind = self.fmt_kind(d)?;
                let w = aw(self, 0);
                self.g.need("objects");
                call!("fmt_value", format!("fmt_value({kind}, {w}, {})", a[1]));
            }

            StrConcat => call!("str_cat", format!("str_cat({}, {})", a[0], a[1])),
            StrLen => call!("str_len", format!("str_len({})", a[0])),
            StrIsEmpty => {
                self.g.need("objects");
                self.emit_set(dst, &format!("STR({})->len == 0", a[0]));
            }
            StrChars => call!("str_chars", format!("str_chars({})", a[0])),
            StrSubstring => {
                call!("str_substring", format!("str_substring({}, {}, {})", a[0], int64(&a[1]), int64(&a[2])))
            }
            StrContains => call!("str_pos", format!("str_pos({}, {}) >= 0", a[0], a[1])),
            StrStartsWith => call!("str_starts", format!("str_starts({}, {})", a[0], a[1])),
            StrEndsWith => call!("str_ends", format!("str_ends({}, {})", a[0], a[1])),
            StrToUpper => call!("str_upper", format!("str_upper({})", a[0])),
            StrToLower => call!("str_lower", format!("str_lower({})", a[0])),
            StrTrim => call!("str_trim", format!("str_trim({})", a[0])),
            StrSplit => call!("str_split", format!("str_split({}, {})", a[0], a[1])),
            StrCharAt => call!("str_char_at", format!("str_char_at({}, {})", a[0], int64(&a[1]))),
            StrFind => call!("str_find", format!("str_find({}, {})", a[0], a[1])),
            StrParseI64 => call!("parse_i64", format!("parse_i64({})", a[0])),
            StrParseU64 => call!("parse_u64", format!("parse_u64({})", a[0])),
            StrParseF64 => call!("parse_f64", format!("parse_f64({})", a[0])),
            StrTryParseI64 => call!("try_parse_i64", format!("try_parse_i64({})", a[0])),
            StrTryParseU64 => call!("try_parse_u64", format!("try_parse_u64({})", a[0])),
            StrTryParseF64 => call!("try_parse_f64", format!("try_parse_f64({})", a[0])),
            StrGetChar => call!("str_get_char", format!("str_get_char({}, {})", a[0], int64(&a[1]))),
            StrTrySubstring => {
                call!("str_try_substring", format!("str_try_substring({}, {}, {})", a[0], int64(&a[1]), int64(&a[2])))
            }
            StrIndexOf => call!("str_index_of", format!("str_index_of({}, {})", a[0], a[1])),

            ArrLen => {
                self.g.need("objects");
                result_cast!(format!("ARR({})->len", a[0]));
            }
            ArrIsEmpty => {
                self.g.need("objects");
                self.emit_set(dst, &format!("ARR({})->len == 0", a[0]));
            }
            ArrPush => {
                let w = aw(self, 1);
                stmt!("arr_push", format!("arr_push({}, {w});", a[0]));
                unit_result(self);
            }
            ArrPop => {
                self.g.need("arr_pop");
                self.set_word(dst, &format!("arr_pop({})", a[0]));
            }
            ArrClear => {
                self.g.need("objects");
                self.line(format!("ARR({})->len = 0;", a[0]));
                unit_result(self);
            }
            ArrContains => {
                let eq = self.eq_fn(self.elem_desc(desc)?);
                let w = aw(self, 1);
                call!("arr_contains", format!("arr_contains({}, {w}, {eq})", a[0]));
            }
            ArrInsert => {
                let w = aw(self, 2);
                stmt!("arr_insert", format!("arr_insert({}, {}, {w});", a[0], int64(&a[1])));
                unit_result(self);
            }
            ArrRemove => {
                self.g.need("arr_remove");
                self.set_word(dst, &format!("arr_remove({}, {})", a[0], int64(&a[1])));
            }
            ArrReverse => {
                stmt!("arr_reverse", format!("arr_reverse({});", a[0]));
                unit_result(self);
            }
            ArrClone => call!("arr_clone", format!("arr_clone({})", a[0])),
            ArrJoin => call!("str_join", format!("str_join({}, {})", a[0], a[1])),
            ArrSort => {
                let cmp = self.cmp_fn(self.elem_desc(desc)?);
                stmt!("arr_sort", format!("arr_sort({}, {cmp});", a[0]));
                unit_result(self);
            }
            ArrGet => call!("arr_try_get", format!("arr_try_get({}, {})", a[0], int64(&a[1]))),
            ArrTryPop => call!("arr_try_pop", format!("arr_try_pop({})", a[0])),
            ArrTryRemove => call!("arr_try_remove", format!("arr_try_remove({}, {})", a[0], int64(&a[1]))),

            MapLen | SetLen => {
                self.g.need("objects");
                result_cast!(format!("MAP({})->len", a[0]));
            }
            MapIsEmpty | SetIsEmpty => {
                self.g.need("objects");
                self.emit_set(dst, &format!("MAP({})->len == 0", a[0]));
            }
            MapInsert => {
                let (k, v) = (aw(self, 1), aw(self, 2));
                stmt!("map_put", format!("map_put({}, {k}, {v});", a[0]));
                unit_result(self);
            }
            SetInsert => {
                let k = aw(self, 1);
                call!("map_put", format!("map_put({}, {k}, 0)", a[0]));
            }
            MapGetOr => {
                let (k, d) = (aw(self, 1), aw(self, 2));
                self.g.need("map_get_or");
                self.set_word(dst, &format!("map_get_or({}, {k}, {d})", a[0]));
            }
            MapGet => {
                let k = aw(self, 1);
                call!("map_try_get", format!("map_try_get({}, {k})", a[0]));
            }
            MapContainsKey | SetContains => {
                let k = aw(self, 1);
                call!("map_find", format!("map_find({}, {k}) >= 0", a[0]));
            }
            MapRemove | SetRemove => {
                let k = aw(self, 1);
                call!("map_del", format!("map_del({}, {k})", a[0]));
            }
            MapKeys | SetToArray => call!("map_keys", format!("map_keys({})", a[0])),
            MapValues => call!("map_values", format!("map_values({})", a[0])),
            MapClear | SetClear => {
                stmt!("map_clear", format!("map_clear({});", a[0]));
                unit_result(self);
            }

            CheckedAdd | CheckedSub | CheckedMul | CheckedDiv | CheckedRem | CheckedPow | CheckedNeg | CheckedShl
            | CheckedShr => {
                let d = desc.ok_or("checked operation without a result descriptor")?;
                let TypeDesc::Enum { variants, .. } = self.g.prog.desc(d) else {
                    return Err("checked operation with a non-enum result".into());
                };
                let ty = match variants[0].1.first().map(|p| self.g.prog.desc(*p)) {
                    Some(TypeDesc::Num(n)) => *n,
                    other => return Err(format!("checked operation with payload {other:?}")),
                };
                let chk = self.g.chk_fn(b, ty, true);
                self.g.needs_all(&["opt_some", "opt_none"]);
                let rhs = match b {
                    CheckedNeg => String::new(),
                    CheckedShl | CheckedShr => format!(", {}", int64(&a[1])),
                    CheckedPow => format!(", (uint64_t){}", paren(&a[1])),
                    _ => format!(", {}", a[1]),
                };
                let c = num_cty(ty);
                let w = self.g.as_word(IrTy::Num(ty), "t");
                self.line("{");
                self.line(format!("    {c} t;"));
                let target = if self.pinned[dst.index()] || !self.used[dst.index()] {
                    "o".to_string()
                } else {
                    format!("v{}", dst.index())
                };
                if target == "o" {
                    self.line("    Obj *o;");
                }
                self.line(format!("    {target} = {chk}({}{rhs}, &t) ? opt_some({w}, 0) : opt_none();", a[0]));
                if target == "o" {
                    let s = self.set(dst, "o");
                    self.line(format!("    {s}"));
                }
                self.line("}");
            }

            Abs => {
                let n = self.num_of(desc)?;
                let e = if n.is_float() {
                    self.g.need("hdr_math");
                    format!("{}({})", if n == NumTy::F32 { "fabsf" } else { "fabs" }, a[0])
                } else if n.is_signed() {
                    let f = self.g.abs_fn(n);
                    format!("{f}({})", a[0])
                } else {
                    a[0].clone()
                };
                self.emit_set(dst, &e);
            }
            Pow => {
                let n = self.num_of(desc)?;
                let e = if n.is_float() {
                    self.g.need("hdr_math");
                    format!("{}({}, {})", if n == NumTy::F32 { "powf" } else { "pow" }, a[0], a[1])
                } else {
                    let f = self.g.pow_fn(n);
                    format!("{f}({}, (uint64_t){})", a[0], paren(&a[1]))
                };
                self.emit_set(dst, &e);
            }
            Min | Max => {
                let n = self.num_of(desc)?;
                let (x, y) = (&a[0], &a[1]);
                let want_max = b == Max;
                let pick = if want_max { format!("{x} < {y} ? {y} : {x}") } else { format!("{x} < {y} ? {x} : {y}") };
                let e = if n.is_float() {
                    self.g.need("hdr_math");
                    format!("isnan({x}) ? {y} : isnan({y}) ? {x} : ({pick})")
                } else {
                    pick
                };
                self.emit_set(dst, &e);
            }
            Sqrt | Sin | Cos | Tan | Floor | Ceil | Round | Ln | Exp => {
                let n = self.num_of(desc)?;
                let base = match b {
                    Sqrt => "sqrt",
                    Sin => "sin",
                    Cos => "cos",
                    Tan => "tan",
                    Floor => "floor",
                    Ceil => "ceil",
                    Round => "round",
                    Ln => "log",
                    _ => "exp",
                };
                self.g.need("hdr_math");
                let f = if n == NumTy::F32 { format!("{base}f") } else { base.to_string() };
                self.emit_set(dst, &format!("{f}({})", a[0]));
            }
            IsNan => {
                self.g.need("hdr_math");
                self.emit_set(dst, &format!("isnan({}) != 0", a[0]));
            }
            CharIsDigit => call!("cp_is_digit", format!("cp_is_digit({})", a[0])),
            CharIsAlphabetic => call!("cp_is_alpha", format!("cp_is_alpha({})", a[0])),
            CharIsWhitespace => call!("cp_is_space", format!("cp_is_space({})", a[0])),
            CharToUpper => call!("cp_upper", format!("cp_upper({})", a[0])),
            CharToLower => call!("cp_lower", format!("cp_lower({})", a[0])),
        }
        Ok(())
    }
}

fn map_flags(prog: &IrProgram, k: DescId, v: Option<DescId>) -> String {
    let mut flags: Vec<&str> = Vec::new();
    if prog.desc(k).is_ref() {
        flags.push("MF_KREF");
    }
    if matches!(prog.desc(k), TypeDesc::Str) {
        flags.push("MF_KSTR");
    }
    match v {
        Some(v) => {
            if prog.desc(v).is_ref() {
                flags.push("MF_VREF");
            }
        }
        None => flags.push("MF_SET"),
    }
    if flags.is_empty() {
        "0".into()
    } else {
        flags.join(" | ")
    }
}
