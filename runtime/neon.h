/* neon.h - the interface of the Neon runtime.
 *
 * Programs translated with `neon c` include this header and are linked with neon.c:
 *
 *     cc -O2 -I runtime program.c runtime/neon.c -o program -lm
 *
 * It declares the type names of Neon, the heap objects (strings, arrays, maps, closures, structs and
 * enum variants), the collector's interface, the traps and the library the generated code calls, and
 * defines the checked integer arithmetic as `static inline` functions. The same header is what a C
 * module of a Neon program (`extern fn`) includes.
 *
 * Switches: -DNDEBUG or -DNEON_CHECKS=0 (release: integers wrap around, no recursion guard),
 * -DNEON_MAX_DEPTH=n (checked builds report deeper recursion as a stack overflow), -DNEON_GC_STRESS
 * (collect at every safe-point, to test the collector).
 */
#ifndef NEON_H
#define NEON_H

/* constant bounds make some comparisons always true (an inclusive range up to `255u8`) */
#if defined(__GNUC__) || defined(__clang__)
#pragma GCC diagnostic ignored "-Wtype-limits"
#endif

#include <stddef.h>
#include <stdint.h>
#include <stdarg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>

/* The type names of Neon. `chr` is a Unicode scalar value (`char` is C's own); `unit` is `void`, used
 * for the result of functions that return nothing. */
typedef int8_t i8;
typedef int16_t i16;
typedef int32_t i32;
typedef int64_t i64;
typedef uint8_t u8;
typedef uint16_t u16;
typedef uint32_t u32;
typedef uint64_t u64;
typedef float f32;
typedef double f64;
typedef u32 chr;
typedef void unit;
#if !(defined(__STDC_VERSION__) && __STDC_VERSION__ >= 202311L)
typedef u8 bool;   /* C23 has a `bool` of its own */
#endif

/* Debug or release. Checked (debug) builds trap on integer overflow, division by zero, bad shifts and
 * too deep recursion; with -DNDEBUG (or -DNEON_CHECKS=0) integers wrap around, division by zero is
 * left to C and the recursion is not counted. Array bounds, map keys, parsing and panics are always checked. */
#ifndef NEON_CHECKS
#ifdef NDEBUG
#define NEON_CHECKS 0
#else
#define NEON_CHECKS 1
#endif
#endif

#if defined(__GNUC__) || defined(__clang__)
#define LIKELY(x)   __builtin_expect(!!(x), 1)
#define UNLIKELY(x) __builtin_expect(!!(x), 0)
#else
#define LIKELY(x)   (x)
#define UNLIKELY(x) (x)
#endif

#if defined(__GNUC__) || defined(__clang__)
#define NORETURN __attribute__((noreturn))
#define COLD     __attribute__((cold))
#else
#define NORETURN
#define COLD
#endif

/* CONST_FN: the result depends on the arguments only; PURE_FN: it may also read memory. */
#if defined(__GNUC__) || defined(__clang__)
#define CONST_FN __attribute__((const))
#define PURE_FN  __attribute__((pure))
#else
#define CONST_FN
#define PURE_FN
#endif

#if defined(__GNUC__) || defined(__clang__)
#define PRINTF_FN(f, a) __attribute__((format(printf, f, a)))
#else
#define PRINTF_FN(f, a)
#endif

/* Checked integer arithmetic: true when the mathematical result does not fit the result type. */
#if defined(__GNUC__) || defined(__clang__)
#define OVF_ADD(a, b, r) __builtin_add_overflow((a), (b), (r))
#define OVF_SUB(a, b, r) __builtin_sub_overflow((a), (b), (r))
#define OVF_MUL(a, b, r) __builtin_mul_overflow((a), (b), (r))
#else
#error "checked integer arithmetic needs a GCC-compatible compiler (__builtin_*_overflow)"
#endif

typedef u64 W;     /* a machine word: any value stored inside a container */

/* Heap objects. Every object starts with the same 8-byte header; `flags` and `n` depend on `kind`:
 *   K_ARR  flags & 1: the elements are references
 *   K_AGG  flags = variant number, n = field count (structs and enum variants)
 *   K_MAP  flags: MF_* below (maps and sets share one layout)
 *   K_CLO  n = number of captured cells
 *   K_CELL flags & 1: the value is a reference
 *   K_FREE a cell of the allocator that holds no object                                     */
enum { K_STR, K_ARR, K_AGG, K_MAP, K_CLO, K_CELL, K_FREE = 255 };
enum { MF_KREF = 1, MF_VREF = 2, MF_KSTR = 4, MF_SET = 8 };

typedef struct Obj Obj;
struct Obj {
    u8 kind, mark;
    u16 flags;
    u32 n;
};
typedef struct { Obj h; size_t len; char s[]; } Str;                       /* UTF-8, NUL-terminated */
typedef struct { Obj h; size_t len, cap; W *a; } Arr;
typedef struct { Obj h; u64 mask; W f[]; } Agg;                       /* mask: which fields are references */
typedef struct { Obj h; size_t len, cap, ixcap; W *k, *v; i32 *ix; } Map;  /* insertion ordered + hash index */
typedef struct { Obj h; void (*fn)(void); Obj *c[]; } Clo;                 /* code + captured cells */
typedef struct { Obj h; W v; } Cell;                                       /* a captured variable */

