//! Runtime pieces, part 1: compiler hints, object layout, traps, allocation, the collector, UTF-8
//! helpers and the basic string operations.

use super::rt::{Kind::*, Piece};

pub static PIECES: &[Piece] = &[
    Piece {
        name: "checks",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"/* Debug or release. Checked (debug) builds trap on integer overflow, division by zero, bad shifts and
 * too deep recursion; with -DNDEBUG (or -DNEON_CHECKS=0) integers wrap around, division by zero is
 * left to C and the recursion is not counted. Array bounds, map keys, parsing and panics are always checked. */
#ifndef NEON_CHECKS
#ifdef NDEBUG
#define NEON_CHECKS 0
#else
#define NEON_CHECKS 1
#endif
#endif"##,
    },
    Piece {
        name: "hint_expect",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"#if defined(__GNUC__) || defined(__clang__)
#define LIKELY(x)   __builtin_expect(!!(x), 1)
#define UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define LIKELY(x)   (x)
#define UNLIKELY(x) (x)
#endif"##,
    },
    Piece {
        name: "hint_cold",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"#if defined(__GNUC__) || defined(__clang__)
#define NORETURN __attribute__((noreturn))
#define COLD     __attribute__((cold))
#else
#define NORETURN
#define COLD
#endif"##,
    },
    Piece {
        name: "hint_const",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"/* CONST_FN: the result depends on the arguments only; PURE_FN: it may also read memory. */
#if defined(__GNUC__) || defined(__clang__)
#define CONST_FN __attribute__((const))
#define PURE_FN  __attribute__((pure))
#else
#define CONST_FN
#define PURE_FN
#endif"##,
    },
    Piece {
        name: "hint_printf",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"#if defined(__GNUC__) || defined(__clang__)
#define PRINTF_FN(f, a) __attribute__((format(printf, f, a)))
#else
#define PRINTF_FN(f, a)
#endif"##,
    },
    Piece {
        name: "ovf",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"/* Checked integer arithmetic: true when the mathematical result does not fit the result type. */
#if defined(__GNUC__) || defined(__clang__)
#define OVF_ADD(a, b, r) __builtin_add_overflow((a), (b), (r))
#define OVF_SUB(a, b, r) __builtin_sub_overflow((a), (b), (r))
#define OVF_MUL(a, b, r) __builtin_mul_overflow((a), (b), (r))
#else
#error "checked integer arithmetic needs a GCC-compatible compiler (__builtin_*_overflow)"
#endif"##,
    },
    Piece { name: "hdr_math", deps: &[], hdrs: &["math.h"], kind: Decl, src: "" },
    Piece { name: "hdr_stdio", deps: &[], hdrs: &["stdio.h"], kind: Decl, src: "" },
    Piece {
        name: "unit",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"typedef uint8_t unit_t;  /* the type `()`: always 0 */"##,
    },
    Piece {
        name: "word",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"typedef uint64_t W;     /* a machine word: any value stored inside a container */"##,
    },
    Piece {
        name: "objects",
        deps: &["word"],
        hdrs: &[],
        kind: Decl,
        src: r##"/* Heap objects. Every object starts with the same header; `flags` and `n` depend on `kind`:
 *   K_ARR  flags & 1: the elements are references
 *   K_AGG  n = field count (structs and enum variants)
 *   K_MAP  flags: MF_* below (maps and sets share one layout)
 *   K_CLO  n = number of captured cells
 *   K_CELL flags & 1: the value is a reference                                              */
enum { K_STR, K_ARR, K_AGG, K_MAP, K_CLO, K_CELL };
enum { MF_KREF = 1, MF_VREF = 2, MF_KSTR = 4, MF_SET = 8 };

typedef struct Obj Obj;
struct Obj {
    Obj *next;          /* all objects form a list for the sweep phase */
    uint8_t kind, mark;
    uint16_t flags;
    uint32_t n;
};
typedef struct { Obj h; size_t len; char s[]; } Str;                       /* UTF-8, NUL-terminated */
typedef struct { Obj h; size_t len, cap; W *a; } Arr;
typedef struct { Obj h; uint64_t mask; uint32_t tag; W f[]; } Agg;         /* mask: which fields are references */
typedef struct { Obj h; size_t len, cap, ixcap; W *k, *v; int32_t *ix; } Map;  /* insertion ordered + hash index */
typedef struct { Obj h; void (*fn)(void); Obj *c[]; } Clo;                 /* code + captured cells */
typedef struct { Obj h; W v; } Cell;                                       /* a captured variable */

#define STR(o)  ((Str *)(o))
#define ARR(o)  ((Arr *)(o))
#define AGG(o)  ((Agg *)(o))
#define MAP(o)  ((Map *)(o))
#define CLO(o)  ((Clo *)(o))
#define CELL(o) ((Cell *)(o))
#define W2O(w)  ((Obj *)(uintptr_t)(w))
#define O2W(o)  ((W)(uintptr_t)(o))"##,
    },
    Piece {
        name: "heapstate",
        deps: &["objects"],
        hdrs: &[],
        kind: Decl,
        src: r##"static Obj *gc_all;            /* every object, for the sweep phase */
static size_t gc_bytes;        /* bytes allocated since the last collection, plus the survivors */"##,
    },
    Piece {
        name: "gcstate",
        deps: &["heapstate", "hint_expect"],
        hdrs: &[],
        kind: Decl,
        src: r##"/* Garbage collector state. Functions that hold references register them in a Frame. */
typedef struct Frame Frame;
struct Frame { Frame *up; Obj ***slots; size_t n; };
static Frame *gc_top;          /* innermost registered frame */
static size_t gc_next = (size_t)4 << 20;   /* collect when gc_bytes reaches this */
static Obj **gc_stack;         /* work list of the mark phase */
static size_t gc_nstack, gc_capstack;
#ifdef NEON_GC_STRESS   /* -DNEON_GC_STRESS: collect at every safe-point, to test the collector */
#define GC_POINT() gc_collect()
#else
#define GC_POINT() do { if (UNLIKELY(gc_bytes >= gc_next)) gc_collect(); } while (0)
#endif"##,
    },
    Piece {
        name: "user_roots_decl",
        deps: &[],
        hdrs: &[],
        kind: Decl,
        src: r##"static void user_roots(void);   /* defined with the program: marks globals and string constants */"##,
    },
    Piece {
        name: "depth",
        deps: &["checks"],
        hdrs: &[],
        kind: Decl,
        src: r##"#if NEON_CHECKS
#ifndef NEON_MAX_DEPTH
#define NEON_MAX_DEPTH 20000   /* deeper recursion is reported as a stack overflow */
#endif
static unsigned rt_depth;
#define DEPTH_ENTER() do { if (UNLIKELY(++rt_depth > NEON_MAX_DEPTH)) trap_stack(); } while (0)
#define DEPTH_LEAVE() (rt_depth--)
#else
#define DEPTH_ENTER() ((void)0)
#define DEPTH_LEAVE() ((void)0)
#endif"##,
    },
    // ---- traps ---------------------------------------------------------------------------
    Piece {
        name: "rt_fail",
        deps: &["hint_cold", "hint_printf"],
        hdrs: &["stdarg.h", "stdio.h", "stdlib.h"],
        kind: Funcs,
        src: r##"static NORETURN COLD PRINTF_FN(1, 2) void rt_fail(const char *fmt, ...) {
    va_list ap;
    fflush(stdout);
    fputs("runtime error: ", stderr);
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fputc('\n', stderr);
    exit(2);
}"##,
    },
    Piece {
        name: "trap_overflow",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_overflow(void) {
    rt_fail("integer overflow");
}"##,
    },
    Piece {
        name: "trap_div0",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_div0(void) {
    rt_fail("division by zero");
}"##,
    },
    Piece {
        name: "trap_shift",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_shift(void) {
    rt_fail("shift amount out of range");
}"##,
    },
    Piece {
        name: "trap_index",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_index(int64_t index, size_t len) {
    rt_fail("index %lld out of bounds (length %zu)", (long long)index, len);
}"##,
    },
    Piece {
        name: "trap_key",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_key(void) {
    rt_fail("key not found in map");
}"##,
    },
    Piece {
        name: "trap_empty",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_empty(void) {
    rt_fail("pop from an empty array");
}"##,
    },
    Piece {
        name: "trap_oom",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_oom(void) {
    rt_fail("out of memory");
}"##,
    },
    Piece {
        name: "trap_stack",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_stack(void) {
    rt_fail("stack overflow");
}"##,
    },
    Piece {
        name: "trap_unreach",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_unreach(const char *fn) {
    rt_fail("panic: reached unreachable code in `%s`", fn);
}"##,
    },
    Piece {
        name: "trap_panic_text",
        deps: &["rt_fail"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_panic_text(const char *msg) {
    rt_fail("panic: %s", msg);
}"##,
    },
    Piece {
        name: "trap_panic",
        deps: &["rt_fail", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_panic(Obj *msg) {
    rt_fail("panic: %.*s", (int)STR(msg)->len, STR(msg)->s);
}"##,
    },
    Piece {
        name: "trap_assert",
        deps: &["rt_fail", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_assert(Obj *msg) {
    if (msg == NULL) rt_fail("panic: assertion failed");
    rt_fail("panic: assertion failed: %.*s", (int)STR(msg)->len, STR(msg)->s);
}"##,
    },
    Piece {
        name: "trap_parse",
        deps: &["rt_fail", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static NORETURN COLD void trap_parse(Obj *s) {
    rt_fail("cannot parse `%.*s`", (int)STR(s)->len, STR(s)->s);
}"##,
    },
    // ---- memory ---------------------------------------------------------------------------
    Piece {
        name: "rt_malloc",
        deps: &["trap_oom", "hint_expect"],
        hdrs: &["stdlib.h"],
        kind: Funcs,
        src: r##"static void *rt_malloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (UNLIKELY(p == NULL)) trap_oom();
    return p;
}"##,
    },
    Piece {
        name: "rt_realloc",
        deps: &["trap_oom", "hint_expect"],
        hdrs: &["stdlib.h"],
        kind: Funcs,
        src: r##"static void *rt_realloc(void *p, size_t n) {
    p = realloc(p, n ? n : 1);
    if (UNLIKELY(p == NULL)) trap_oom();
    return p;
}"##,
    },
    Piece {
        name: "obj_new",
        deps: &["objects", "heapstate", "rt_malloc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *obj_new(size_t size, unsigned kind, unsigned flags) {
    Obj *o = (Obj *)rt_malloc(size);
    o->next = gc_all;
    gc_all = o;
    o->kind = (uint8_t)kind;
    o->mark = 0;
    o->flags = (uint16_t)flags;
    o->n = 0;
    gc_bytes += size;
    return o;
}"##,
    },
    // ---- the collector ----------------------------------------------------------------------
    Piece {
        name: "gc_mark",
        deps: &["gcstate", "rt_realloc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void gc_mark(Obj *o) {
    if (o == NULL || o->mark) return;
    o->mark = 1;
    if (o->kind == K_STR) return;
    if (gc_nstack == gc_capstack) {
        gc_capstack = gc_capstack ? gc_capstack * 2 : 256;
        gc_stack = (Obj **)rt_realloc(gc_stack, gc_capstack * sizeof *gc_stack);
    }
    gc_stack[gc_nstack++] = o;
}"##,
    },
    Piece {
        name: "gc_scan",
        deps: &["gc_mark"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void gc_scan(Obj *o) {
    size_t i;
    switch (o->kind) {
    case K_ARR:
        if (o->flags & 1) for (i = 0; i < ARR(o)->len; i++) gc_mark(W2O(ARR(o)->a[i]));
        break;
    case K_AGG:
        for (i = 0; i < o->n && i < 64; i++) if ((AGG(o)->mask >> i) & 1) gc_mark(W2O(AGG(o)->f[i]));
        break;
    case K_MAP:
        for (i = 0; i < MAP(o)->len; i++) {
            if (o->flags & MF_KREF) gc_mark(W2O(MAP(o)->k[i]));
            if (o->flags & MF_VREF) gc_mark(W2O(MAP(o)->v[i]));
        }
        break;
    case K_CLO:
        for (i = 0; i < o->n; i++) gc_mark(CLO(o)->c[i]);
        break;
    case K_CELL:
        if (o->flags & 1) gc_mark(W2O(CELL(o)->v));
        break;
    default:
        break;
    }
}"##,
    },
    Piece {
        name: "gc_size",
        deps: &["objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static size_t gc_size(const Obj *o) {
    switch (o->kind) {
    case K_STR: return sizeof(Str) + ((const Str *)o)->len + 1;
    case K_ARR: return sizeof(Arr) + ((const Arr *)o)->cap * sizeof(W);
    case K_AGG: return sizeof(Agg) + o->n * sizeof(W);
    case K_MAP: return sizeof(Map) + ((const Map *)o)->cap * 2 * sizeof(W) + ((const Map *)o)->ixcap * sizeof(int32_t);
    case K_CLO: return sizeof(Clo) + o->n * sizeof(Obj *);
    default:    return sizeof(Cell);
    }
}"##,
    },
    Piece {
        name: "gc_free",
        deps: &["objects"],
        hdrs: &["stdlib.h"],
        kind: Funcs,
        src: r##"static void gc_free(Obj *o) {
    if (o->kind == K_ARR) free(ARR(o)->a);
    if (o->kind == K_MAP) { free(MAP(o)->k); free(MAP(o)->v); free(MAP(o)->ix); }
    free(o);
}"##,
    },
    Piece {
        name: "gc_collect",
        deps: &["gcstate", "gc_mark", "gc_scan", "gc_size", "gc_free", "user_roots_decl"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Mark and sweep. Called only at GC_POINT()s, so no runtime function ever sees a collection. */
static void gc_collect(void) {
    Frame *f;
    Obj **link, *o;
    size_t i, live = 0;
    for (f = gc_top; f != NULL; f = f->up)
        for (i = 0; i < f->n; i++) gc_mark(*f->slots[i]);
    user_roots();
    while (gc_nstack > 0) gc_scan(gc_stack[--gc_nstack]);
    link = &gc_all;
    while ((o = *link) != NULL) {
        if (o->mark) {
            o->mark = 0;
            live += gc_size(o);
            link = &o->next;
        } else {
            *link = o->next;
            gc_free(o);
        }
    }
    gc_bytes = live;
    gc_next = live * 2 > ((size_t)4 << 20) ? live * 2 : ((size_t)4 << 20);
}"##,
    },
    // ---- bit casts ------------------------------------------------------------------------------
    Piece {
        name: "f64_bits",
        deps: &["hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static CONST_FN W f64_bits(double x) {
    W w;
    memcpy(&w, &x, sizeof w);
    return w;
}"##,
    },
    Piece {
        name: "bits_f64",
        deps: &["hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static CONST_FN double bits_f64(W w) {
    double x;
    memcpy(&x, &w, sizeof x);
    return x;
}"##,
    },
    Piece {
        name: "f32_bits",
        deps: &["hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static CONST_FN W f32_bits(float x) {
    uint32_t u;
    memcpy(&u, &x, sizeof u);
    return u;
}"##,
    },
    Piece {
        name: "bits_f32",
        deps: &["hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static CONST_FN float bits_f32(W w) {
    uint32_t u = (uint32_t)w;
    float x;
    memcpy(&x, &u, sizeof x);
    return x;
}"##,
    },
    // ---- UTF-8 ------------------------------------------------------------------------------------
    Piece {
        name: "utf8_dec",
        deps: &[],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Decodes the code point at p; returns the number of bytes it takes (a stray byte counts as one). */
static size_t utf8_dec(const char *p, const char *end, uint32_t *cp) {
    unsigned b0 = (unsigned char)p[0];
    size_t left = (size_t)(end - p);
    if (b0 < 0x80) { *cp = b0; return 1; }
    if (b0 >= 0xC0 && b0 < 0xE0 && left >= 2) {
        *cp = ((b0 & 0x1Fu) << 6) | ((unsigned char)p[1] & 0x3Fu);
        return 2;
    }
    if (b0 >= 0xE0 && b0 < 0xF0 && left >= 3) {
        *cp = ((b0 & 0x0Fu) << 12) | (((unsigned char)p[1] & 0x3Fu) << 6) | ((unsigned char)p[2] & 0x3Fu);
        return 3;
    }
    if (b0 >= 0xF0 && b0 < 0xF8 && left >= 4) {
        *cp = ((b0 & 0x07u) << 18) | (((unsigned char)p[1] & 0x3Fu) << 12) | (((unsigned char)p[2] & 0x3Fu) << 6) |
              ((unsigned char)p[3] & 0x3Fu);
        return 4;
    }
    *cp = 0xFFFD;
    return 1;
}"##,
    },
    Piece {
        name: "utf8_enc",
        deps: &[],
        hdrs: &[],
        kind: Funcs,
        src: r##"static size_t utf8_enc(uint32_t c, char *out) {
    if (c < 0x80) { out[0] = (char)c; return 1; }
    if (c < 0x800) { out[0] = (char)(0xC0 | (c >> 6)); out[1] = (char)(0x80 | (c & 0x3F)); return 2; }
    if (c < 0x10000) {
        out[0] = (char)(0xE0 | (c >> 12));
        out[1] = (char)(0x80 | ((c >> 6) & 0x3F));
        out[2] = (char)(0x80 | (c & 0x3F));
        return 3;
    }
    out[0] = (char)(0xF0 | (c >> 18));
    out[1] = (char)(0x80 | ((c >> 12) & 0x3F));
    out[2] = (char)(0x80 | ((c >> 6) & 0x3F));
    out[3] = (char)(0x80 | (c & 0x3F));
    return 4;
}"##,
    },
    Piece {
        name: "utf8_count",
        deps: &["hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Number of code points in n bytes. */
static PURE_FN size_t utf8_count(const char *p, size_t n) {
    size_t i, c = 0;
    for (i = 0; i < n; i++) c += ((unsigned char)p[i] & 0xC0) != 0x80;
    return c;
}"##,
    },
    Piece {
        name: "utf8_skip",
        deps: &["utf8_dec"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* The position after n code points (or the end). */
static const char *utf8_skip(const char *p, const char *end, size_t n) {
    uint32_t c;
    while (n > 0 && p < end) {
        p += utf8_dec(p, end, &c);
        n--;
    }
    return p;
}"##,
    },
    // ---- basic strings ------------------------------------------------------------------------
    Piece {
        name: "str_new",
        deps: &["obj_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *str_new(const char *s, size_t n) {
    Str *o = (Str *)obj_new(sizeof(Str) + n + 1, K_STR, 0);
    o->len = n;
    if (n) memcpy(o->s, s, n);
    o->s[n] = 0;
    return &o->h;
}"##,
    },
    Piece {
        name: "str_cat",
        deps: &["obj_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *str_cat(Obj *a, Obj *b) {
    size_t n = STR(a)->len, m = STR(b)->len;
    Str *o = (Str *)obj_new(sizeof(Str) + n + m + 1, K_STR, 0);
    o->len = n + m;
    memcpy(o->s, STR(a)->s, n);
    memcpy(o->s + n, STR(b)->s, m);
    o->s[n + m] = 0;
    return &o->h;
}"##,
    },
    Piece {
        name: "str_eq",
        deps: &["objects", "hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static PURE_FN int str_eq(const Obj *a, const Obj *b) {
    return ((const Str *)a)->len == ((const Str *)b)->len &&
           memcmp(((const Str *)a)->s, ((const Str *)b)->s, ((const Str *)a)->len) == 0;
}"##,
    },
    Piece {
        name: "str_cmp",
        deps: &["objects", "hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* Byte-wise lexicographic order: -1, 0 or 1. */
static PURE_FN int str_cmp(const Obj *a, const Obj *b) {
    size_t n = ((const Str *)a)->len < ((const Str *)b)->len ? ((const Str *)a)->len : ((const Str *)b)->len;
    int c = memcmp(((const Str *)a)->s, ((const Str *)b)->s, n);
    if (c != 0) return c < 0 ? -1 : 1;
    return (((const Str *)a)->len > ((const Str *)b)->len) - (((const Str *)a)->len < ((const Str *)b)->len);
}"##,
    },
    Piece {
        name: "str_len",
        deps: &["objects", "utf8_count"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Length in characters. */
static int64_t str_len(Obj *o) {
    return (int64_t)utf8_count(STR(o)->s, STR(o)->len);
}"##,
    },
    Piece {
        name: "str_pos",
        deps: &["objects", "hint_const"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* Byte offset of the first occurrence of `sub`, or -1. */
static PURE_FN long str_pos(const Obj *a, const Obj *b) {
    const Str *s = (const Str *)a, *t = (const Str *)b;
    size_t i;
    if (t->len > s->len) return -1;
    for (i = 0; i + t->len <= s->len; i++)
        if (memcmp(s->s + i, t->s, t->len) == 0) return (long)i;
    return -1;
}"##,
    },
    Piece {
        name: "str_find",
        deps: &["str_pos", "utf8_count"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Character index of the first occurrence of `sub`, or -1. */
static int64_t str_find(Obj *a, Obj *b) {
    long at = str_pos(a, b);
    return at < 0 ? -1 : (int64_t)utf8_count(STR(a)->s, (size_t)at);
}"##,
    },
    Piece {
        name: "str_starts",
        deps: &["objects"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static int str_starts(Obj *a, Obj *b) {
    return STR(b)->len <= STR(a)->len && memcmp(STR(a)->s, STR(b)->s, STR(b)->len) == 0;
}"##,
    },
    Piece {
        name: "str_ends",
        deps: &["objects"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static int str_ends(Obj *a, Obj *b) {
    return STR(b)->len <= STR(a)->len && memcmp(STR(a)->s + STR(a)->len - STR(b)->len, STR(b)->s, STR(b)->len) == 0;
}"##,
    },
    Piece {
        name: "str_substring",
        deps: &["str_new", "utf8_count", "utf8_skip", "trap_index"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *str_substring(Obj *o, int64_t lo, int64_t hi) {
    const Str *s = STR(o);
    size_t n = utf8_count(s->s, s->len);
    const char *end = s->s + s->len, *b, *e;
    if (lo < 0 || hi < lo || (uint64_t)hi > n) trap_index(lo < 0 || hi < lo ? lo : hi, n);
    b = utf8_skip(s->s, end, (size_t)lo);
    e = utf8_skip(b, end, (size_t)(hi - lo));
    return str_new(b, (size_t)(e - b));
}"##,
    },
    Piece {
        name: "str_char_at",
        deps: &["utf8_count", "utf8_skip", "utf8_dec", "trap_index"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static uint32_t str_char_at(Obj *o, int64_t i) {
    const Str *s = STR(o);
    const char *end = s->s + s->len, *p;
    uint32_t c;
    if (i < 0 || (uint64_t)i >= utf8_count(s->s, s->len)) trap_index(i, utf8_count(s->s, s->len));
    p = utf8_skip(s->s, end, (size_t)i);
    utf8_dec(p, end, &c);
    return c;
}"##,
    },
];
