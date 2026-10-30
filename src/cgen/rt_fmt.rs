//! Runtime pieces, part 3: string builder, number and value formatting, Python-style format
//! specifications, parsing, character classes, the remaining string operations and I/O.

use super::rt::{Kind::*, Piece};

pub static PIECES: &[Piece] = &[
    // ---- string builder ---------------------------------------------------------------------------
    Piece {
        name: "sbtype",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"/* A growable text buffer. `rt_sb` is the scratch buffer shared by printing and formatting. */
typedef struct { char *p; size_t len, cap; } SB;
static SB rt_sb;"##,
    },
    Piece {
        name: "sb_put",
        deps: &["sbtype", "rt_realloc"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static void sb_put(SB *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        size_t cap = b->cap ? b->cap : 64;
        while (cap < b->len + n + 1) cap *= 2;
        b->p = (char *)rt_realloc(b->p, cap);
        b->cap = cap;
    }
    if (n) memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}"##,
    },
    Piece {
        name: "sb_puts",
        deps: &["sb_put"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static void sb_puts(SB *b, const char *s) {
    sb_put(b, s, strlen(s));
}"##,
    },
    Piece {
        name: "sb_putc",
        deps: &["sb_put"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void sb_putc(SB *b, char c) {
    sb_put(b, &c, 1);
}"##,
    },
    Piece {
        name: "sb_cp",
        deps: &["sb_put", "utf8_enc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Appends a code point as UTF-8. */
static void sb_cp(SB *b, uint32_t c) {
    char buf[4];
    sb_put(b, buf, utf8_enc(c, buf));
}"##,
    },
    Piece {
        name: "sb_take",
        deps: &["sbtype", "str_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* The scratch buffer's text as a string object; the buffer is emptied. */
static Obj *sb_take(void) {
    Obj *s = str_new(rt_sb.p ? rt_sb.p : "", rt_sb.len);
    rt_sb.len = 0;
    return s;
}"##,
    },
    Piece {
        name: "sb_i64",
        deps: &["sb_put"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static void sb_i64(SB *b, int64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
    sb_put(b, buf, (size_t)n);
}"##,
    },
    Piece {
        name: "sb_u64",
        deps: &["sb_put"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static void sb_u64(SB *b, uint64_t v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%llu", (unsigned long long)v);
    sb_put(b, buf, (size_t)n);
}"##,
    },
    Piece {
        name: "sb_float",
        deps: &["sb_put", "sb_putc", "sb_puts"],
        hdrs: &["math.h", "stdio.h", "stdlib.h"],
        kind: Funcs,
        src: r##"/* The shortest text that reads back as the same number, laid out like `1.0`, `0.1`, `1e16`, `NaN`, `inf`. */
static void sb_float(SB *b, double v, int single) {
    char buf[48], digs[24];
    int nd = 0, e10, prec, i;
    const char *q;
    if (isnan(v)) { sb_puts(b, "NaN"); return; }
    if (signbit(v)) sb_putc(b, '-');
    if (isinf(v)) { sb_puts(b, "inf"); return; }
    v = fabs(v);
    if (v == 0) { sb_puts(b, "0.0"); return; }
    for (prec = 1; prec <= (single ? 9 : 17); prec++) {
        snprintf(buf, sizeof buf, "%.*e", prec - 1, v);
        if (single ? (strtof(buf, NULL) == (float)v) : (strtod(buf, NULL) == v)) break;
    }
    for (q = buf; *q != 'e'; q++)
        if (*q != '.') digs[nd++] = *q;
    e10 = atoi(q + 1);
    if (v >= 1e16 || v < 1e-4) {
        sb_putc(b, digs[0]);
        if (nd > 1) { sb_putc(b, '.'); sb_put(b, digs + 1, (size_t)nd - 1); }
        snprintf(buf, sizeof buf, "e%d", e10);
        sb_puts(b, buf);
    } else if (e10 >= 0) {
        for (i = 0; i <= e10; i++) sb_putc(b, i < nd ? digs[i] : '0');
        sb_putc(b, '.');
        if (nd > e10 + 1) sb_put(b, digs + e10 + 1, (size_t)(nd - e10 - 1)); else sb_putc(b, '0');
    } else {
        sb_puts(b, "0.");
        for (i = 1; i < -e10; i++) sb_putc(b, '0');
        sb_put(b, digs, (size_t)nd);
    }
}"##,
    },
    Piece {
        name: "sb_escaped",
        deps: &["sb_put", "sb_puts", "sb_putc", "sb_cp"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* One character of a quoted string or character literal (`quote` is the delimiter being used). */
static void sb_escaped(SB *b, uint32_t c, uint32_t quote) {
    char buf[16];
    switch (c) {
    case '\n': sb_puts(b, "\\n"); return;
    case '\r': sb_puts(b, "\\r"); return;
    case '\t': sb_puts(b, "\\t"); return;
    case '\\': sb_puts(b, "\\\\"); return;
    case 0:    sb_puts(b, "\\0"); return;
    default: break;
    }
    if (c == quote) { sb_putc(b, '\\'); sb_putc(b, (char)c); return; }
    if (c < 0x20 || c == 0x7F || (c >= 0x80 && c < 0xA0)) {
        snprintf(buf, sizeof buf, "\\u{%x}", (unsigned)c);
        sb_puts(b, buf);
        return;
    }
    sb_cp(b, c);
}"##,
    },
    Piece {
        name: "sb_dbg_str",
        deps: &["sb_escaped", "utf8_dec", "sb_putc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* A string inside a collection: quoted and escaped. */
static void sb_dbg_str(SB *b, Obj *o) {
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    uint32_t c;
    sb_putc(b, '"');
    while (p < end) {
        p += utf8_dec(p, end, &c);
        sb_escaped(b, c, '"');
    }
    sb_putc(b, '"');
}"##,
    },
    Piece {
        name: "sb_dbg_char",
        deps: &["sb_escaped", "sb_putc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void sb_dbg_char(SB *b, uint32_t c) {
    sb_putc(b, '\'');
    sb_escaped(b, c, '\'');
    sb_putc(b, '\'');
}"##,
    },
    // ---- output and input -------------------------------------------------------------------------------
    Piece {
        name: "out_sb",
        deps: &["sbtype"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* Writes the scratch buffer to stdout (and a newline) and empties it. */
static void out_sb(int newline) {
    if (rt_sb.len) fwrite(rt_sb.p, 1, rt_sb.len, stdout);
    if (newline) putc('\n', stdout);
    rt_sb.len = 0;
}"##,
    },
    Piece {
        name: "out_str",
        deps: &["objects"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static void out_str(Obj *s, int newline) {
    if (STR(s)->len) fwrite(STR(s)->s, 1, STR(s)->len, stdout);
    if (newline) putc('\n', stdout);
}"##,
    },
    Piece {
        name: "out_nl",
        deps: &[],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static void out_nl(void) {
    putc('\n', stdout);
}"##,
    },
    Piece {
        name: "io_getline",
        deps: &["sb_putc"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* Reads a line into the scratch buffer without its line terminator; 0 at the end of the input. */
static int io_getline(void) {
    int c, any = 0;
    fflush(stdout);
    rt_sb.len = 0;
    while ((c = getc(stdin)) != EOF) {
        any = 1;
        if (c == '\n') break;
        sb_putc(&rt_sb, (char)c);
    }
    if (!any) return 0;
    while (rt_sb.len > 0 && rt_sb.p[rt_sb.len - 1] == '\r') rt_sb.len--;
    return 1;
}"##,
    },
    Piece {
        name: "rt_input",
        deps: &["io_getline", "sb_take"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* input(): the next line, or an empty string at the end of the input. */
static Obj *rt_input(void) {
    io_getline();
    return sb_take();
}"##,
    },
    Piece {
        name: "rt_read_line",
        deps: &["io_getline", "sb_take", "opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* read_line(): Some(line), or None at the end of the input. */
static Obj *rt_read_line(void) {
    if (!io_getline()) return opt_none();
    return opt_some(O2W(sb_take()), 1);
}"##,
    },
    // ---- character classes and case -------------------------------------------------------------
    Piece {
        name: "cp_is_digit",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int cp_is_digit(uint32_t c) {
    return c >= '0' && c <= '9';
}"##,
    },
    Piece {
        name: "cp_is_space",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Unicode White_Space. */
static CONST_FN int cp_is_space(uint32_t c) {
    return (c >= 9 && c <= 13) || c == 32 || c == 0x85 || c == 0xA0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200A) ||
           c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000;
}"##,
    },
    Piece {
        name: "cp_is_alpha",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Letters of the common scripts (Latin, Greek, Cyrillic, Armenian, Hebrew, Arabic, Thai, kana, CJK, Hangul). */
static CONST_FN int cp_is_alpha(uint32_t c) {
    if (c < 0x80) return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
    return c == 0xAA || c == 0xB5 || c == 0xBA || (c >= 0xC0 && c <= 0x2C1 && c != 0xD7 && c != 0xF7) ||
           (c >= 0x370 && c <= 0x373) || (c >= 0x376 && c <= 0x377) || (c >= 0x37B && c <= 0x37D) || c == 0x386 ||
           (c >= 0x388 && c <= 0x3FF && c != 0x3F6) || (c >= 0x400 && c <= 0x481) || (c >= 0x48A && c <= 0x52F) ||
           (c >= 0x531 && c <= 0x556) || (c >= 0x561 && c <= 0x587) || (c >= 0x5D0 && c <= 0x5EA) ||
           (c >= 0x620 && c <= 0x64A) || (c >= 0xE01 && c <= 0xE30) || (c >= 0x1E00 && c <= 0x1FFF) ||
           (c >= 0x3041 && c <= 0x3096) || (c >= 0x30A1 && c <= 0x30FA) || (c >= 0x3400 && c <= 0x4DBF) ||
           (c >= 0x4E00 && c <= 0x9FFF) || (c >= 0xAC00 && c <= 0xD7A3) || (c >= 0xFF21 && c <= 0xFF3A) ||
           (c >= 0xFF41 && c <= 0xFF5A);
}"##,
    },
    Piece {
        name: "cp_upper",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Upper case of a code point: ASCII, Latin-1, Latin Extended-A, Greek and Cyrillic. */
static CONST_FN uint32_t cp_upper(uint32_t c) {
    if (c < 0x80) return (c >= 'a' && c <= 'z') ? c - 32 : c;
    if (c == 0xB5) return 0x39C;
    if (c == 0xFF) return 0x178;
    if (c >= 0xE0 && c <= 0xFE && c != 0xF7) return c - 32;
    if ((c >= 0x100 && c <= 0x137) || (c >= 0x14A && c <= 0x177)) return (c & 1) ? c - 1 : c;
    if ((c >= 0x139 && c <= 0x148) || (c >= 0x179 && c <= 0x17E)) return (c & 1) ? c : c - 1;
    if (c == 0x3C2) return 0x3A3;
    if (c >= 0x3B1 && c <= 0x3CB) return c - 32;
    if (c == 0x3AC) return 0x386;
    if (c >= 0x3AD && c <= 0x3AF) return c - 37;
    if (c == 0x3CC) return 0x38C;
    if (c == 0x3CD || c == 0x3CE) return c - 63;
    if (c >= 0x430 && c <= 0x44F) return c - 32;
    if (c >= 0x450 && c <= 0x45F) return c - 80;
    if ((c >= 0x460 && c <= 0x481) || (c >= 0x48A && c <= 0x4BF) || (c >= 0x4D0 && c <= 0x52F)) return (c & 1) ? c - 1 : c;
    return c;
}"##,
    },
    Piece {
        name: "cp_lower",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN uint32_t cp_lower(uint32_t c) {
    if (c < 0x80) return (c >= 'A' && c <= 'Z') ? c + 32 : c;
    if (c == 0x178) return 0xFF;
    if (c >= 0xC0 && c <= 0xDE && c != 0xD7) return c + 32;
    if ((c >= 0x100 && c <= 0x137) || (c >= 0x14A && c <= 0x177)) return (c & 1) ? c : c + 1;
    if ((c >= 0x139 && c <= 0x148) || (c >= 0x179 && c <= 0x17E)) return (c & 1) ? c + 1 : c;
    if (c >= 0x391 && c <= 0x3AB && c != 0x3A2) return c + 32;
    if (c == 0x386) return 0x3AC;
    if (c >= 0x388 && c <= 0x38A) return c + 37;
    if (c == 0x38C) return 0x3CC;
    if (c == 0x38E || c == 0x38F) return c + 63;
    if (c >= 0x410 && c <= 0x42F) return c + 32;
    if (c >= 0x400 && c <= 0x40F) return c + 80;
    if ((c >= 0x460 && c <= 0x481) || (c >= 0x48A && c <= 0x4BF) || (c >= 0x4D0 && c <= 0x52F)) return (c & 1) ? c : c + 1;
    return c;
}"##,
    },
    // ---- more string operations --------------------------------------------------------------------
    Piece {
        name: "str_chars",
        deps: &["arr_new", "utf8_dec"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_chars(Obj *o) {
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    Obj *r = arr_new(STR(o)->len, 0);
    uint32_t c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        ARR(r)->a[ARR(r)->len++] = c;
    }
    return r;
}"##,
    },
    Piece {
        name: "str_upper",
        deps: &["sb_cp", "sb_puts", "sb_take", "utf8_dec", "cp_upper"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_upper(Obj *o) {
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    uint32_t c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        if (c == 0xDF) sb_puts(&rt_sb, "SS"); else sb_cp(&rt_sb, cp_upper(c));
    }
    return sb_take();
}"##,
    },
    Piece {
        name: "str_lower",
        deps: &["sb_cp", "sb_take", "utf8_dec", "cp_lower"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_lower(Obj *o) {
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    uint32_t c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        sb_cp(&rt_sb, cp_lower(c));
    }
    return sb_take();
}"##,
    },
    Piece {
        name: "trim_span",
        deps: &["utf8_dec", "cp_is_space"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* The part of the string without leading and trailing white space. */
static void trim_span(const Str *s, const char **b, const char **e) {
    const char *p = s->s, *end = s->s + s->len, *q;
    uint32_t c;
    size_t k;
    while (p < end) {
        k = utf8_dec(p, end, &c);
        if (!cp_is_space(c)) break;
        p += k;
    }
    while (end > p) {
        q = end - 1;
        while (q > p && ((unsigned char)*q & 0xC0) == 0x80) q--;
        utf8_dec(q, end, &c);
        if (!cp_is_space(c)) break;
        end = q;
    }
    *b = p;
    *e = end;
}"##,
    },
    Piece {
        name: "str_trim",
        deps: &["trim_span", "str_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_trim(Obj *o) {
    const char *b, *e;
    trim_span(STR(o), &b, &e);
    return str_new(b, (size_t)(e - b));
}"##,
    },
    Piece {
        name: "str_split",
        deps: &["arr_push", "str_new", "utf8_dec"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* Pieces between separators; an empty separator splits into characters. */
static Obj *str_split(Obj *o, Obj *sepo) {
    const Str *s = STR(o), *sep = STR(sepo);
    const char *p = s->s, *end = s->s + s->len, *q;
    Obj *r = arr_new(4, 1);
    uint32_t c;
    size_t k;
    if (sep->len == 0) {
        while (p < end) {
            k = utf8_dec(p, end, &c);
            arr_push(r, O2W(str_new(p, k)));
            p += k;
        }
        return r;
    }
    for (;;) {
        for (q = p; q + sep->len <= end; q++)
            if (memcmp(q, sep->s, sep->len) == 0) break;
        if (q + sep->len > end) {
            arr_push(r, O2W(str_new(p, (size_t)(end - p))));
            return r;
        }
        arr_push(r, O2W(str_new(p, (size_t)(q - p))));
        p = q + sep->len;
    }
}"##,
    },
    Piece {
        name: "str_join",
        deps: &["sb_take", "sb_put", "sbtype"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_join(Obj *arr, Obj *sep) {
    const Arr *a = ARR(arr);
    size_t i;
    for (i = 0; i < a->len; i++) {
        if (i) sb_put(&rt_sb, STR(sep)->s, STR(sep)->len);
        sb_put(&rt_sb, STR(W2O(a->a[i]))->s, STR(W2O(a->a[i]))->len);
    }
    return sb_take();
}"##,
    },
    // ---- parsing -------------------------------------------------------------------------------------
    Piece {
        name: "parse_i64_try",
        deps: &["trim_span"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Decimal i64 with an optional sign; surrounding white space is ignored. 1 on success. */
static int parse_i64_try(Obj *o, int64_t *out) {
    const char *b, *e;
    int neg = 0;
    uint64_t v = 0;
    trim_span(STR(o), &b, &e);
    if (b < e && (*b == '+' || *b == '-')) { neg = *b == '-'; b++; }
    if (b == e) return 0;
    for (; b < e; b++) {
        unsigned d = (unsigned char)*b - '0';
        if (d > 9 || v > (UINT64_MAX - d) / 10) return 0;
        v = v * 10 + d;
    }
    if (neg ? v > (uint64_t)INT64_MAX + 1 : v > (uint64_t)INT64_MAX) return 0;
    *out = neg ? (int64_t)(0 - v) : (int64_t)v;
    return 1;
}"##,
    },
    Piece {
        name: "parse_u64_try",
        deps: &["trim_span"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Decimal u64; a leading `+` is accepted, a `-` is not. */
static int parse_u64_try(Obj *o, uint64_t *out) {
    const char *b, *e;
    uint64_t v = 0;
    trim_span(STR(o), &b, &e);
    if (b < e && *b == '+') b++;
    if (b == e) return 0;
    for (; b < e; b++) {
        unsigned d = (unsigned char)*b - '0';
        if (d > 9 || v > (UINT64_MAX - d) / 10) return 0;
        v = v * 10 + d;
    }
    *out = v;
    return 1;
}"##,
    },
    Piece {
        name: "parse_f64_try",
        deps: &["trim_span", "rt_malloc"],
        hdrs: &["stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"/* Decimal floating point (`1`, `-2.5`, `.5`, `1e9`, `inf`, `NaN`); no hex, no stray characters. */
static int parse_f64_try(Obj *o, double *out) {
    const char *b, *e, *p;
    char *buf, low[10];
    size_t n, i, d1 = 0, d2 = 0;
    int ok = 1;
    trim_span(STR(o), &b, &e);
    n = (size_t)(e - b);
    buf = (char *)rt_malloc(n + 1);
    memcpy(buf, b, n);
    buf[n] = 0;
    p = buf;
    if (*p == '+' || *p == '-') p++;
    for (i = 0; i < 9 && p[i]; i++) low[i] = (char)((p[i] >= 'A' && p[i] <= 'Z') ? p[i] + 32 : p[i]);
    low[i] = 0;
    if (strcmp(low, "inf") == 0 || strcmp(low, "infinity") == 0 || strcmp(low, "nan") == 0) {
        *out = strtod(buf, NULL);
        free(buf);
        return 1;
    }
    while (*p >= '0' && *p <= '9') { p++; d1++; }
    if (*p == '.') { p++; while (*p >= '0' && *p <= '9') { p++; d2++; } }
    if (d1 + d2 == 0) ok = 0;
    if (ok && (*p == 'e' || *p == 'E')) {
        size_t de = 0;
        p++;
        if (*p == '+' || *p == '-') p++;
        while (*p >= '0' && *p <= '9') { p++; de++; }
        if (de == 0) ok = 0;
    }
    if (*p) ok = 0;
    if (ok) *out = strtod(buf, NULL);
    free(buf);
    return ok;
}"##,
    },
    Piece {
        name: "parse_i64",
        deps: &["parse_i64_try", "trap_parse"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int64_t parse_i64(Obj *o) {
    int64_t v;
    if (!parse_i64_try(o, &v)) trap_parse(o);
    return v;
}"##,
    },
    Piece {
        name: "parse_u64",
        deps: &["parse_u64_try", "trap_parse"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static uint64_t parse_u64(Obj *o) {
    uint64_t v;
    if (!parse_u64_try(o, &v)) trap_parse(o);
    return v;
}"##,
    },
    Piece {
        name: "parse_f64",
        deps: &["parse_f64_try", "trap_parse"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static double parse_f64(Obj *o) {
    double v;
    if (!parse_f64_try(o, &v)) trap_parse(o);
    return v;
}"##,
    },
    Piece {
        name: "parse_err",
        deps: &["res_err", "rt_malloc"],
        hdrs: &["stdio.h", "stdlib.h"],
        kind: Funcs,
        src: r##"static Obj *parse_err(Obj *o) {
    char *msg = (char *)rt_malloc(STR(o)->len + 32);
    Obj *r;
    sprintf(msg, "cannot parse `%.*s`", (int)STR(o)->len, STR(o)->s);
    r = res_err(msg);
    free(msg);
    return r;
}"##,
    },
    Piece {
        name: "try_parse_i64",
        deps: &["parse_i64_try", "parse_err", "res_ok"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *try_parse_i64(Obj *o) {
    int64_t v;
    return parse_i64_try(o, &v) ? res_ok((W)v, 0) : parse_err(o);
}"##,
    },
    Piece {
        name: "try_parse_u64",
        deps: &["parse_u64_try", "parse_err", "res_ok"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *try_parse_u64(Obj *o) {
    uint64_t v;
    return parse_u64_try(o, &v) ? res_ok(v, 0) : parse_err(o);
}"##,
    },
    Piece {
        name: "try_parse_f64",
        deps: &["parse_f64_try", "parse_err", "res_ok", "f64_bits"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *try_parse_f64(Obj *o) {
    double v;
    return parse_f64_try(o, &v) ? res_ok(f64_bits(v), 0) : parse_err(o);
}"##,
    },
    Piece {
        name: "str_get_char",
        deps: &["utf8_count", "utf8_skip", "utf8_dec", "opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_get_char(Obj *o, int64_t i) {
    const Str *s = STR(o);
    const char *end = s->s + s->len, *p;
    uint32_t c;
    if (i < 0 || (uint64_t)i >= utf8_count(s->s, s->len)) return opt_none();
    p = utf8_skip(s->s, end, (size_t)i);
    utf8_dec(p, end, &c);
    return opt_some(c, 0);
}"##,
    },
    Piece {
        name: "str_try_substring",
        deps: &["utf8_count", "str_substring", "res_ok", "res_err"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static Obj *str_try_substring(Obj *o, int64_t lo, int64_t hi) {
    size_t n = utf8_count(STR(o)->s, STR(o)->len);
    char msg[96];
    if (lo < 0 || hi < lo || (uint64_t)hi > n) {
        snprintf(msg, sizeof msg, "index %lld out of bounds (length %zu)", (long long)(lo < 0 || hi < lo ? lo : hi), n);
        return res_err(msg);
    }
    return res_ok(O2W(str_substring(o, lo, hi)), 1);
}"##,
    },
    Piece {
        name: "str_index_of",
        deps: &["str_find", "opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_index_of(Obj *a, Obj *b) {
    int64_t at = str_find(a, b);
    return at < 0 ? opt_none() : opt_some((W)at, 0);
}"##,
    },
    // ---- format specifications (Python style) -----------------------------------------------------
    Piece {
        name: "fmtspec_types",
        deps: &["sbtype"],
        hdrs: &[],
        kind: Decl,
        src: r##"/* `[[fill]align][sign][#][0][width][,|_][.precision][type]` */
typedef struct { uint32_t fill; int align, sign, alt, zero, grouping, ty; size_t width; long prec; } FSpec;
enum { FK_SINT, FK_UINT, FK_F64, FK_F32, FK_STR, FK_BOOL, FK_CHAR };   /* what is being formatted */
static SB fs_tmp;               /* the body, before padding */
static char fs_err[160];        /* the last error message */"##,
    },
    Piece {
        name: "fs_align",
        deps: &["fmtspec_types"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int fs_align(uint32_t c) {
    return c == '<' || c == '>' || c == '^' || c == '=';
}"##,
    },
    Piece {
        name: "fs_digits",
        deps: &["fmtspec_types"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Reads a run of decimal digits; -1 when there is none (saturates for absurd values). */
static long fs_digits(const uint32_t *c, size_t len, size_t *i) {
    size_t start = *i;
    unsigned long v = 0;
    while (*i < len && c[*i] >= '0' && c[*i] <= '9') {
        v = v * 10 + (c[*i] - '0');
        if (v > (1ul << 24)) v = 1ul << 24;
        (*i)++;
    }
    return *i > start ? (long)v : -1;
}"##,
    },
    Piece {
        name: "fs_read",
        deps: &["fmtspec_types", "fs_align", "fs_digits", "utf8_dec", "rt_malloc"],
        hdrs: &["stdio.h", "stdlib.h"],
        kind: Funcs,
        src: r##"/* Parses a specification; returns NULL, or a message saying what is wrong with it. */
static const char *fs_read(FSpec *s, const char *p, size_t n) {
    const char *start = p, *end = p + n, *err = NULL;
    uint32_t *c = (uint32_t *)rt_malloc((n + 1) * sizeof *c);
    size_t len = 0, i = 0;
    long w;
    while (p < end) p += utf8_dec(p, end, &c[len++]);
    s->fill = ' '; s->align = 0; s->sign = '-'; s->alt = 0; s->zero = 0;
    s->width = 0; s->grouping = 0; s->prec = -1; s->ty = 0;
    if (len >= 2 && fs_align(c[1])) { s->fill = c[0]; s->align = (int)c[1]; i = 2; }
    else if (len >= 1 && fs_align(c[0])) { s->align = (int)c[0]; i = 1; }
    if (i < len && (c[i] == '+' || c[i] == '-' || c[i] == ' ')) { s->sign = (int)c[i]; i++; }
    if (i < len && c[i] == '#') { s->alt = 1; i++; }
    if (i < len && c[i] == '0') { s->zero = 1; i++; }
    w = fs_digits(c, len, &i);
    if (w >= 0) {
        if (w > (1l << 20)) err = "format width is too large"; else s->width = (size_t)w;
    }
    if (!err && i < len && (c[i] == ',' || c[i] == '_')) { s->grouping = (int)c[i]; i++; }
    if (!err && i < len && c[i] == '.') {
        i++;
        w = fs_digits(c, len, &i);
        if (w < 0) err = "format specifier is missing the precision after `.`";
        else if (w > (1l << 20)) err = "format precision is too large";
        else s->prec = w;
    }
    if (!err) {
        if (len - i == 1) s->ty = (int)c[i];
        else if (len - i > 1) {
            snprintf(fs_err, sizeof fs_err, "invalid format specifier `%.*s`", (int)(n > 100 ? 100 : n), start);
            err = fs_err;
        }
    }
    free(c);
    return err;
}"##,
    },
    Piece {
        name: "fs_unknown",
        deps: &["fmtspec_types", "utf8_enc"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static const char *fs_unknown(int code, const char *what) {
    char ch[5];
    ch[utf8_enc((uint32_t)code, ch)] = 0;
    snprintf(fs_err, sizeof fs_err, "unknown format code `%s` for %s", ch, what);
    return fs_err;
}"##,
    },
    Piece {
        name: "fs_pad",
        deps: &["fmtspec_types", "sb_put", "sb_cp", "utf8_count"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* Appends sign + prefix + body to the scratch buffer, padded to the width. */
static void fs_pad(const char *sign, const char *prefix, const char *body, size_t blen, const FSpec *s, int dflt) {
    uint32_t fill = s->fill;
    int align = s->align;
    size_t len, gap, left = 0, mid = 0, right = 0, i;
    if (s->zero && !align) { fill = '0'; align = '='; }
    if (!align) align = dflt;
    len = strlen(sign) + strlen(prefix) + utf8_count(body, blen);
    gap = s->width > len ? s->width - len : 0;
    if (align == '<') right = gap;
    else if (align == '^') { left = gap / 2; right = gap - left; }
    else if (align == '=') mid = gap;
    else left = gap;
    for (i = 0; i < left; i++) sb_cp(&rt_sb, fill);
    sb_put(&rt_sb, sign, strlen(sign));
    sb_put(&rt_sb, prefix, strlen(prefix));
    for (i = 0; i < mid; i++) sb_cp(&rt_sb, fill);
    sb_put(&rt_sb, body, blen);
    for (i = 0; i < right; i++) sb_cp(&rt_sb, fill);
}"##,
    },
    Piece {
        name: "fs_sign",
        deps: &["fmtspec_types"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static const char *fs_sign(int negative, const FSpec *s) {
    if (negative) return "-";
    return s->sign == '+' ? "+" : s->sign == ' ' ? " " : "";
}"##,
    },
    Piece {
        name: "fs_group",
        deps: &["sb_put", "sb_putc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Appends `digits` to the temporary buffer with `sep` between groups of `size`, counting from the right. */
static void fs_group(const char *digits, size_t n, int sep, size_t size) {
    size_t i;
    for (i = 0; i < n; i++) {
        if (i > 0 && (n - i) % size == 0) sb_putc(&fs_tmp, (char)sep);
        sb_putc(&fs_tmp, digits[i]);
    }
}"##,
    },
    Piece {
        name: "fs_int",
        deps: &["fs_pad", "fs_sign", "fs_group", "fs_unknown", "fs_float_spec", "utf8_enc", "sb_put"],
        hdrs: &["stdio.h", "string.h"],
        kind: Funcs,
        src: r##"/* An integer given as a sign and a magnitude. */
static const char *fs_int(int neg, uint64_t mag, const FSpec *s) {
    int code = s->ty ? s->ty : 'd';
    char digits[72], *d = digits + sizeof digits;
    const char *prefix = "";
    size_t base = 10, size = 3, n;
    if (strchr("eEfFgG%", code)) return fs_float_spec(neg ? -(double)mag : (double)mag, s);
    if (!strchr("dnboxXc", code)) return fs_unknown(code, "an integer");
    if (s->prec >= 0) return "a precision is not allowed for integers";
    if (code == 'c') {
        char ch[5];
        if (s->sign != '-' || s->alt || s->grouping) return "sign, `#` and grouping are not allowed with the type `c`";
        if (neg || mag > 0x10FFFF || (mag >= 0xD800 && mag <= 0xDFFF)) return "value is not a valid character";
        fs_pad("", "", ch, utf8_enc((uint32_t)mag, ch), s, '<');
        return NULL;
    }
    if (code == 'b') { base = 2; prefix = "0b"; size = 4; }
    else if (code == 'o') { base = 8; prefix = "0o"; size = 4; }
    else if (code == 'x') { base = 16; prefix = "0x"; size = 4; }
    else if (code == 'X') { base = 16; prefix = "0X"; size = 4; }
    do {
        unsigned dg = (unsigned)(mag % base);
        *--d = (char)(dg < 10 ? '0' + dg : (code == 'X' ? 'A' : 'a') + dg - 10);
        mag /= base;
    } while (mag);
    n = (size_t)(digits + sizeof digits - d);
    fs_tmp.len = 0;
    if (s->grouping == ',' && size != 3) {
        snprintf(fs_err, sizeof fs_err, "`,` cannot be used with the type `%c` (use `_`)", code);
        return fs_err;
    }
    if (s->grouping) fs_group(d, n, s->grouping, size); else sb_put(&fs_tmp, d, n);
    fs_pad(fs_sign(neg, s), s->alt ? prefix : "", fs_tmp.p, fs_tmp.len, s, '>');
    return NULL;
}"##,
    },
    Piece {
        name: "fs_zeros",
        deps: &["fmtspec_types"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* "1.500" -> "1.5", "2.000" -> "2"; text without a point is left alone. */
static void fs_zeros(char *t) {
    size_t n;
    if (!strchr(t, '.')) return;
    n = strlen(t);
    while (n > 0 && t[n - 1] == '0') t[--n] = 0;
    if (n > 0 && t[n - 1] == '.') t[--n] = 0;
}"##,
    },
    Piece {
        name: "fs_printf",
        deps: &["rt_malloc"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* snprintf("%.*<conv>") into a fresh buffer. */
static char *fs_printf(const char *conv, int prec, double v) {
    int n = snprintf(NULL, 0, conv, prec, v);
    char *buf = (char *)rt_malloc((size_t)n + 1);
    snprintf(buf, (size_t)n + 1, conv, prec, v);
    return buf;
}"##,
    },
    Piece {
        name: "fs_float_spec",
        deps: &["fs_pad", "fs_sign", "fs_group", "fs_unknown", "fs_zeros", "fs_printf", "sb_puts", "sb_putc"],
        hdrs: &["math.h", "stdio.h", "stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"/* A float formatted with an explicit type (or a precision). */
static const char *fs_float_spec(double v, const FSpec *s) {
    int code = s->ty ? s->ty : 'g';
    int upper = code >= 'A' && code <= 'Z', neg;
    double a;
    char *t, *ex;
    if (!strchr("eEfFgG%", code)) return fs_unknown(code, "a float");
    neg = signbit(v) && !isnan(v);
    a = fabs(v);
    fs_tmp.len = 0;
    if (!isfinite(a)) {
        const char *text = isnan(a) ? (upper ? "NAN" : "nan") : (upper ? "INF" : "inf");
        sb_puts(&fs_tmp, text);
        if (code == '%') sb_putc(&fs_tmp, '%');
        fs_pad(fs_sign(neg, s), "", fs_tmp.p, fs_tmp.len, s, '>');
        return NULL;
    }
    if (code == 'f' || code == 'F') {
        t = fs_printf("%.*f", s->prec < 0 ? 6 : (int)s->prec, a);
        sb_puts(&fs_tmp, t);
        free(t);
    } else if (code == '%') {
        t = fs_printf("%.*f", s->prec < 0 ? 6 : (int)s->prec, a * 100.0);
        sb_puts(&fs_tmp, t);
        sb_putc(&fs_tmp, '%');
        free(t);
    } else if (code == 'e' || code == 'E') {
        t = fs_printf(upper ? "%.*E" : "%.*e", s->prec < 0 ? 6 : (int)s->prec, a);
        sb_puts(&fs_tmp, t);
        free(t);
    } else {
        int digits = s->prec < 0 ? 6 : s->prec == 0 ? 1 : (int)s->prec, e, decimals;
        t = fs_printf("%.*e", digits - 1, a);
        ex = strchr(t, 'e');
        e = atoi(ex + 1);
        *ex = 0;
        if (e >= -4 && e < digits) {
            char *fixed;
            decimals = digits - 1 - e;
            if (decimals < 0) decimals = 0;
            fixed = fs_printf("%.*f", decimals, a);
            if (!s->alt) fs_zeros(fixed);
            sb_puts(&fs_tmp, fixed);
            free(fixed);
        } else {
            char tail[16];
            if (!s->alt) fs_zeros(t);
            sb_puts(&fs_tmp, t);
            snprintf(tail, sizeof tail, "%c%c%02d", upper ? 'E' : 'e', e < 0 ? '-' : '+', e < 0 ? -e : e);
            sb_puts(&fs_tmp, tail);
        }
        free(t);
    }
    if (s->grouping) {
        char *copy = (char *)rt_malloc(fs_tmp.len + 1);
        size_t end = 0;
        memcpy(copy, fs_tmp.p, fs_tmp.len + 1);
        while (copy[end] >= '0' && copy[end] <= '9') end++;
        fs_tmp.len = 0;
        fs_group(copy, end, s->grouping, 3);
        sb_puts(&fs_tmp, copy + end);
        free(copy);
    }
    fs_pad(fs_sign(neg, s), "", fs_tmp.p, fs_tmp.len, s, '>');
    return NULL;
}"##,
    },
    Piece {
        name: "fs_float",
        deps: &["fs_float_spec", "sb_float", "fs_pad", "fs_sign", "fs_group", "sb_puts", "rt_malloc"],
        hdrs: &["string.h", "stdlib.h"],
        kind: Funcs,
        src: r##"/* A float formatted without type and precision prints like `print`, padded and grouped. */
static const char *fs_float(double v, int single, const FSpec *s) {
    const char *body;
    int neg = 0;
    size_t end = 0, n;
    if (s->ty || s->prec >= 0) return fs_float_spec(v, s);
    fs_tmp.len = 0;
    sb_float(&fs_tmp, v, single);
    body = fs_tmp.p;
    n = fs_tmp.len;
    if (body[0] == '-') { neg = 1; body++; n--; }
    if (s->grouping) {
        char *copy = (char *)rt_malloc(n + 1);
        memcpy(copy, body, n + 1);
        while (copy[end] >= '0' && copy[end] <= '9') end++;
        fs_tmp.len = 0;
        fs_group(copy, end, s->grouping, 3);
        sb_puts(&fs_tmp, copy + end);
        free(copy);
        body = fs_tmp.p;
        n = fs_tmp.len;
    }
    fs_pad(fs_sign(neg, s), "", body, n, s, '>');
    return NULL;
}"##,
    },
    Piece {
        name: "fs_str",
        deps: &["fs_pad", "fs_unknown", "utf8_skip"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static const char *fs_str(const char *text, size_t n, const FSpec *s) {
    if (s->ty && s->ty != 's' && s->ty != 'c') return fs_unknown(s->ty, "a string");
    if (s->sign != '-') return "a sign is not allowed in a string format specifier";
    if (s->alt) return "`#` is not allowed in a string format specifier";
    if (s->grouping) return "grouping is not allowed in a string format specifier";
    if (s->align == '=') return "`=` alignment is not allowed in a string format specifier";
    if (s->prec >= 0) n = (size_t)(utf8_skip(text, text + n, (size_t)s->prec) - text);
    fs_pad("", "", text, n, s, '<');
    return NULL;
}"##,
    },
    Piece {
        name: "fmt_value",
        deps: &[
            "fs_read",
            "fs_int",
            "fs_float",
            "fs_str",
            "sb_take",
            "trap_panic_text",
            "bits_f64",
            "bits_f32",
            "utf8_enc",
        ],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* x.format(spec) and the fields of format strings. `bits` is the canonical word of the value. */
static Obj *fmt_value(int kind, W bits, Obj *spec) {
    FSpec s;
    char msg[240], ch[5];
    const char *err = fs_read(&s, STR(spec)->s, STR(spec)->len);
    rt_sb.len = 0;
    if (!err) {
        int intcode = s.ty == 'd' || s.ty == 'b' || s.ty == 'o' || s.ty == 'x' || s.ty == 'X' || s.ty == 'n';
        switch (kind) {
        case FK_SINT: err = fs_int((int64_t)bits < 0, (int64_t)bits < 0 ? 0 - bits : bits, &s); break;
        case FK_UINT: err = fs_int(0, bits, &s); break;
        case FK_F64:  err = fs_float(bits_f64(bits), 0, &s); break;
        case FK_F32:  err = fs_float((double)bits_f32(bits), 1, &s); break;
        case FK_STR:  err = fs_str(STR(W2O(bits))->s, STR(W2O(bits))->len, &s); break;
        case FK_BOOL:
            if (intcode) err = fs_int(0, bits != 0, &s);
            else err = fs_str(bits ? "true" : "false", bits ? 4 : 5, &s);
            break;
        default:
            if (intcode) err = fs_int(0, (uint32_t)bits, &s);
            else { err = fs_str(ch, utf8_enc((uint32_t)bits, ch), &s); }
            break;
        }
    }
    if (err) {
        snprintf(msg, sizeof msg, "invalid format specifier: %s", err);
        trap_panic_text(msg);
    }
    return sb_take();
}"##,
    },
];
