//! Heap-independent value operations: formatting, equality and ordering.
//!
//! Engines implement [`HeapView`] to expose their object representation; the
//! operations here are then identical for the IR interpreter and the VM.

use std::cmp::Ordering;
use std::fmt::Write;

use super::format::{format_bool, format_char, format_num};
use super::numeric::{compare, CmpOp};
use crate::ir::{DescId, TypeDesc};

/// Read-only access to heap objects by handle.
pub trait HeapView {
    fn str_of(&self, h: u64) -> &str;
    fn array_of(&self, h: u64) -> &[u64];
    fn map_entries(&self, h: u64) -> Vec<(u64, u64)>;
    fn set_elems(&self, h: u64) -> Vec<u64>;
    fn struct_fields(&self, h: u64) -> &[u64];
    /// The variant number and payload of an enum value.
    fn enum_parts(&self, h: u64) -> (u32, &[u64]);
}

/// Appends the text form of `bits` (a value of type `d`). Top-level strings
/// and characters print raw; inside collections they are quoted.
pub fn format_value(view: &dyn HeapView, descs: &[TypeDesc], d: DescId, bits: u64, nested: bool, out: &mut String) {
    match &descs[d.index()] {
        TypeDesc::Unit => out.push_str("()"),
        TypeDesc::Num(n) => out.push_str(&format_num(*n, bits)),
        TypeDesc::Bool => out.push_str(format_bool(bits)),
        TypeDesc::Char => {
            if nested {
                let _ = write!(out, "{:?}", char::from_u32(bits as u32).unwrap_or('\u{fffd}'));
            } else {
                out.push_str(&format_char(bits));
            }
        }
        TypeDesc::Str => {
            if nested {
                let _ = write!(out, "{:?}", view.str_of(bits));
            } else {
                out.push_str(view.str_of(bits));
            }
        }
        TypeDesc::Array(e) => {
            out.push('[');
            for (i, &v) in view.array_of(bits).iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_value(view, descs, *e, v, true, out);
            }
            out.push(']');
        }
        TypeDesc::Set(e) => {
            out.push_str("set{");
            for (i, v) in view.set_elems(bits).into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_value(view, descs, *e, v, true, out);
            }
            out.push('}');
        }
        TypeDesc::Map(k, v) => {
            out.push('{');
            for (i, (key, val)) in view.map_entries(bits).into_iter().enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                format_value(view, descs, *k, key, true, out);
                out.push_str(": ");
                format_value(view, descs, *v, val, true, out);
            }
            out.push('}');
        }
        TypeDesc::Struct { name, fields } => {
            out.push_str(name);
            if fields.is_empty() {
                return;
            }
            out.push_str(" { ");
            for (i, ((fname, fd), &v)) in fields.iter().zip(view.struct_fields(bits)).enumerate() {
                if i > 0 {
                    out.push_str(", ");
                }
                out.push_str(fname);
                out.push_str(": ");
                format_value(view, descs, *fd, v, true, out);
            }
            out.push_str(" }");
        }
        TypeDesc::Enum { name, variants } => {
            let (tag, payload) = view.enum_parts(bits);
            let (vname, ftys) = &variants[tag as usize];
            let _ = write!(out, "{name}::{vname}");
            if !ftys.is_empty() {
                out.push('(');
                for (i, (&fd, &v)) in ftys.iter().zip(payload).enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    format_value(view, descs, fd, v, true, out);
                }
                out.push(')');
            }
        }
        TypeDesc::Fn => out.push_str("<fn>"),
    }
}

/// `a == b` for values of a primitive or string type (identity otherwise).
pub fn value_eq(view: &dyn HeapView, descs: &[TypeDesc], d: DescId, a: u64, b: u64) -> bool {
    match &descs[d.index()] {
        TypeDesc::Num(n) => compare(CmpOp::Eq, *n, a, b),
        TypeDesc::Str => view.str_of(a) == view.str_of(b),
        _ => a == b,
    }
}

/// Ordering of two values of a number, bool, char or string type
/// (`None` for unordered floats).
pub fn value_cmp(view: &dyn HeapView, descs: &[TypeDesc], d: DescId, a: u64, b: u64) -> Option<Ordering> {
    match &descs[d.index()] {
        TypeDesc::Num(n) => {
            if compare(CmpOp::Lt, *n, a, b) {
                Some(Ordering::Less)
            } else if compare(CmpOp::Gt, *n, a, b) {
                Some(Ordering::Greater)
            } else if compare(CmpOp::Eq, *n, a, b) {
                Some(Ordering::Equal)
            } else {
                None
            }
        }
        TypeDesc::Str => Some(view.str_of(a).cmp(view.str_of(b))),
        _ => Some(a.cmp(&b)),
    }
}