#define STR(o)  ((Str *)(o))
#define ARR(o)  ((Arr *)(o))
#define AGG(o)  ((Agg *)(o))
#define TAG(o)  (AGG(o)->h.flags)
#define MAP(o)  ((Map *)(o))
#define CLO(o)  ((Clo *)(o))
#define CELL(o) ((Cell *)(o))
#define W2O(w)  ((Obj *)(uintptr_t)(w))
#define O2W(o)  ((W)(uintptr_t)(o))

/* The allocator. Objects of up to GC_SMALL bytes live in 64 KB blocks cut into cells of one size class
 * (a multiple of 16 bytes); freed cells go on a per-class list. Bigger objects are plain malloc blocks
 * behind a small prefix. Nothing ever moves, so references stay valid across collections. */
enum { GC_BLOCK = 65536, GC_SMALL = 256, GC_CLASSES = GC_SMALL / 16 };
typedef struct Block Block;
struct Block { Block *next; size_t cls, top, pad; };   /* the cells follow; `top` = bytes carved so far */
typedef struct Big Big;
struct Big { Big *next; size_t size; };                /* the object follows */
extern size_t gc_bytes;   /* bytes allocated since the last collection, plus the survivors */

/* Garbage collector state. Functions that hold references register them in a Frame. */
typedef struct Frame Frame;
struct Frame { Frame *up; Obj ***slots; size_t n; };
extern Frame *gc_top;   /* innermost registered frame */
extern size_t gc_next;   /* collect when gc_bytes reaches this */
/* FRAME(&a, &b, ...) at the top of a function registers its reference variables; FRAME_LEAVE() before every return. */
#define FRAME(...) Obj **fr_roots[] = {__VA_ARGS__}; Frame fr = {gc_top, fr_roots, sizeof fr_roots / sizeof *fr_roots}; gc_top = &fr
#define FRAME_LEAVE() (gc_top = fr.up)
#ifdef NEON_GC_STRESS   /* -DNEON_GC_STRESS: collect at every safe-point, to test the collector */
#define GC_POINT() gc_collect()
#else
#define GC_POINT() do { if (UNLIKELY(gc_bytes >= gc_next)) gc_collect(); } while (0)
#endif

void user_roots(void);   /* defined with the program: marks globals and string constants */

#if NEON_CHECKS
#ifndef NEON_MAX_DEPTH
#define NEON_MAX_DEPTH 20000   /* deeper recursion is reported as a stack overflow */
#endif
extern unsigned rt_depth;
#define DEPTH_ENTER() do { if (UNLIKELY(++rt_depth > NEON_MAX_DEPTH)) trap_stack(); } while (0)
#define DEPTH_LEAVE() (rt_depth--)
#else
#define DEPTH_ENTER() ((void)0)
#define DEPTH_LEAVE() ((void)0)
#endif

/* A growable text buffer. `rt_sb` is the scratch buffer shared by printing and formatting. */
typedef struct { char *p; size_t len, cap; } SB;
extern SB rt_sb;

extern int rt_argc;   /* the arguments of main, for sys.args */
extern char **rt_argv;

/* `[[fill]align][sign][#][0][width][,|_][.precision][type]` */
typedef struct { u32 fill; int align, sign, alt, zero, grouping, ty; size_t width; long prec; } FSpec;
enum { FK_SINT, FK_UINT, FK_F64, FK_F32, FK_STR, FK_BOOL, FK_CHAR };   /* what is being formatted */

/* ---- the library -------------------------------------------------------------------------- */

