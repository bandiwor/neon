//! Builtin (intrinsic) operations. These mirror the reference interpreter's
//! implementations; the differential tests keep the two in lockstep.

use super::Vm;
use crate::builtin::Builtin;
use crate::ir::TypeDesc;
use crate::runtime::numeric::NumTy;
use crate::runtime::value::{option_payload_num, value_cmp, value_eq, ERR_TAG, NONE_TAG, OK_TAG, SOME_TAG};
use crate::runtime::{mathfn, HeapView, Trap};

impl Vm<'_, '_> {
    fn num_ty(&self, desc: Option<u32>) -> NumTy {
        match desc.map(|d| self.desc(d)) {
            Some(TypeDesc::Num(n)) => *n,
            other => panic!("expected a numeric descriptor, got {other:?}"),
        }
    }

    /// The element type of the collection `desc` describes.
    fn elem_desc(&self, desc: Option<u32>) -> u32 {
        match desc.map(|d| self.desc(d)) {
            Some(TypeDesc::Array(e)) | Some(TypeDesc::Set(e)) => e.0,
            Some(TypeDesc::Map(k, _)) => k.0,
            other => panic!("expected a collection descriptor, got {other:?}"),
        }
    }

    fn some(&mut self, desc: Option<u32>, v: u64) -> Result<u64, Trap> {
        self.heap.alloc_enum(desc.expect("result descriptor"), SOME_TAG, [v])
    }

    fn none(&mut self, desc: Option<u32>) -> Result<u64, Trap> {
        self.heap.alloc_enum(desc.expect("result descriptor"), NONE_TAG, [])
    }

    fn ok(&mut self, desc: Option<u32>, v: u64) -> Result<u64, Trap> {
        self.heap.alloc_enum(desc.expect("result descriptor"), OK_TAG, [v])
    }

    fn err_str(&mut self, desc: Option<u32>, message: String) -> Result<u64, Trap> {
        let m = self.heap.alloc_str(message)?;
        self.heap.alloc_enum(desc.expect("result descriptor"), ERR_TAG, [m])
    }

    /// `Some(x)` / `None` from an optional value.
    fn option(&mut self, desc: Option<u32>, v: Option<u64>) -> Result<u64, Trap> {
        match v {
            Some(v) => self.some(desc, v),
            None => self.none(desc),
        }
    }

    fn array_refs_checked(&self, h: u64) -> bool {
        self.heap.array_refs(h)
    }

