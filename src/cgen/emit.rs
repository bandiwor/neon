//! Translation of IR functions to C.
//!
//! Registers become typed C locals (`i64 v3`, `Obj *v7`); containers store 64-bit words, so a
//! value is converted with `to_w` / `from_w` at the boundary. Control flow is `goto`, with the
//! blocks in the compiler's layout order so most jumps are fall-throughs. References live in the
//! locals the collector learns about through a per-call `Frame`; a variable captured by a closure
//! lives in a heap `Cell` shared with the closure.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;

use super::names;
use crate::builtin::Builtin;
use crate::compiler::block_layout;
use crate::effects::Effects;
use crate::ir::effects::{analyze, FuncEffects};
use crate::ir::*;
use crate::runtime::{ArithOp, CmpOp, NumTy};

pub struct Gen<'p> {
    pub prog: &'p IrProgram,
    fx: FuncEffects,
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
    /// Names of the program level (functions, globals, field and variant enumerators): no local takes them.
    taken: HashSet<String>,
    /// Field and variant enumerators per descriptor (a struct's fields, an enum's variants); descriptors of
    /// the same name and shape share one set.
    layouts: HashMap<usize, Layout>,
    layouts_used: BTreeSet<usize>,
}

/// The names of the fields of a struct, or of the variants of an enum, as C enumerators.
#[derive(Debug, Clone)]
struct Layout {
    /// The descriptor that owns the enumerators.
    owner: usize,
    is_enum: bool,
    type_name: String,
    items: Vec<String>,
}

/// The pieces of the generated program, before they are put in order.
pub struct Output {
    pub fn_protos: Vec<String>,
    pub fn_bodies: Vec<String>,
    pub typedefs: Vec<String>,
    pub globals: Vec<String>,
    pub strs: Vec<u32>,
    pub ref_globals: Vec<String>,
    /// `enum { Point_x, Point_y };`: the names of the fields and variants the program uses.
    pub enums: Vec<String>,
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
        NumTy::I8 => "i8",
        NumTy::I16 => "i16",
        NumTy::I32 => "i32",
        NumTy::I64 => "i64",
        NumTy::U8 => "u8",
        NumTy::U16 => "u16",
        NumTy::U32 => "u32",
        NumTy::U64 => "u64",
        NumTy::F32 => "f32",
        NumTy::F64 => "f64",
    }
}

fn cty(t: IrTy) -> &'static str {
    match t {
        IrTy::Num(n) => num_cty(n),
        IrTy::Bool => "bool",
        IrTy::Char => "chr",
        IrTy::Ref => "Obj *",
        // `unit` is `void`, which only a function result can be: a unit parameter or global is a dummy byte
        IrTy::Unit => "u8",
        IrTy::Packed => unreachable!("packed enums are legalized before code generation"),
    }
}

/// The C type of a function's result: `unit` (`void`) for `()`.
fn ret_cty(t: IrTy) -> &'static str {
    if t == IrTy::Unit {
        "unit"
    } else {
        cty(t)
    }
}

/// `f(x)` -> `x` when the whole of `e` is that one call.
fn unwrap_call<'a>(e: &'a str, f: &str) -> Option<&'a str> {
    let inner = e.strip_prefix(f)?.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0i32;
    for c in inner.chars() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
    }
    (depth == 0).then_some(inner)
}

/// The negation of a condition, without parentheses where it does not need them.
/// A word as an object pointer (`O2W(x)` is just `x`).
fn w2o(w: &str) -> String {
    match unwrap_call(w, "O2W") {
        Some(inner) => inner.to_string(),
        None => format!("W2O({w})"),
    }
}