NORETURN COLD PRINTF_FN(1, 2) void rt_fail(const char *fmt, ...);
NORETURN COLD void trap_overflow(void);
NORETURN COLD void trap_div0(void);
NORETURN COLD void trap_shift(void);
NORETURN COLD void trap_index(i64 index, size_t len);
NORETURN COLD void trap_stack(void);
NORETURN COLD void trap_unreach(const char *fn);
NORETURN COLD void trap_panic(Obj *msg);
NORETURN COLD void trap_assert(Obj *msg);
Obj *obj_new(size_t size, unsigned kind, unsigned flags);
void gc_mark(Obj *o);
void gc_collect(void);
CONST_FN W f64_bits(f64 x);
CONST_FN f64 bits_f64(W w);
CONST_FN W f32_bits(f32 x);
CONST_FN f32 bits_f32(W w);
const char *utf8_skip(const char *p, const char *end, size_t n);
Obj *str_new(const char *s, size_t n);
Obj *str_from_c(const char *s);   /* from NUL-terminated C text; invalid UTF-8 becomes U+FFFD */
Obj *str_cat(Obj *a, Obj *b);
PURE_FN int str_eq(const Obj *a, const Obj *b);
PURE_FN int str_cmp(const Obj *a, const Obj *b);
i64 str_len(Obj *o);
PURE_FN long str_pos(const Obj *a, const Obj *b);
i64 str_find(Obj *a, Obj *b);
int str_starts(Obj *a, Obj *b);
int str_ends(Obj *a, Obj *b);
Obj *str_substring(Obj *o, i64 lo, i64 hi);
u32 str_char_at(Obj *o, i64 i);
Obj *agg_new(u32 tag, u32 n, u64 mask);
Obj *opt_some(W v, int is_ref);
Obj *opt_none(void);
Obj *arr_new(size_t cap, int is_ref);
void arr_push(Obj *o, W v);
W arr_pop(Obj *o);
void arr_insert(Obj *o, i64 i, W v);
W arr_remove(Obj *o, i64 i);
void arr_reverse(Obj *o);
Obj *arr_clone(Obj *o);
int arr_contains(Obj *o, W x, int (*eq)(W, W));
void arr_sort(Obj *o, int (*cmp)(W, W));
Obj *arr_try_get(Obj *o, i64 i);
Obj *arr_try_pop(Obj *o);
Obj *arr_try_remove(Obj *o, i64 i);
PURE_FN int eq_str(W a, W b);
PURE_FN int cmp_str(W a, W b);
Obj *map_new(unsigned flags);
long map_find(Obj *o, W key);
int map_put(Obj *o, W key, W val);
W map_get(Obj *o, W key);
W map_get_or(Obj *o, W key, W dflt);
Obj *map_try_get(Obj *o, W key);
int map_del(Obj *o, W key);
void map_clear(Obj *o);
Obj *map_keys(Obj *o);
Obj *map_values(Obj *o);
Obj *clo_new(void (*fn)(void), u32 n);
Obj *cell_new(int is_ref, W v);
void sb_put(SB *b, const char *s, size_t n);
void sb_puts(SB *b, const char *s);
void sb_cp(SB *b, u32 c);
Obj *sb_take(void);
void sb_i64(SB *b, i64 v);
void sb_u64(SB *b, u64 v);
void sb_float(SB *b, f64 v, int single);
void sb_dbg_str(SB *b, Obj *o);
void sb_dbg_char(SB *b, u32 c);
void out_sb(int newline);
void out_str(Obj *s, int newline);
void out_nl(void);
Obj *rt_input(void);
Obj *rt_read_line(void);
Obj *rt_args(void);
CONST_FN int cp_is_digit(u32 c);
CONST_FN int cp_is_space(u32 c);
CONST_FN int cp_is_alpha(u32 c);
CONST_FN u32 cp_upper(u32 c);
CONST_FN u32 cp_lower(u32 c);
Obj *str_chars(Obj *o);
Obj *str_upper(Obj *o);
Obj *str_lower(Obj *o);
Obj *str_trim(Obj *o);
Obj *str_split(Obj *o, Obj *sepo);
Obj *str_join(Obj *arr, Obj *sep);
i64 parse_i64(Obj *o);
u64 parse_u64(Obj *o);
f64 parse_f64(Obj *o);
Obj *try_parse_i64(Obj *o);
Obj *try_parse_u64(Obj *o);
Obj *try_parse_f64(Obj *o);
Obj *str_get_char(Obj *o, i64 i);
Obj *str_try_substring(Obj *o, i64 lo, i64 hi);
Obj *str_index_of(Obj *a, Obj *b);
Obj *fmt_value(int kind, W bits, Obj *spec);
NORETURN void rt_sys_exit(i64 code);
Obj *rt_sys_env(Obj *name);
Obj *rt_sys_platform(void);
i64 rt_sys_now_ms(void);
i64 rt_sys_ticks_ms(void);
void rt_sys_sleep(i64 ms);
Obj *rt_fs_read(Obj *path);
Obj *rt_fs_write(Obj *path, Obj *text, int append);
int rt_fs_exists(Obj *path);
int rt_fs_is_file(Obj *path);
int rt_fs_is_dir(Obj *path);
Obj *rt_fs_list(Obj *path);
Obj *rt_fs_create_dir(Obj *path);
Obj *rt_fs_remove(Obj *path);
Obj *rt_fs_rename(Obj *from, Obj *to);
int rt_term_is_tty(void);
int rt_term_raw(int on);
i64 rt_term_width(void);
i64 rt_term_height(void);
Obj *rt_term_key(i64 wait_ms, int poll);

/* ---- hot paths, inlined into the program ----------------------------------------------------- */

static inline W arr_get(Obj *o, i64 i) {
    const Arr *a = ARR(o);
    if (UNLIKELY((u64)i >= a->len)) trap_index(i, a->len);
    return a->a[i];
}

static inline void arr_set(Obj *o, i64 i, W v) {
    Arr *a = ARR(o);
    if (UNLIKELY((u64)i >= a->len)) trap_index(i, a->len);
    a->a[i] = v;
}

static inline CONST_FN int eq_bits(W a, W b) {
    return a == b;
}

static inline CONST_FN int eq_f64(W a, W b) {
    return bits_f64(a) == bits_f64(b);
}

static inline CONST_FN int eq_f32(W a, W b) {
    return bits_f32(a) == bits_f32(b);
}

static inline CONST_FN int cmp_s64(W a, W b) {
    return ((i64)a > (i64)b) - ((i64)a < (i64)b);
}