    pub(super) fn intrinsic(&mut self, b: Builtin, desc: Option<u32>, a: &[u64]) -> Result<u64, Trap> {
        use Builtin::*;
        let h = &self.heap;
        Ok(match b {
            Print | Println => {
                let text = a.first().map(|&v| self.format(desc.expect("print has a descriptor"), v));
                match (b, text) {
                    (Print, Some(t)) => self.io.print(&t),
                    (Println, Some(t)) => self.io.println(&t),
                    (Println, None) => self.io.println(""),
                    _ => {}
                }
                0
            }
            Input => {
                let line = self.io.input().map_err(Trap::Io)?.unwrap_or_default();
                self.heap.alloc_str(line)?
            }
            Panic => return Err(Trap::Panic(h.str_of(a[0]).to_string())),
            Assert => {
                if a[0] == 0 {
                    let msg = a.get(1).map(|&m| format!(": {}", h.str_of(m))).unwrap_or_default();
                    return Err(Trap::Panic(format!("assertion failed{msg}")));
                }
                0
            }
            ToString => {
                let s = self.format(desc.expect("to_string has a descriptor"), a[0]);
                self.heap.alloc_str(s)?
            }
            Format => {
                let d = desc.expect("format has a descriptor");
                let kind = crate::runtime::fmtspec::kind_of(self.desc(d))
                    .ok_or_else(|| Trap::Panic("this type cannot take a format specifier".into()))?;
                let text = (kind == crate::runtime::fmtspec::Kind::Str).then(|| h.str_of(a[0]).to_string());
                let s = crate::runtime::fmtspec::format_with(kind, a[0], text.as_deref(), h.str_of(a[1]))
                    .map_err(|e| Trap::Panic(format!("invalid format specifier: {e}")))?;
                self.heap.alloc_str(s)?
            }

            StrConcat => {
                let s = format!("{}{}", h.str_of(a[0]), h.str_of(a[1]));
                self.heap.alloc_str(s)?
            }
            StrLen => h.str_of(a[0]).chars().count() as u64,
            StrIsEmpty => h.str_of(a[0]).is_empty() as u64,
            StrChars => {
                let v: Vec<u64> = h.str_of(a[0]).chars().map(|c| c as u64).collect();
                self.heap.alloc_array(v, false)?
            }
            StrSubstring => {
                let s = h.str_of(a[0]);
                let n = s.chars().count();
                let (lo, hi) = (a[1] as i64, a[2] as i64);
                if lo < 0 || hi < lo || hi as usize > n {
                    return Err(Trap::IndexOutOfBounds {
                        index: if lo < 0 || hi < lo { lo as i128 } else { hi as i128 },
                        len: n,
                    });
                }
                let sub: String = s.chars().skip(lo as usize).take((hi - lo) as usize).collect();
                self.heap.alloc_str(sub)?
            }
            StrContains => h.str_of(a[0]).contains(h.str_of(a[1])) as u64,
            StrStartsWith => h.str_of(a[0]).starts_with(h.str_of(a[1])) as u64,
            StrEndsWith => h.str_of(a[0]).ends_with(h.str_of(a[1])) as u64,
            StrToUpper => {
                let s = h.str_of(a[0]).to_uppercase();
                self.heap.alloc_str(s)?
            }
            StrToLower => {
                let s = h.str_of(a[0]).to_lowercase();
                self.heap.alloc_str(s)?
            }
            StrTrim => {
                let s = h.str_of(a[0]).trim().to_string();
                self.heap.alloc_str(s)?
            }
            StrSplit => {
                let parts: Vec<String> = {
                    let (s, sep) = (h.str_of(a[0]), h.str_of(a[1]));
                    if sep.is_empty() {
                        s.chars().map(String::from).collect()
                    } else {
                        s.split(sep).map(String::from).collect()
                    }
                };
                // nothing collects in the middle of an instruction, so the parts need no rooting
                let mut handles = Vec::with_capacity(parts.len());
                for p in parts {
                    handles.push(self.heap.alloc_str(p)?);
                }
                self.heap.alloc_array(handles, true)?
            }
            StrCharAt => {
                let s = h.str_of(a[0]);
                let i = a[1] as i64;
                match (i >= 0).then(|| s.chars().nth(i as usize)).flatten() {
                    Some(c) => c as u64,
                    None => return Err(Trap::IndexOutOfBounds { index: i as i128, len: s.chars().count() }),
                }
            }
            StrFind => {
                let (s, sub) = (h.str_of(a[0]), h.str_of(a[1]));
                match s.find(sub) {
                    Some(byte) => s[..byte].chars().count() as u64,
                    None => (-1i64) as u64,
                }
            }
            StrParseI64 => mathfn::parse_i64(h.str_of(a[0]))?,
            StrParseU64 => mathfn::parse_u64(h.str_of(a[0]))?,
            StrParseF64 => mathfn::parse_f64(h.str_of(a[0]))?,

            ArrLen => h.array_of(a[0]).len() as u64,
            ArrIsEmpty => h.array_of(a[0]).is_empty() as u64,
            ArrPush => {
                let v = a[1];
                self.heap.with_array(a[0], |arr| arr.push(v))?;
                0
            }
            ArrPop => self.heap.with_array(a[0], |arr| arr.pop())?.ok_or(Trap::EmptyArray)?,
            ArrClear => {
                self.heap.with_array(a[0], |arr| arr.clear())?;
                0
            }
            ArrContains => {
                let e = crate::ir::DescId(self.elem_desc(desc));
                h.array_of(a[0]).iter().any(|&x| value_eq(h, &self.prog.descs, e, x, a[1])) as u64
            }
            ArrInsert => {
                let len = h.array_of(a[0]).len();
                let i = a[1] as i64;
                if i < 0 || i as usize > len {
                    return Err(Trap::IndexOutOfBounds { index: i as i128, len });
                }
                let v = a[2];
                self.heap.with_array(a[0], |arr| arr.insert(i as usize, v))?;
                0
            }
            ArrRemove => {
                let len = h.array_of(a[0]).len();
                let i = a[1] as i64;
                if i < 0 || i as usize >= len {
                    return Err(Trap::IndexOutOfBounds { index: i as i128, len });
                }
                self.heap.with_array(a[0], |arr| arr.remove(i as usize))?
            }
            ArrReverse => {
                self.heap.with_array(a[0], |arr| arr.reverse())?;
                0
            }
            ArrClone => {
                let v = h.array_of(a[0]).to_vec();
                let refs = self.array_refs_checked(a[0]);
                self.heap.alloc_array(v, refs)?
            }
            ArrJoin => {
                let sep = h.str_of(a[1]).to_string();
                let joined = h.array_of(a[0]).iter().map(|&e| h.str_of(e).to_string()).collect::<Vec<_>>().join(&sep);
                self.heap.alloc_str(joined)?
            }
            ArrSort => {
                let e = crate::ir::DescId(self.elem_desc(desc));
                let mut v = h.array_of(a[0]).to_vec();
                // a stable sort with unordered (NaN) values last
                v.sort_by(|&x, &y| {
                    value_cmp(h, &self.prog.descs, e, x, y).unwrap_or_else(|| {
                        let nan_x = value_cmp(h, &self.prog.descs, e, x, x).is_none();
                        let nan_y = value_cmp(h, &self.prog.descs, e, y, y).is_none();
                        nan_x.cmp(&nan_y)
                    })
                });
                self.heap.with_array(a[0], |arr| *arr = v)?;
                0
            }

            MapLen | SetLen => h.map_len(a[0]) as u64,
            MapIsEmpty | SetIsEmpty => (h.map_len(a[0]) == 0) as u64,
            MapInsert => {
                self.heap.map_insert(a[0], a[1], a[2])?;
                0
            }
            MapGetOr => match h.map_find(a[0], a[1]) {
                Some(i) => h.map_val_at(a[0], i),
                None => a[2],
            },
            MapContainsKey | SetContains => h.map_find(a[0], a[1]).is_some() as u64,
            MapRemove | SetRemove => self.heap.map_remove(a[0], a[1])?.is_some() as u64,
            MapKeys | SetToArray => {
                let keys = h.map_live_entries(a[0]).map(|e| e.0).collect();
                let (key_ref, _) = h.map_flags(a[0]);
                self.heap.alloc_array(keys, key_ref)?
            }
            MapValues => {
                let vals = h.map_live_entries(a[0]).map(|e| e.1).collect();
                let (_, val_ref) = h.map_flags(a[0]);
                self.heap.alloc_array(vals, val_ref)?
            }
            MapClear | SetClear => {
                self.heap.map_clear(a[0])?;
                0
            }
            SetInsert => self.heap.map_insert(a[0], a[1], 0)? as u64,

            StrTryParseI64 | StrTryParseU64 | StrTryParseF64 => {
                let parsed = match b {
                    StrTryParseI64 => mathfn::parse_i64(h.str_of(a[0])),
                    StrTryParseU64 => mathfn::parse_u64(h.str_of(a[0])),
                    _ => mathfn::parse_f64(h.str_of(a[0])),
                };
                match parsed {
                    Ok(v) => self.ok(desc, v)?,
                    Err(e) => self.err_str(desc, e.to_string())?,
                }
            }
            StrGetChar => {
                let i = a[1] as i64;
                let c = (i >= 0).then(|| h.str_of(a[0]).chars().nth(i as usize)).flatten().map(|c| c as u64);
                self.option(desc, c)?
            }
            StrTrySubstring => {
                let s = h.str_of(a[0]);
                let n = s.chars().count();
                let (lo, hi) = (a[1] as i64, a[2] as i64);
                if lo < 0 || hi < lo || hi as usize > n {
                    let index = if lo < 0 || hi < lo { lo as i128 } else { hi as i128 };
                    self.err_str(desc, Trap::IndexOutOfBounds { index, len: n }.to_string())?
                } else {
                    let sub: String = s.chars().skip(lo as usize).take((hi - lo) as usize).collect();
                    let v = self.heap.alloc_str(sub)?;
                    self.ok(desc, v)?
                }
            }
            StrIndexOf => {
                let (s, sub) = (h.str_of(a[0]), h.str_of(a[1]));
                let at = s.find(sub).map(|byte| s[..byte].chars().count() as u64);
                self.option(desc, at)?
            }
            ArrGet => {
                let v = h.array_of(a[0]);
                let i = a[1] as i64;
                let e = (i >= 0).then(|| v.get(i as usize).copied()).flatten();
                self.option(desc, e)?
            }
            ArrTryPop => {
                let e = self.heap.with_array(a[0], |arr| arr.pop())?;
                self.option(desc, e)?
            }
            ArrTryRemove => {
                let len = h.array_of(a[0]).len();
                let i = a[1] as i64;
                let e = if i >= 0 && (i as usize) < len {
                    Some(self.heap.with_array(a[0], |arr| arr.remove(i as usize))?)
                } else {
                    None
                };
                self.option(desc, e)?
            }
            MapGet => {
                let e = h.map_find(a[0], a[1]).map(|i| h.map_val_at(a[0], i));
                self.option(desc, e)?
            }
            CheckedAdd | CheckedSub | CheckedMul | CheckedDiv | CheckedRem | CheckedPow | CheckedShl | CheckedShr => {
                let ty = option_payload_num(&self.prog.descs, crate::ir::DescId(desc.expect("result descriptor")));
                self.option(desc, mathfn::checked_op(b, ty, a[0], a[1]))?
            }
            CheckedNeg => {
                let ty = option_payload_num(&self.prog.descs, crate::ir::DescId(desc.expect("result descriptor")));
                self.option(desc, mathfn::checked_op(b, ty, a[0], 0))?
            }
            Args => {
                let mut handles = Vec::new();
                for a in self.io.args() {
                    handles.push(self.heap.alloc_str(a)?);
                }
                self.heap.alloc_array(handles, true)?
            }
            ReadLine => {
                let line = self.io.input().map_err(Trap::Io)?;
                match line {
                    Some(l) => {
                        let s = self.heap.alloc_str(l)?;
                        self.some(desc, s)?
                    }
                    None => self.none(desc)?,
                }
            }

            Abs => mathfn::abs(self.num_ty(desc), a[0])?,
            Pow => mathfn::pow(self.num_ty(desc), a[0], a[1])?,
            Min => mathfn::min_max(self.num_ty(desc), a[0], a[1], false),
            Max => mathfn::min_max(self.num_ty(desc), a[0], a[1], true),
            Sqrt | Sin | Cos | Tan | Floor | Ceil | Round | Ln | Exp => {
                mathfn::float_fn(b, self.num_ty(desc), a[0]).expect("float builtin")
            }
            IsNan => mathfn::is_nan(self.num_ty(desc), a[0]) as u64,

            CharIsDigit => char::from_u32(a[0] as u32).is_some_and(|c| c.is_ascii_digit()) as u64,
            CharIsAlphabetic => char::from_u32(a[0] as u32).is_some_and(char::is_alphabetic) as u64,
            CharIsWhitespace => char::from_u32(a[0] as u32).is_some_and(char::is_whitespace) as u64,
            CharToUpper => char::from_u32(a[0] as u32).and_then(|c| c.to_uppercase().next()).map_or(a[0], |c| c as u64),
            CharToLower => char::from_u32(a[0] as u32).and_then(|c| c.to_lowercase().next()).map_or(a[0], |c| c as u64),
        })
    }
}