fn negate(cond: &str) -> String {
    if cond.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return format!("!{cond}");
    }
    if let Some(rest) = cond.strip_prefix('!') {
        if rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return rest.to_string();
        }
    }
    if cond.starts_with("TAG(") && !cond.contains("&&") && !cond.contains("||") && cond.matches(" == ").count() == 1 {
        return cond.replace(" == ", " != ");
    }
    format!("!({cond})")
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
        IrTy::Packed => unreachable!("packed enums are legalized before code generation"),
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
            taken: HashSet::new(),
            layouts: HashMap::new(),
            layouts_used: BTreeSet::new(),
        }
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
        for n in self.names.iter().chain(&self.gnames) {
            self.taken.insert(n.clone());
        }
        for n in self.names.clone() {
            self.taken.insert(format!("{n}_clo"));
        }
    }

    /// Enumerators for the fields of every struct and the variants of every enum (`Point_x`, `Shape_Circle`).
    fn build_layouts(&mut self) {
        let mut type_bases: HashSet<String> = HashSet::new();
        let mut by_shape: HashMap<(bool, String, Vec<String>), usize> = HashMap::new();
        for (i, d) in self.prog.descs.iter().enumerate() {
            let (is_enum, name, items): (bool, &String, Vec<&String>) = match d {
                TypeDesc::Struct { name, fields } => (false, name, fields.iter().map(|f| &f.0).collect()),
                TypeDesc::Enum { name, variants } => (true, name, variants.iter().map(|v| &v.0).collect()),
                _ => continue,
            };
            let key = (is_enum, name.clone(), items.iter().map(|s| s.to_string()).collect::<Vec<_>>());
            if let Some(&owner) = by_shape.get(&key) {
                let l = self.layouts[&owner].clone();
                self.layouts.insert(i, l);
                continue;
            }
            let base = names::c_ident(name).unwrap_or_else(|| format!("T{i}"));
            let mut type_name = base.clone();
            let mut n = 2;
            while !type_bases.insert(type_name.clone()) {
                type_name = format!("{base}_{n}");
                n += 1;
            }
            let items: Vec<String> = items
                .iter()
                .enumerate()
                .map(|(k, it)| {
                    let id = names::c_ident(it).unwrap_or_else(|| k.to_string());
                    names::unique(&format!("{type_name}_{id}"), &mut self.taken)
                })
                .collect();
            by_shape.insert(key, i);
            self.layouts.insert(i, Layout { owner: i, is_enum, type_name, items });
        }
    }

    /// The enumerator declarations of the layouts the program used.
    fn layout_decls(&self) -> Vec<String> {
        self.layouts_used
            .iter()
            .map(|d| {
                let l = &self.layouts[d];
                let what = if l.is_enum { "variants of enum" } else { "fields of struct" };
                let list = l.items.join(", ");
                format!("enum {{ {list} }};   /* {what} {} */", l.type_name)
            })
            .collect()
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
            IrTy::Num(NumTy::I64) => format!("(W){}", paren(e)),
            IrTy::Num(n) if n.is_signed() => format!("(W)(i64){}", paren(e)),
            IrTy::Num(NumTy::F64) => {
                format!("f64_bits({e})")
            }
            IrTy::Num(NumTy::F32) => {
                format!("f32_bits({e})")
            }
            IrTy::Num(_) | IrTy::Bool | IrTy::Char => format!("(W){}", paren(e)),
            IrTy::Ref => match unwrap_call(e, "W2O") {
                Some(inner) => inner.to_string(),
                None => format!("O2W({e})"),
            },
            IrTy::Unit => "0".into(),
            IrTy::Packed => unreachable!("packed enums are legalized before code generation"),
        }
    }

    fn of_word(&mut self, t: IrTy, e: &str) -> String {
        match t {
            IrTy::Num(NumTy::I64) => format!("(i64){}", paren(e)),
            IrTy::Num(n) if n.is_signed() => format!("({})(i64){}", num_cty(n), paren(e)),
            IrTy::Num(NumTy::F64) => {
                format!("bits_f64({e})")
            }
            IrTy::Num(NumTy::F32) => {
                format!("bits_f32({e})")
            }
            IrTy::Num(n) => format!("({}){}", num_cty(n), paren(e)),
            IrTy::Bool => format!("(u8){}", paren(e)),
            IrTy::Char => format!("(u32){}", paren(e)),
            IrTy::Ref => match unwrap_call(e, "O2W") {
                Some(inner) => inner.to_string(),
                None => format!("W2O({e})"),
            },
            IrTy::Unit => "0".into(),
            IrTy::Packed => unreachable!("packed enums are legalized before code generation"),
        }
    }

    // ---- arithmetic -----------------------------------------------------------------------------
    //
    // The functions themselves (checked arithmetic, conversions) live in `runtime/neon.h` for every
    // integer type; the generator only has to name them.

    /// `add_i64`, `shr_u8`, ...: a checked (debug) build traps on overflow, division by zero and bad
    /// shifts; a release build wraps around and leaves the rest to C.
    fn arith_fn(&mut self, op: ArithOp, ty: NumTy) -> String {
        format!("{}_{}", op.name(), ty.name())
    }

    fn neg_fn(&mut self, ty: NumTy) -> String {
        format!("neg_{}", ty.name())
    }

    fn abs_fn(&mut self, ty: NumTy) -> String {
        format!("abs_{}", ty.name())
    }

    /// `chk_<op>_<ty>(a, b, &r)`: stores the result and returns 1, or returns 0 where the plain
    /// operation would trap.
    fn chk_fn(&mut self, b: Builtin, ty: NumTy) -> String {
        use Builtin::*;
        let op = match b {
            CheckedAdd => "add",
            CheckedSub => "sub",
            CheckedMul => "mul",
            CheckedDiv => "div",
            CheckedRem => "rem",
            CheckedShl => "shl",
            CheckedShr => "shr",
            CheckedNeg => "neg",
            CheckedPow => "pow",
            _ => unreachable!("not a checked operation"),
        };
        format!("chk_{op}_{}", ty.name())
    }

    /// `pow_<ty>(base, exp)` for integers: a checked build traps on overflow, a release build wraps.
    fn pow_fn(&mut self, ty: NumTy) -> String {
        format!("pow_{}", ty.name())
    }

    /// Float to integer conversion: saturating, NaN becomes 0.
    fn f2i_fn(&mut self, to: NumTy) -> String {
        format!("f2i_{}", to.name())
    }

    // ---- function pointer types ------------------------------------------------------------------

    fn fnptr(&mut self, ret: IrTy, args: &[IrTy]) -> String {
        let mut sig = format!("{} (*)(Obj **", ret_cty(ret));
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
        let prog = self.prog;
        match prog.desc(d) {
            TypeDesc::Unit => "sb_puts(&rt_sb, \"()\");".into(),
            TypeDesc::Num(NumTy::F64) => {
                format!("sb_float(&rt_sb, bits_f64({w}), 0);")
            }
            TypeDesc::Num(NumTy::F32) => {
                format!("sb_float(&rt_sb, (f64)bits_f32({w}), 1);")
            }
            TypeDesc::Num(n) if n.is_signed() => {
                format!("sb_i64(&rt_sb, (i64){w});")
            }
            TypeDesc::Num(_) => {
                format!("sb_u64(&rt_sb, {w});")
            }
            TypeDesc::Bool => {
                format!("sb_puts(&rt_sb, {w} ? \"true\" : \"false\");")
            }
            TypeDesc::Char if nested => {
                format!("sb_dbg_char(&rt_sb, (u32){w});")
            }
            TypeDesc::Char => {
                format!("sb_cp(&rt_sb, (u32){w});")
            }
            TypeDesc::Str if nested => {
                format!("sb_dbg_str(&rt_sb, {});", w2o(w))
            }
            TypeDesc::Str => {
                let o = w2o(w);
                format!("sb_put(&rt_sb, STR({o})->s, STR({o})->len);")
            }
            _ if crate::ir::packed_enum(&prog.descs, d).is_some() => format!("{}({w});", self.printer(d)),
            _ => {
                format!("{}({});", self.printer(d), w2o(w))
            }
        }
    }

    fn put_str(&mut self, text: &str) -> String {
        format!("sb_puts(&rt_sb, {});", c_string(text))
    }

    fn gen_printer(&mut self, d: usize) {
        let prog = self.prog;
        let name = format!("pr_{d}");
        let mut body = String::new();
        if let Some(pk) = crate::ir::packed_enum(&prog.descs, DescId(d as u32)) {
            self.gen_packed_printer(d, &pk);
            return;
        }
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
            TypeDesc::Enum { variants, .. }
                if crate::runtime::value::is_tuple_enum(
                    match prog.desc(DescId(d as u32)) {
                        TypeDesc::Enum { name, .. } => name,
                        _ => unreachable!(),
                    },
                    variants,
                ) =>
            {
                // a tuple prints as `(a, b)`
                let _ = writeln!(body, "    const Agg *g = AGG(o);");
                let _ = writeln!(body, "    {}", self.put_str("("));
                for (i, fd) in variants[0].1.iter().enumerate() {
                    if i > 0 {
                        let _ = writeln!(body, "    {}", self.put_str(", "));
                    }
                    let w = format!("g->f[{i}]");
                    let _ = writeln!(body, "    {}", self.put_word(*fd, &w, true));
                }
                let _ = writeln!(body, "    {}", self.put_str(")"));
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

    /// The printer of a packed enum: it takes the word itself, not an object.
    fn gen_packed_printer(&mut self, d: usize, pk: &crate::ir::PackedEnum) {
        use crate::ir::PackedKind;
        let prog = self.prog;
        let TypeDesc::Enum { name: ename, variants } = prog.desc(DescId(d as u32)) else { unreachable!() };
        let tuple = crate::runtime::value::is_tuple_enum(ename, variants);
        let mut body = String::from("    switch (w & 0xff) {\n");
        for (t, (vname, fds)) in variants.iter().enumerate() {
            let _ = writeln!(body, "    case {t}:");
            if tuple {
                let _ = writeln!(body, "        {}", self.put_str("("));
            } else {
                let _ = writeln!(body, "        {}", self.put_str(&format!("{ename}::{vname}")));
                if !fds.is_empty() {
                    let _ = writeln!(body, "        {}", self.put_str("("));
                }
            }
            for (i, fd) in fds.iter().enumerate() {
                if i > 0 {
                    let _ = writeln!(body, "        {}", self.put_str(", "));
                }
                let f = pk.variants[t][i];
                let at = if f.shift == 0 { "w".to_string() } else { format!("(w >> {})", f.shift) };
                let expr = match f.kind {
                    PackedKind::Unit => "0".to_string(),
                    PackedKind::Bool => format!("(W)(u8){at}"),
                    PackedKind::Char => format!("(W)(u32){at}"),
                    PackedKind::Int { signed: true } => format!("(W)(i64)(i{}){at}", f.bits),
                    PackedKind::Int { signed: false } => format!("(W)(u{}){at}", f.bits),
                };
                let _ = writeln!(body, "        {}", self.put_word(*fd, &expr, true));
            }
            if tuple || !fds.is_empty() {
                let _ = writeln!(body, "        {}", self.put_str(")"));
            }
            let _ = writeln!(body, "        break;");
        }
        let _ = writeln!(body, "    default:\n        break;\n    }}");
        let header = format!("static void pr_{d}(W w)");
        self.printers.insert(d, (format!("{header};"), format!("{header} {{\n{body}}}")));
    }

    // ---- driver -------------------------------------------------------------------------------------

    pub fn generate(mut self) -> Result<Output, String> {
        self.find_live();
        self.assign_names();
        self.find_guarded();
        self.build_layouts();
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
            if info.ty == IrTy::Ref {
                ref_globals.push(self.gnames[g].clone());
            }
        }
        let typedefs = self.fnptrs.iter().map(|(_, t)| t.clone()).collect();
        let out = Output {
            fn_protos,
            fn_bodies,
            typedefs,
            globals,
            strs: self.strs.clone(),
            ref_globals,
            enums: self.layout_decls(),
            entry: self.names[self.prog.entry.index()].clone(),
        };
        Ok(out)
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
        let ret = ret_cty(f.ret);
        let sep = if ret.ends_with('*') { "" } else { " " };
        let header = format!("static {ret}{sep}{name}_clo({})", params.join(", "));
        let call = if f.ret == IrTy::Unit { String::new() } else { "return ".to_string() };
        let body = format!("{header} {{\n    (void)up;\n    {call}{name}({});\n}}", args.join(", "));
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

        // names: the source's where the debug information has them, `t<n>` for temporaries
        let mut taken = self.taken.clone();
        let mut vn: Vec<String> = vec![String::new(); nregs];
        let mut cn: Vec<String> = vec![String::new(); nregs];
        // only registers that become variables get a name (a folded constant does not)
        let alive: Vec<bool> = (0..nregs)
            .map(|i| (used[i] || i < params || (pinned[i] && defined[i])) && (i < params || consts[i].is_none()))
            .collect();
        for i in (0..nregs).filter(|&i| alive[i]) {
            if let Some(base) = f.reg_name(VReg(i as u32)).and_then(names::c_ident) {
                vn[i] = names::unique(&base, &mut taken);
                if pinned[i] {
                    cn[i] = names::unique(&format!("{}_cell", vn[i]), &mut taken);
                }
            }
        }
        for i in (0..nregs).filter(|&i| alive[i]) {
            if vn[i].is_empty() {
                vn[i] = names::unique(&format!("t{i}"), &mut taken);
            }
            if pinned[i] && cn[i].is_empty() {
                cn[i] = names::unique(&format!("c{i}"), &mut taken);
            }
        }
        let lay = infer_layouts(prog, f);

        // signature
        let mut sig_params = Vec::new();
        if f.is_closure {
            sig_params.push("Obj **up".to_string());
        }
        for (pn, &ty) in vn.iter().zip(&f.regs).take(params) {
            sig_params.push(format!("{}{}{pn}", cty(ty), if ty == IrTy::Ref { "" } else { " " }));
        }
        if sig_params.is_empty() {
            sig_params.push("void".into());
        }
        let eff = self.fx.of(id);
        let attr = if f.is_closure || self.guarded[id.index()] || f.ret == IrTy::Unit {
            ""
        } else if eff == Effects::NONE {
            "CONST_FN "
        } else if eff == Effects::READS {
            "PURE_FN "
        } else {
            ""
        };
        let ret = ret_cty(f.ret);
        let sep = if ret.ends_with('*') { "" } else { " " };
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
            if pinned[i] {
                cells.push(i);
                roots.push(format!("&{}", cn[i]));
                continue;
            }
            if i < params {
                if f.regs[i] == IrTy::Ref {
                    roots.push(format!("&{}", vn[i]));
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
                ("Obj", format!("*{} = NULL", vn[i]))
            } else {
                (cty(f.regs[i]), format!("{} = 0", vn[i]))
            };
            locals.entry(t).or_default().push(init);
            if f.regs[i] == IrTy::Ref {
                roots.push(format!("&{}", vn[i]));
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
            vn: vn.clone(),
            cn: cn.clone(),
            lay,
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
        if let Some(loc) = f.loc.and_then(|l| prog.sources.get(l.file as usize).map(|src| (src, l))) {
            let (src, l) = loc;
            let _ = writeln!(
                text,
                "/* {} ({}:{}) */",
                f.name.replace("*/", "* /"),
                src.path.replace("*/", "* /"),
                src.line_of(l.offset)
            );
        }
        let _ = writeln!(text, "{header} {{");
        for (t, vars) in &locals {
            let _ = writeln!(text, "    {t} {};", vars.join(", "));
        }
        if !cells.is_empty() {
            let list: Vec<String> = cells.iter().map(|&i| format!("*{} = NULL", cn[i])).collect();
            let _ = writeln!(text, "    Obj {};", list.join(", "));
        }
        if need_frame {
            let _ = writeln!(text, "    FRAME({});", roots.join(", "));
        }
        if guarded {
            let _ = writeln!(text, "    DEPTH_ENTER();");
        }
        if f.is_closure && !uses_up {
            let _ = writeln!(text, "    (void)up;");
        }
        for (i, &is_pinned) in pinned.iter().enumerate().take(params) {
            if (!used[i] || f.regs[i] == IrTy::Unit) && !is_pinned {
                let _ = writeln!(text, "    (void){};", vn[i]);
            }
        }
        for &i in &cells {
            let ty = f.regs[i];
            let init = if i < params {
                let w = self.as_word(ty, &vn[i]);
                format!("cell_new({}, {w})", (ty == IrTy::Ref) as u8)
            } else {
                format!("cell_new({}, 0)", (ty == IrTy::Ref) as u8)
            };
            let _ = writeln!(text, "    {} = {init};", cn[i]);
        }
        // a blank line after a jump and before a label; a jump right before a label gets just one
        let is_label = |l: &str| l.ends_with(':') && l.starts_with('B') && !l.contains(' ');
        let is_jump = |l: &str| l.contains("goto B");
        let mut prev: Option<&String> = None;
        for l in &body_lines {
            let after_jump = prev.is_some_and(|p| is_jump(p) && !is_jump(l));
            let before_label = is_label(l) && prev.is_some_and(|p| !is_label(p));
            if after_jump || before_label {
                text.push('\n');
            }
            if is_label(l) {
                let _ = writeln!(text, "{l}");
            } else {
                let _ = writeln!(text, "    {l}");
            }
            prev = Some(l);
        }
        text.push('}');
        Ok((proto, text))
    }
}

/// The struct or enum descriptor each register points to, as far as the program says: what lowering
/// recorded for variables, the descriptors of `new_struct` / `new_enum`, copies, and the fields of structs.
fn infer_layouts(prog: &IrProgram, f: &IrFunc) -> Vec<Option<usize>> {
    let mut lay: Vec<Option<usize>> = f.dbg.iter().map(|d| d.desc.map(|x| x.index())).collect();
    lay.resize(f.regs.len(), None);
    let is_adt = |d: DescId| matches!(prog.desc(d), TypeDesc::Struct { .. } | TypeDesc::Enum { .. });
    for _ in 0..4 {
        let mut changed = false;
        let mut set = |lay: &mut Vec<Option<usize>>, r: VReg, d: Option<usize>| {
            if lay[r.index()].is_none() && d.is_some() {
                lay[r.index()] = d;
                changed = true;
            }
        };
        for b in &f.blocks {
            for inst in &b.insts {
                match inst {
                    Inst::NewStruct { dst, desc, .. } | Inst::NewEnum { dst, desc, .. } => {
                        set(&mut lay, *dst, Some(desc.index()))
                    }
                    Inst::Move { dst, src } => {
                        let (a, c) = (lay[dst.index()], lay[src.index()]);
                        set(&mut lay, *dst, c);
                        set(&mut lay, *src, a);
                    }
                    Inst::GetField { dst, obj, idx } => {
                        if let Some(o) = lay[obj.index()] {
                            if let TypeDesc::Struct { fields, .. } = prog.desc(DescId(o as u32)) {
                                if let Some(&(_, fd)) = fields.get(*idx as usize) {
                                    if is_adt(fd) {
                                        set(&mut lay, *dst, Some(fd.index()));
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if !changed {
            break;
        }
    }
    lay
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
    /// The C names of the registers' values and of the cells of captured registers.
    vn: Vec<String>,
    cn: Vec<String>,
    /// The struct or enum layout (descriptor) a register points to, where known.
    lay: Vec<Option<usize>>,
}

impl FnEmit<'_, '_> {
    fn line(&mut self, s: impl Into<String>) {
        self.out.push(s.into());
    }

    fn ty(&self, r: VReg) -> IrTy {
        self.f.regs[r.index()]
    }

    /// The enumerator for field `idx` of the struct `obj` points to, or the plain number.
    fn field_ref(&mut self, obj: VReg, idx: u32) -> String {
        let l = self.lay[obj.index()].and_then(|d| self.g.layouts.get(&d));
        match l {
            Some(l) if !l.is_enum && (idx as usize) < l.items.len() => {
                let (owner, name) = (l.owner, l.items[idx as usize].clone());
                self.g.layouts_used.insert(owner);
                name
            }
            _ => idx.to_string(),
        }
    }

    /// The enumerator for variant `tag` of the enum with descriptor `desc` (or of the enum `obj` points to).
    fn tag_ref(&mut self, desc: Option<usize>, tag: u32) -> String {
        let l = desc.and_then(|d| self.g.layouts.get(&d));
        match l {
            Some(l) if l.is_enum && (tag as usize) < l.items.len() => {
                let (owner, name) = (l.owner, l.items[tag as usize].clone());
                self.g.layouts_used.insert(owner);
                name
            }
            _ => tag.to_string(),
        }
    }

    fn rd(&mut self, r: VReg) -> String {
        if self.ty(r) == IrTy::Unit && !self.pinned[r.index()] {
            return "0".into();
        }
        if let Some(bits) = self.consts[r.index()] {
            let l = lit(self.ty(r), bits);
            return l;
        }
        if self.pinned[r.index()] {
            let ty = self.ty(r);
            let c = self.cn[r.index()].clone();
            self.g.of_word(ty, &format!("CELL({c})->v"))
        } else {
            self.vn[r.index()].clone()
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
            let w = self.g.as_word(ty, expr);
            return format!("CELL({})->v = {w};", self.cn[dst.index()]);
        }
        if !self.used[dst.index()] || self.ty(dst) == IrTy::Unit {
            // nothing reads the result: keep the call, drop plain expressions
            // (a plain variable is still "used": its own assignment would otherwise warn)
            if self.ty(dst) == IrTy::Unit && expr.contains('(') {
                return format!("{expr};");
            }
            let plain_variable = expr.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
                && expr.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
            return if expr.contains('(') || plain_variable {
                format!("(void){};", paren(expr))
            } else {
                String::new()
            };
        }
        format!("{} = {expr};", self.vn[dst.index()])
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
        self.line("GC_POINT();");
    }

    fn epilogue(&mut self) {
        if self.guarded {
            self.line("DEPTH_LEAVE();");
        }
        if self.frame {
            self.line("FRAME_LEAVE();");
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
            let v = self.vn[dst.index()].clone();
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
                let t = self.tag_ref(self.lay[obj.index()], *tag);
                self.branch(format!("TAG({o}) == {t}"), *then, *els, next);
            }
            Term::Return(r) => {
                let unit = self.f.ret == IrTy::Unit;
                let e = if unit { String::new() } else { format!(" {}", self.rd(*r)) };
                self.epilogue();
                self.line(format!("return{e};"));
            }
            Term::Unreachable => {
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
            self.line(format!("if ({}) goto B{};", negate(&cond), els.index()));
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
                            format!("{n}({a}, (i64){b})")
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
                            format!("str_eq({a}, {b})")
                        }
                        CmpOp::Ne => {
                            format!("!str_eq({a}, {b})")
                        }
                        _ => {
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

            Inst::NewStruct { dst, desc, fields } => {
                self.alloc_agg(*dst, Some(desc.index()), "0".into(), fields)?;
            }
            Inst::NewEnum { dst, desc, tag, fields } => {
                let t = self.tag_ref(Some(desc.index()), *tag);
                self.alloc_agg(*dst, None, t, fields)?;
            }
            Inst::GetField { dst, obj, idx } => {
                let o = self.rd(*obj);
                let fld = self.field_ref(*obj, *idx);
                self.set_word(*dst, &format!("AGG({o})->f[{fld}]"));
            }
            Inst::EnumField { dst, obj, idx, .. } => {
                let o = self.rd(*obj);
                self.set_word(*dst, &format!("AGG({o})->f[{idx}]"));
            }
            Inst::SetField { obj, idx, src } => {
                let o = self.rd(*obj);
                let w = self.w(*src);
                let fld = self.field_ref(*obj, *idx);
                self.line(format!("AGG({o})->f[{fld}] = {w};"));
            }
            Inst::EnumTag { dst, obj } => {
                let o = self.rd(*obj);
                let c = cty(self.ty(*dst));
                self.emit_set(*dst, &format!("({c})TAG({o})"));
            }
            Inst::NewArray { dst, desc, elems } => {
                let TypeDesc::Array(e) = prog.desc(*desc) else {
                    return Err("NewArray with a non-array descriptor".into());
                };
                let is_ref = crate::ir::desc_is_ref(&prog.descs, *e) as u8;
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
                let (a, i) = (self.rd(*arr), self.rd(*idx));
                self.set_word(*dst, &format!("arr_get({a}, (i64){i})"));
            }
            Inst::ArraySet { arr, idx, src, .. } => {
                let (a, i, w) = (self.rd(*arr), self.rd(*idx), self.w(*src));
                self.line(format!("arr_set({a}, (i64){i}, {w});"));
            }
            Inst::NewMap { dst, desc, entries } => {
                let TypeDesc::Map(k, v) = prog.desc(*desc) else {
                    return Err("NewMap with a non-map descriptor".into());
                };
                let flags = map_flags(prog, *k, Some(*v));
                let ws: Vec<(String, String)> = entries.iter().map(|&(k, v)| (self.w(k), self.w(v))).collect();
                self.with_obj(*dst, &format!("map_new({flags})"), |me, m| {
                    for (k, v) in &ws {
                        me.line(format!("map_put({m}, {k}, {v});"));
                    }
                });
            }
            Inst::MapGet { dst, map, key } => {
                let (m, k) = (self.rd(*map), self.w(*key));
                self.set_word(*dst, &format!("map_get({m}, {k})"));
            }
            Inst::MapSet { map, key, val } => {
                let (m, k, v) = (self.rd(*map), self.w(*key), self.w(*val));
                self.line(format!("map_put({m}, {k}, {v});"));
            }
            Inst::NewSet { dst, desc, elems } => {
                let TypeDesc::Set(e) = prog.desc(*desc) else { return Err("NewSet with a non-set descriptor".into()) };
                let flags = map_flags(prog, *e, None);
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
                let c = self.rd(*callee);
                let tys: Vec<IrTy> = args.iter().map(|&r| self.ty(r)).collect();
                let fp = self.g.fnptr(self.ty(*dst), &tys);
                let a: Vec<String> = args.iter().map(|&r| self.rd(r)).collect();
                let mut call_args = vec![format!("CLO({c})->c")];
                call_args.extend(a);
                self.emit_set(*dst, &format!("(({fp})CLO({c})->fn)({})", call_args.join(", ")));
            }
            Inst::MakeClosure { dst, func, captures } => {
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
                            self.cn[r.index()].clone()
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
                self.set_word(*dst, &format!("CELL(up[{idx}])->v"));
            }
            Inst::SetUpval { idx, src } => {
                let w = self.w(*src);
                self.line(format!("CELL(up[{idx}])->v = {w};"));
            }
            Inst::CloseUpvals { regs } => {
                for r in regs {
                    if !self.pinned[r.index()] {
                        continue;
                    }
                    let is_ref = (self.ty(*r) == IrTy::Ref) as u8;
                    // closures keep the old cell; the function continues with a fresh one
                    let c = self.cn[r.index()].clone();
                    self.line(format!("{c} = cell_new({is_ref}, CELL({c})->v);"));
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

    /// Allocates a struct (`layout`: its descriptor, whose field names the stores use) or an enum variant.
    fn alloc_agg(&mut self, dst: VReg, layout: Option<usize>, tag: String, fields: &[VReg]) -> Result<(), String> {
        let mask = self.mask_of(fields)?;
        let ws: Vec<String> = fields.iter().map(|&r| self.w(r)).collect();
        let names: Vec<String> = (0..fields.len())
            .map(|i| match layout.and_then(|d| self.g.layouts.get(&d)) {
                Some(l) if !l.is_enum && i < l.items.len() => {
                    self.g.layouts_used.insert(l.owner);
                    l.items[i].clone()
                }
                _ => i.to_string(),
            })
            .collect();
        let alloc = format!("agg_new({tag}, {}, {mask})", fields.len());
        self.with_obj(dst, &alloc, |me, v| {
            for (n, w) in names.iter().zip(&ws) {
                me.line(format!("AGG({v})->f[{n}] = {w};"));
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
        // top-level strings and characters print raw
        match self.g.prog.desc(d) {
            TypeDesc::Num(NumTy::F64) => {
                format!("sb_float(&rt_sb, {e}, 0);")
            }
            TypeDesc::Num(NumTy::F32) => {
                format!("sb_float(&rt_sb, (f64){e}, 1);")
            }
            TypeDesc::Num(n) if n.is_signed() => {
                format!("sb_i64(&rt_sb, {e});")
            }
            TypeDesc::Num(_) => {
                format!("sb_u64(&rt_sb, {e});")
            }
            TypeDesc::Bool => {
                format!("sb_puts(&rt_sb, {e} ? \"true\" : \"false\");")
            }
            TypeDesc::Char => {
                format!("sb_cp(&rt_sb, {e});")
            }
            TypeDesc::Str => {
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
        // these allocate through runtime functions that open with `GC_POINT()` themselves
        let collects_inside = matches!(
            b,
            StrConcat | StrChars | StrToUpper | StrToLower | StrTrim | StrSplit | ArrJoin | MapKeys | SetToArray
        );
        if b.effects().allocates() && !collects_inside {
            self.gc_point();
        }
        let a: Vec<String> = args.iter().map(|&r| self.rd(r)).collect();
        let aw = |me: &mut Self, i: usize| me.w(args[i]);
        let dst_ty = self.ty(dst);
        let int64 = |e: &str| format!("(i64){}", paren(e));
        macro_rules! call {
            ($piece:expr, $e:expr) => {{
                let e: String = $e;
                self.emit_set(dst, &e);
            }};
        }
        macro_rules! stmt {
            ($piece:expr, $s:expr) => {{
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
                match (args.first(), desc) {
                    (Some(&r), Some(d)) => {
                        if matches!(self.g.prog.desc(d), TypeDesc::Str) {
                            let s = self.rd(r);
                            self.line(format!("out_str({s}, {nl});"));
                        } else {
                            let put = self.put_reg(d, r);
                            self.line(put);
                            self.line(format!("out_sb({nl});"));
                        }
                    }
                    _ => {
                        if nl == 1 {
                            self.line("out_nl();");
                        }
                    }
                }
                unit_result(self);
            }
            Input => call!("rt_input", "rt_input()".into()),
            ReadLine => call!("rt_read_line", "rt_read_line()".into()),
            Args => call!("rt_args", "rt_args()".into()),
            SysExit => {
                self.line(format!("rt_sys_exit({});", a[0]));
                unit_result(self);
            }
            SysEnv => call!("rt_sys_env", format!("rt_sys_env({})", a[0])),
            SysPlatform => call!("rt_sys_platform", "rt_sys_platform()".into()),
            SysNowMs => call!("rt_sys_now_ms", "rt_sys_now_ms()".into()),
            SysTicksMs => call!("rt_sys_ticks_ms", "rt_sys_ticks_ms()".into()),
            SysSleep => {
                self.line(format!("rt_sys_sleep({});", a[0]));
                unit_result(self);
            }
            FsRead => call!("rt_fs_read", format!("rt_fs_read({})", a[0])),
            FsWrite => call!("rt_fs_write", format!("rt_fs_write({}, {}, 0)", a[0], a[1])),
            FsAppend => call!("rt_fs_write", format!("rt_fs_write({}, {}, 1)", a[0], a[1])),
            FsExists => call!("rt_fs_exists", format!("rt_fs_exists({})", a[0])),
            FsIsFile => call!("rt_fs_is_file", format!("rt_fs_is_file({})", a[0])),
            FsIsDir => call!("rt_fs_is_dir", format!("rt_fs_is_dir({})", a[0])),
            FsList => call!("rt_fs_list", format!("rt_fs_list({})", a[0])),
            FsCreateDir => call!("rt_fs_create_dir", format!("rt_fs_create_dir({})", a[0])),
            FsRemove => call!("rt_fs_remove", format!("rt_fs_remove({})", a[0])),
            FsRename => call!("rt_fs_rename", format!("rt_fs_rename({}, {})", a[0], a[1])),
            TermRaw => call!("rt_term_raw", format!("rt_term_raw({})", a[0])),
            TermReadKey => call!("rt_term_key", "rt_term_key(-1, 0)".into()),
            TermPollKey => call!("rt_term_key", format!("rt_term_key({}, 1)", a[0])),
            TermWidth => call!("rt_term_width", "rt_term_width()".into()),
            TermHeight => call!("rt_term_height", "rt_term_height()".into()),
            TermIsTty => call!("rt_term_is_tty", "rt_term_is_tty()".into()),
            Panic => {
                self.line(format!("trap_panic({});", a[0]));
                unit_result(self);
            }
            Assert => {
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
                call!("fmt_value", format!("fmt_value({kind}, {w}, {})", a[1]));
            }

            StrConcat => call!("str_cat", format!("str_cat({}, {})", a[0], a[1])),
            StrLen => call!("str_len", format!("str_len({})", a[0])),
            StrIsEmpty => {
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
                result_cast!(format!("ARR({})->len", a[0]));
            }
            ArrIsEmpty => {
                self.emit_set(dst, &format!("ARR({})->len == 0", a[0]));
            }
            ArrPush => {
                let w = aw(self, 1);
                stmt!("arr_push", format!("arr_push({}, {w});", a[0]));
                unit_result(self);
            }
            ArrPop => {
                self.set_word(dst, &format!("arr_pop({})", a[0]));
            }
            ArrClear => {
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
                result_cast!(format!("MAP({})->len", a[0]));
            }
            MapIsEmpty | SetIsEmpty => {
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
                let chk = self.g.chk_fn(b, ty);
                let rhs = match b {
                    CheckedNeg => String::new(),
                    CheckedShl | CheckedShr => format!(", {}", int64(&a[1])),
                    CheckedPow => format!(", (u64){}", paren(&a[1])),
                    _ => format!(", {}", a[1]),
                };
                let c = num_cty(ty);
                let w = self.g.as_word(IrTy::Num(ty), "t");
                self.line("{");
                self.line(format!("    {c} t;"));
                let target = if self.pinned[dst.index()] || !self.used[dst.index()] {
                    "o".to_string()
                } else {
                    self.vn[dst.index()].clone()
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
                    format!("{}({}, {})", if n == NumTy::F32 { "powf" } else { "pow" }, a[0], a[1])
                } else {
                    let f = self.g.pow_fn(n);
                    format!("{f}({}, (u64){})", a[0], paren(&a[1]))
                };
                self.emit_set(dst, &e);
            }
            Min | Max => {
                let n = self.num_of(desc)?;
                let (x, y) = (&a[0], &a[1]);
                let want_max = b == Max;
                let pick = if want_max { format!("{x} < {y} ? {y} : {x}") } else { format!("{x} < {y} ? {x} : {y}") };
                let e = if n.is_float() { format!("isnan({x}) ? {y} : isnan({y}) ? {x} : ({pick})") } else { pick };
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
                let f = if n == NumTy::F32 { format!("{base}f") } else { base.to_string() };
                self.emit_set(dst, &format!("{f}({})", a[0]));
            }
            IsNan => {
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
    if crate::ir::desc_is_ref(&prog.descs, k) {
        flags.push("MF_KREF");
    }
    if matches!(prog.desc(k), TypeDesc::Str) {
        flags.push("MF_KSTR");
    }
    match v {
        Some(v) => {
            if crate::ir::desc_is_ref(&prog.descs, v) {
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