static inline CONST_FN int cmp_u64(W a, W b) {
    return (a > b) - (a < b);
}

/* Floats: NaN is unordered and sorts last. */
static inline CONST_FN int cmp_f64(W a, W b) {
    f64 x = bits_f64(a), y = bits_f64(b);
    if (x < y) return -1;
    if (x > y) return 1;
    if (x == y) return 0;
    return (isnan(x) != 0) - (isnan(y) != 0);
}

static inline CONST_FN int cmp_f32(W a, W b) {
    f32 x = bits_f32(a), y = bits_f32(b);
    if (x < y) return -1;
    if (x > y) return 1;
    if (x == y) return 0;
    return (isnan(x) != 0) - (isnan(y) != 0);
}

/* ---- checked arithmetic ------------------------------------------------------------------------ */

static inline int chk_add_i8(i8 a, i8 b, i8 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_i16(i16 a, i16 b, i16 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_i32(i32 a, i32 b, i32 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_i64(i64 a, i64 b, i64 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_u8(u8 a, u8 b, u8 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_u16(u16 a, u16 b, u16 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_u32(u32 a, u32 b, u32 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_add_u64(u64 a, u64 b, u64 *r) {
    return !OVF_ADD(a, b, r);
}

static inline int chk_div_i8(i8 a, i8 b, i8 *r) {
    if (b == 0) return 0;
    if (a == INT8_MIN && b == -1) return 0;
    *r = (i8)(a / b);
    return 1;
}

static inline int chk_div_i16(i16 a, i16 b, i16 *r) {
    if (b == 0) return 0;
    if (a == INT16_MIN && b == -1) return 0;
    *r = (i16)(a / b);
    return 1;
}

static inline int chk_div_i32(i32 a, i32 b, i32 *r) {
    if (b == 0) return 0;
    if (a == INT32_MIN && b == -1) return 0;
    *r = (i32)(a / b);
    return 1;
}

static inline int chk_div_i64(i64 a, i64 b, i64 *r) {
    if (b == 0) return 0;
    if (a == INT64_MIN && b == -1) return 0;
    *r = (i64)(a / b);
    return 1;
}

static inline int chk_div_u8(u8 a, u8 b, u8 *r) {
    if (b == 0) return 0;
    *r = (u8)(a / b);
    return 1;
}

static inline int chk_div_u16(u16 a, u16 b, u16 *r) {
    if (b == 0) return 0;
    *r = (u16)(a / b);
    return 1;
}

static inline int chk_div_u32(u32 a, u32 b, u32 *r) {
    if (b == 0) return 0;
    *r = (u32)(a / b);
    return 1;
}

static inline int chk_div_u64(u64 a, u64 b, u64 *r) {
    if (b == 0) return 0;
    *r = (u64)(a / b);
    return 1;
}

static inline int chk_mul_i8(i8 a, i8 b, i8 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_i16(i16 a, i16 b, i16 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_i32(i32 a, i32 b, i32 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_i64(i64 a, i64 b, i64 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_u8(u8 a, u8 b, u8 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_u16(u16 a, u16 b, u16 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_u32(u32 a, u32 b, u32 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_mul_u64(u64 a, u64 b, u64 *r) {
    return !OVF_MUL(a, b, r);
}

static inline int chk_neg_i8(i8 a, i8 *r) {
    if (a == INT8_MIN) return 0;
    *r = (i8)-a;
    return 1;
}

static inline int chk_neg_i16(i16 a, i16 *r) {
    if (a == INT16_MIN) return 0;
    *r = (i16)-a;
    return 1;
}

static inline int chk_neg_i32(i32 a, i32 *r) {
    if (a == INT32_MIN) return 0;
    *r = (i32)-a;
    return 1;
}

static inline int chk_neg_i64(i64 a, i64 *r) {
    if (a == INT64_MIN) return 0;
    *r = (i64)-a;
    return 1;
}

static inline int chk_neg_u8(u8 a, u8 *r) {
    if (a != 0) return 0;
    *r = 0;
    return 1;
}

static inline int chk_neg_u16(u16 a, u16 *r) {
    if (a != 0) return 0;
    *r = 0;
    return 1;
}

static inline int chk_neg_u32(u32 a, u32 *r) {
    if (a != 0) return 0;
    *r = 0;
    return 1;
}

static inline int chk_neg_u64(u64 a, u64 *r) {
    if (a != 0) return 0;
    *r = 0;
    return 1;
}

static inline int chk_pow_i8(i8 base, u64 exp, i8 *r) {
    i8 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_i16(i16 base, u64 exp, i16 *r) {
    i16 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_i32(i32 base, u64 exp, i32 *r) {
    i32 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_i64(i64 base, u64 exp, i64 *r) {
    i64 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_u8(u8 base, u64 exp, u8 *r) {
    u8 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_u16(u16 base, u64 exp, u16 *r) {
    u16 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_u32(u32 base, u64 exp, u32 *r) {
    u32 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_pow_u64(u64 base, u64 exp, u64 *r) {
    u64 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) {
            if (OVF_MUL(result, square, &result)) return 0;
        }
        exp >>= 1;
        if (exp > 0 && OVF_MUL(square, square, &square)) return 0;
    }
    *r = result;
    return 1;
}

static inline int chk_rem_i8(i8 a, i8 b, i8 *r) {
    if (b == 0) return 0;
    if (b == -1) { *r = 0; return 1; }
    *r = (i8)(a % b);
    return 1;
}

static inline int chk_rem_i16(i16 a, i16 b, i16 *r) {
    if (b == 0) return 0;
    if (b == -1) { *r = 0; return 1; }
    *r = (i16)(a % b);
    return 1;
}

static inline int chk_rem_i32(i32 a, i32 b, i32 *r) {
    if (b == 0) return 0;
    if (b == -1) { *r = 0; return 1; }
    *r = (i32)(a % b);
    return 1;
}

static inline int chk_rem_i64(i64 a, i64 b, i64 *r) {
    if (b == 0) return 0;
    if (b == -1) { *r = 0; return 1; }
    *r = (i64)(a % b);
    return 1;
}

static inline int chk_rem_u8(u8 a, u8 b, u8 *r) {
    if (b == 0) return 0;
    *r = (u8)(a % b);
    return 1;
}

static inline int chk_rem_u16(u16 a, u16 b, u16 *r) {
    if (b == 0) return 0;
    *r = (u16)(a % b);
    return 1;
}

static inline int chk_rem_u32(u32 a, u32 b, u32 *r) {
    if (b == 0) return 0;
    *r = (u32)(a % b);
    return 1;
}

static inline int chk_rem_u64(u64 a, u64 b, u64 *r) {
    if (b == 0) return 0;
    *r = (u64)(a % b);
    return 1;
}

static inline int chk_shl_i8(i8 a, i64 n, i8 *r) {
    if ((u64)n >= 8) return 0;
    *r = (i8)((u8)a << n);
    return 1;
}

static inline int chk_shl_i16(i16 a, i64 n, i16 *r) {
    if ((u64)n >= 16) return 0;
    *r = (i16)((u16)a << n);
    return 1;
}

static inline int chk_shl_i32(i32 a, i64 n, i32 *r) {
    if ((u64)n >= 32) return 0;
    *r = (i32)((u32)a << n);
    return 1;
}

static inline int chk_shl_i64(i64 a, i64 n, i64 *r) {
    if ((u64)n >= 64) return 0;
    *r = (i64)((u64)a << n);
    return 1;
}

static inline int chk_shl_u8(u8 a, i64 n, u8 *r) {
    if ((u64)n >= 8) return 0;
    *r = (u8)((u8)a << n);
    return 1;
}

static inline int chk_shl_u16(u16 a, i64 n, u16 *r) {
    if ((u64)n >= 16) return 0;
    *r = (u16)((u16)a << n);
    return 1;
}

static inline int chk_shl_u32(u32 a, i64 n, u32 *r) {
    if ((u64)n >= 32) return 0;
    *r = (u32)((u32)a << n);
    return 1;
}

static inline int chk_shl_u64(u64 a, i64 n, u64 *r) {
    if ((u64)n >= 64) return 0;
    *r = (u64)((u64)a << n);
    return 1;
}

static inline int chk_shr_i8(i8 a, i64 n, i8 *r) {
    if ((u64)n >= 8) return 0;
    *r = (i8)(a >> n);
    return 1;
}

static inline int chk_shr_i16(i16 a, i64 n, i16 *r) {
    if ((u64)n >= 16) return 0;
    *r = (i16)(a >> n);
    return 1;
}

static inline int chk_shr_i32(i32 a, i64 n, i32 *r) {
    if ((u64)n >= 32) return 0;
    *r = (i32)(a >> n);
    return 1;
}

static inline int chk_shr_i64(i64 a, i64 n, i64 *r) {
    if ((u64)n >= 64) return 0;
    *r = (i64)(a >> n);
    return 1;
}

static inline int chk_shr_u8(u8 a, i64 n, u8 *r) {
    if ((u64)n >= 8) return 0;
    *r = (u8)(a >> n);
    return 1;
}

static inline int chk_shr_u16(u16 a, i64 n, u16 *r) {
    if ((u64)n >= 16) return 0;
    *r = (u16)(a >> n);
    return 1;
}

static inline int chk_shr_u32(u32 a, i64 n, u32 *r) {
    if ((u64)n >= 32) return 0;
    *r = (u32)(a >> n);
    return 1;
}

static inline int chk_shr_u64(u64 a, i64 n, u64 *r) {
    if ((u64)n >= 64) return 0;
    *r = (u64)(a >> n);
    return 1;
}

static inline int chk_sub_i8(i8 a, i8 b, i8 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_i16(i16 a, i16 b, i16 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_i32(i32 a, i32 b, i32 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_i64(i64 a, i64 b, i64 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_u8(u8 a, u8 b, u8 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_u16(u16 a, u16 b, u16 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_u32(u32 a, u32 b, u32 *r) {
    return !OVF_SUB(a, b, r);
}

static inline int chk_sub_u64(u64 a, u64 b, u64 *r) {
    return !OVF_SUB(a, b, r);
}

static inline i8 add_i8(i8 a, i8 b) {
#if NEON_CHECKS
    i8 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (i8)((u32)a + (u32)b);
#endif
}

static inline i16 add_i16(i16 a, i16 b) {
#if NEON_CHECKS
    i16 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (i16)((u32)a + (u32)b);
#endif
}

static inline i32 add_i32(i32 a, i32 b) {
#if NEON_CHECKS
    i32 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (i32)((u32)a + (u32)b);
#endif
}

static inline i64 add_i64(i64 a, i64 b) {
#if NEON_CHECKS
    i64 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (i64)((u64)a + (u64)b);
#endif
}

static inline u8 add_u8(u8 a, u8 b) {
#if NEON_CHECKS
    u8 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (u8)((u32)a + (u32)b);
#endif
}

static inline u16 add_u16(u16 a, u16 b) {
#if NEON_CHECKS
    u16 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (u16)((u32)a + (u32)b);
#endif
}

static inline u32 add_u32(u32 a, u32 b) {
#if NEON_CHECKS
    u32 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (u32)((u32)a + (u32)b);
#endif
}

static inline u64 add_u64(u64 a, u64 b) {
#if NEON_CHECKS
    u64 r;
    if (UNLIKELY(OVF_ADD(a, b, &r))) trap_overflow();
    return r;
#else
    return (u64)((u64)a + (u64)b);
#endif
}

static inline i8 sub_i8(i8 a, i8 b) {
#if NEON_CHECKS
    i8 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (i8)((u32)a - (u32)b);
#endif
}

static inline i16 sub_i16(i16 a, i16 b) {
#if NEON_CHECKS
    i16 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (i16)((u32)a - (u32)b);
#endif
}

static inline i32 sub_i32(i32 a, i32 b) {
#if NEON_CHECKS
    i32 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (i32)((u32)a - (u32)b);
#endif
}

static inline i64 sub_i64(i64 a, i64 b) {
#if NEON_CHECKS
    i64 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (i64)((u64)a - (u64)b);
#endif
}

static inline u8 sub_u8(u8 a, u8 b) {
#if NEON_CHECKS
    u8 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (u8)((u32)a - (u32)b);
#endif
}

static inline u16 sub_u16(u16 a, u16 b) {
#if NEON_CHECKS
    u16 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (u16)((u32)a - (u32)b);
#endif
}

static inline u32 sub_u32(u32 a, u32 b) {
#if NEON_CHECKS
    u32 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (u32)((u32)a - (u32)b);
#endif
}

static inline u64 sub_u64(u64 a, u64 b) {
#if NEON_CHECKS
    u64 r;
    if (UNLIKELY(OVF_SUB(a, b, &r))) trap_overflow();
    return r;
#else
    return (u64)((u64)a - (u64)b);
#endif
}

static inline i8 mul_i8(i8 a, i8 b) {
#if NEON_CHECKS
    i8 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (i8)((u32)a * (u32)b);
#endif
}

static inline i16 mul_i16(i16 a, i16 b) {
#if NEON_CHECKS
    i16 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (i16)((u32)a * (u32)b);
#endif
}

static inline i32 mul_i32(i32 a, i32 b) {
#if NEON_CHECKS
    i32 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (i32)((u32)a * (u32)b);
#endif
}

static inline i64 mul_i64(i64 a, i64 b) {
#if NEON_CHECKS
    i64 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (i64)((u64)a * (u64)b);
#endif
}

static inline u8 mul_u8(u8 a, u8 b) {
#if NEON_CHECKS
    u8 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (u8)((u32)a * (u32)b);
#endif
}

static inline u16 mul_u16(u16 a, u16 b) {
#if NEON_CHECKS
    u16 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (u16)((u32)a * (u32)b);
#endif
}

static inline u32 mul_u32(u32 a, u32 b) {
#if NEON_CHECKS
    u32 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (u32)((u32)a * (u32)b);
#endif
}

static inline u64 mul_u64(u64 a, u64 b) {
#if NEON_CHECKS
    u64 r;
    if (UNLIKELY(OVF_MUL(a, b, &r))) trap_overflow();
    return r;
#else
    return (u64)((u64)a * (u64)b);
#endif
}

static inline i8 div_i8(i8 a, i8 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (UNLIKELY(a == INT8_MIN && b == -1)) trap_overflow();
#endif
    return (i8)(a / b);
}

static inline i16 div_i16(i16 a, i16 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (UNLIKELY(a == INT16_MIN && b == -1)) trap_overflow();
#endif
    return (i16)(a / b);
}

static inline i32 div_i32(i32 a, i32 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (UNLIKELY(a == INT32_MIN && b == -1)) trap_overflow();
#endif
    return (i32)(a / b);
}

static inline i64 div_i64(i64 a, i64 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (UNLIKELY(a == INT64_MIN && b == -1)) trap_overflow();
#endif
    return (i64)(a / b);
}

static inline u8 div_u8(u8 a, u8 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u8)(a / b);
}

static inline u16 div_u16(u16 a, u16 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u16)(a / b);
}

static inline u32 div_u32(u32 a, u32 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u32)(a / b);
}

static inline u64 div_u64(u64 a, u64 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u64)(a / b);
}

static inline i8 rem_i8(i8 a, i8 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (b == -1) return 0;  /* MIN % -1 is 0, not an overflow */
#endif
    return (i8)(a % b);
}

static inline i16 rem_i16(i16 a, i16 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (b == -1) return 0;  /* MIN % -1 is 0, not an overflow */
#endif
    return (i16)(a % b);
}

static inline i32 rem_i32(i32 a, i32 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (b == -1) return 0;  /* MIN % -1 is 0, not an overflow */
#endif
    return (i32)(a % b);
}

static inline i64 rem_i64(i64 a, i64 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
    if (b == -1) return 0;  /* MIN % -1 is 0, not an overflow */
#endif
    return (i64)(a % b);
}

static inline u8 rem_u8(u8 a, u8 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u8)(a % b);
}

static inline u16 rem_u16(u16 a, u16 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u16)(a % b);
}

static inline u32 rem_u32(u32 a, u32 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u32)(a % b);
}

static inline u64 rem_u64(u64 a, u64 b) {
#if NEON_CHECKS
    if (UNLIKELY(b == 0)) trap_div0();
#endif
    return (u64)(a % b);
}

static inline i8 shl_i8(i8 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 8)) trap_shift();
#endif
    return (i8)((u8)a << (n & 7));
}

static inline i16 shl_i16(i16 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 16)) trap_shift();
#endif
    return (i16)((u16)a << (n & 15));
}

static inline i32 shl_i32(i32 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 32)) trap_shift();
#endif
    return (i32)((u32)a << (n & 31));
}

static inline i64 shl_i64(i64 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 64)) trap_shift();
#endif
    return (i64)((u64)a << (n & 63));
}

static inline u8 shl_u8(u8 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 8)) trap_shift();
#endif
    return (u8)((u8)a << (n & 7));
}

static inline u16 shl_u16(u16 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 16)) trap_shift();
#endif
    return (u16)((u16)a << (n & 15));
}

static inline u32 shl_u32(u32 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 32)) trap_shift();
#endif
    return (u32)((u32)a << (n & 31));
}

static inline u64 shl_u64(u64 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 64)) trap_shift();
#endif
    return (u64)((u64)a << (n & 63));
}

static inline i8 shr_i8(i8 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 8)) trap_shift();
#endif
    return (i8)(a >> (n & 7));
}

static inline i16 shr_i16(i16 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 16)) trap_shift();
#endif
    return (i16)(a >> (n & 15));
}

static inline i32 shr_i32(i32 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 32)) trap_shift();
#endif
    return (i32)(a >> (n & 31));
}

static inline i64 shr_i64(i64 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 64)) trap_shift();
#endif
    return (i64)(a >> (n & 63));
}

static inline u8 shr_u8(u8 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 8)) trap_shift();
#endif
    return (u8)(a >> (n & 7));
}

static inline u16 shr_u16(u16 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 16)) trap_shift();
#endif
    return (u16)(a >> (n & 15));
}

static inline u32 shr_u32(u32 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 32)) trap_shift();
#endif
    return (u32)(a >> (n & 31));
}

static inline u64 shr_u64(u64 a, i64 n) {
#if NEON_CHECKS
    if (UNLIKELY((u64)n >= 64)) trap_shift();
#endif
    return (u64)(a >> (n & 63));
}

static inline i8 neg_i8(i8 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT8_MIN)) trap_overflow();
#endif
    return (i8)(0 - (u32)a);
}

static inline i16 neg_i16(i16 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT16_MIN)) trap_overflow();
#endif
    return (i16)(0 - (u32)a);
}

static inline i32 neg_i32(i32 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT32_MIN)) trap_overflow();
#endif
    return (i32)(0 - (u32)a);
}

static inline i64 neg_i64(i64 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT64_MIN)) trap_overflow();
#endif
    return (i64)(0 - (u64)a);
}

static inline u8 neg_u8(u8 a) {
#if NEON_CHECKS
    if (UNLIKELY(a != 0)) trap_overflow();
#endif
    return (u8)(0 - (u32)a);
}

static inline u16 neg_u16(u16 a) {
#if NEON_CHECKS
    if (UNLIKELY(a != 0)) trap_overflow();
#endif
    return (u16)(0 - (u32)a);
}

static inline u32 neg_u32(u32 a) {
#if NEON_CHECKS
    if (UNLIKELY(a != 0)) trap_overflow();
#endif
    return (u32)(0 - (u32)a);
}

static inline u64 neg_u64(u64 a) {
#if NEON_CHECKS
    if (UNLIKELY(a != 0)) trap_overflow();
#endif
    return (u64)(0 - (u64)a);
}

static inline i8 abs_i8(i8 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT8_MIN)) trap_overflow();
#endif
    return a < 0 ? (i8)(0 - (u32)a) : a;
}

static inline i16 abs_i16(i16 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT16_MIN)) trap_overflow();
#endif
    return a < 0 ? (i16)(0 - (u32)a) : a;
}

static inline i32 abs_i32(i32 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT32_MIN)) trap_overflow();
#endif
    return a < 0 ? (i32)(0 - (u32)a) : a;
}

static inline i64 abs_i64(i64 a) {
#if NEON_CHECKS
    if (UNLIKELY(a == INT64_MIN)) trap_overflow();
#endif
    return a < 0 ? (i64)(0 - (u64)a) : a;
}

static inline i8 pow_i8(i8 base, u64 exp) {
#if NEON_CHECKS
    i8 r;
    if (UNLIKELY(!chk_pow_i8(base, exp, &r))) trap_overflow();
    return r;
#else
    i8 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (i8)((u32)result * (u32)square);
        exp >>= 1;
        square = (i8)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline i16 pow_i16(i16 base, u64 exp) {
#if NEON_CHECKS
    i16 r;
    if (UNLIKELY(!chk_pow_i16(base, exp, &r))) trap_overflow();
    return r;
#else
    i16 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (i16)((u32)result * (u32)square);
        exp >>= 1;
        square = (i16)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline i32 pow_i32(i32 base, u64 exp) {
#if NEON_CHECKS
    i32 r;
    if (UNLIKELY(!chk_pow_i32(base, exp, &r))) trap_overflow();
    return r;
#else
    i32 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (i32)((u32)result * (u32)square);
        exp >>= 1;
        square = (i32)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline i64 pow_i64(i64 base, u64 exp) {
#if NEON_CHECKS
    i64 r;
    if (UNLIKELY(!chk_pow_i64(base, exp, &r))) trap_overflow();
    return r;
#else
    i64 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (i64)((u64)result * (u64)square);
        exp >>= 1;
        square = (i64)((u64)square * (u64)square);
    }
    return result;
#endif
}

static inline u8 pow_u8(u8 base, u64 exp) {
#if NEON_CHECKS
    u8 r;
    if (UNLIKELY(!chk_pow_u8(base, exp, &r))) trap_overflow();
    return r;
#else
    u8 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (u8)((u32)result * (u32)square);
        exp >>= 1;
        square = (u8)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline u16 pow_u16(u16 base, u64 exp) {
#if NEON_CHECKS
    u16 r;
    if (UNLIKELY(!chk_pow_u16(base, exp, &r))) trap_overflow();
    return r;
#else
    u16 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (u16)((u32)result * (u32)square);
        exp >>= 1;
        square = (u16)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline u32 pow_u32(u32 base, u64 exp) {
#if NEON_CHECKS
    u32 r;
    if (UNLIKELY(!chk_pow_u32(base, exp, &r))) trap_overflow();
    return r;
#else
    u32 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (u32)((u32)result * (u32)square);
        exp >>= 1;
        square = (u32)((u32)square * (u32)square);
    }
    return result;
#endif
}

static inline u64 pow_u64(u64 base, u64 exp) {
#if NEON_CHECKS
    u64 r;
    if (UNLIKELY(!chk_pow_u64(base, exp, &r))) trap_overflow();
    return r;
#else
    u64 result = 1, square = base;
    while (exp > 0) {
        if (exp & 1) result = (u64)((u64)result * (u64)square);
        exp >>= 1;
        square = (u64)((u64)square * (u64)square);
    }
    return result;
#endif
}

static inline i64 f2i_i64(f64 v) {
    if (v != v) return 0;
    if (v >= 9223372036854775808.0) return INT64_MAX;
    if (v <= -9223372036854775808.0) return INT64_MIN;
    return (i64)v;
}

static inline u64 f2i_u64(f64 v) {
    if (!(v > 0)) return 0;
    if (v >= 18446744073709551616.0) return UINT64_MAX;
    return (u64)v;
}

static inline i8 f2i_i8(f64 v) {
    i64 w = f2i_i64(v);
    return w < INT8_MIN ? (i8)INT8_MIN : w > (i64)INT8_MAX ? (i8)INT8_MAX : (i8)w;
}

static inline i16 f2i_i16(f64 v) {
    i64 w = f2i_i64(v);
    return w < INT16_MIN ? (i16)INT16_MIN : w > (i64)INT16_MAX ? (i16)INT16_MAX : (i16)w;
}

static inline i32 f2i_i32(f64 v) {
    i64 w = f2i_i64(v);
    return w < INT32_MIN ? (i32)INT32_MIN : w > (i64)INT32_MAX ? (i32)INT32_MAX : (i32)w;
}

static inline u8 f2i_u8(f64 v) {
    i64 w = f2i_i64(v);
    return w < 0 ? (u8)0 : w > (i64)UINT8_MAX ? (u8)UINT8_MAX : (u8)w;
}

static inline u16 f2i_u16(f64 v) {
    i64 w = f2i_i64(v);
    return w < 0 ? (u16)0 : w > (i64)UINT16_MAX ? (u16)UINT16_MAX : (u16)w;
}

static inline u32 f2i_u32(f64 v) {
    i64 w = f2i_i64(v);
    return w < 0 ? (u32)0 : w > (i64)UINT32_MAX ? (u32)UINT32_MAX : (u32)w;
}

#endif
