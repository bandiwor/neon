/* neon.c - the runtime of programs translated from Neon: allocator and collector, strings, arrays, maps,
 * formatting, text input, and the standard modules sys / fs / term. Build it once and link it with
 * every generated program:  cc -c -O2 neon.c  */

/* the standard modules need POSIX; the feature macro must come before the first include */
#ifndef _WIN32
#define _POSIX_C_SOURCE 200809L
#endif

#include "neon.h"

#include <errno.h>
#include <sys/stat.h>
#ifdef _WIN32
#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <direct.h>
#include <io.h>
#include <conio.h>
#else
#include <dirent.h>
#include <sys/ioctl.h>
#include <sys/select.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>
#endif

/* ---- state ----------------------------------------------------------------------------------- */

/* shared with the generated code (through the macros and the printers) */
Frame *gc_top;                          /* innermost registered frame */
size_t gc_bytes;                        /* bytes allocated since the last collection, plus the survivors */
size_t gc_next = (size_t)4 << 20;       /* collect when gc_bytes reaches this */
unsigned rt_depth;                      /* the recursion guard of checked builds */
SB rt_sb;                               /* scratch buffer of printing and formatting */
int rt_argc;                            /* the arguments of main, for sys.args */
char **rt_argv;

/* private */
static Block *gc_blocks;                /* every block */
static Block *gc_cur[GC_CLASSES + 1];   /* the block being carved, per size class */
static Obj *gc_free[GC_CLASSES + 1];    /* free cells, per size class (linked through the cell body) */
static Big *gc_big;                     /* every big object */
static Obj **gc_stack;                  /* work list of the mark phase */
static size_t gc_nstack, gc_capstack;
static SB fs_tmp;                       /* the body, before padding */
static char fs_err[160];                /* the last error message */

/* ---- private functions ---------------------------------------------------------------------- */

static NORETURN COLD void trap_key(void);
static NORETURN COLD void trap_empty(void);
static NORETURN COLD void trap_oom(void);
static NORETURN COLD void trap_panic_text(const char *msg);
static NORETURN COLD void trap_parse(Obj *s);
static void *rt_malloc(size_t n);
static void *rt_realloc(void *p, size_t n);
static Block *gc_block(size_t cls);
static void gc_scan(Obj *o);
static size_t gc_extern(const Obj *o);
static void gc_finish(Obj *o);
static void gc_sweep(void);
static size_t utf8_dec(const char *p, const char *end, u32 *cp);
static size_t utf8_enc(u32 c, char *out);
static PURE_FN size_t utf8_count(const char *p, size_t n);
static Obj *agg_alloc(u32 tag, u32 n, u64 mask);
static Obj *agg1(u32 tag, W payload, int is_ref);
static Obj *res_ok(W v, int is_ref);
static Obj *res_err(const char *msg);
static Obj *arr_alloc(size_t cap, int is_ref);
static u64 map_hash(const Map *m, W k);
static void map_reindex(Map *m);
static void sb_putc(SB *b, char c);
static void sb_escaped(SB *b, u32 c, u32 quote);
static int io_getline(void);
static void trim_span(const Str *s, const char **b, const char **e);
static int parse_i64_try(Obj *o, i64 *out);
static int parse_u64_try(Obj *o, u64 *out);
static int parse_f64_try(Obj *o, f64 *out);
static Obj *parse_err(Obj *o);
static int fs_align(u32 c);
static long fs_digits(const u32 *c, size_t len, size_t *i);
static const char *fs_read(FSpec *s, const char *p, size_t n);
static const char *fs_unknown(int code, const char *what);
static void fs_pad(const char *sign, const char *prefix, const char *body, size_t blen, const FSpec *s, int dflt);
static const char *fs_sign(int negative, const FSpec *s);
static void fs_group(const char *digits, size_t n, int sep, size_t size);
static const char *fs_int(int neg, u64 mag, const FSpec *s);
static void fs_zeros(char *t);
static char *fs_printf(const char *conv, int prec, f64 v);
static const char *fs_float_spec(f64 v, const FSpec *s);
static const char *fs_float(f64 v, int single, const FSpec *s);
static const char *fs_str(const char *text, size_t n, const FSpec *s);
static const char *host_reason(int e);
static Obj *host_fail(const char *path, const char *reason);
static int host_utf8_ok(const unsigned char *s, size_t n);
static int host_kind(const char *p);
static int host_name_cmp(const void *a, const void *b);
static void term_restore(void);
static int tk_set(const char *s);
static int tk_decode(i64 wait_ms);
static int tk_next(i64 ms);
static void tk_mods(int m, char *out);
static int tk_csi(void);
static int tk_escape(void);
static int tk_decode(i64 wait_ms);

/* ---- implementation -------------------------------------------------------------------------- */

NORETURN COLD PRINTF_FN(1, 2) void rt_fail(const char *fmt, ...) {
    va_list ap;
    fflush(stdout);
    fputs("runtime error: ", stderr);
    va_start(ap, fmt);
    vfprintf(stderr, fmt, ap);
    va_end(ap);
    fputc('\n', stderr);
    exit(2);
}

NORETURN COLD void trap_overflow(void) {
    rt_fail("integer overflow");
}

NORETURN COLD void trap_div0(void) {
    rt_fail("division by zero");
}

NORETURN COLD void trap_shift(void) {
    rt_fail("shift amount out of range");
}

NORETURN COLD void trap_index(i64 index, size_t len) {
    rt_fail("index %lld out of bounds (length %zu)", (long long)index, len);
}

static NORETURN COLD void trap_key(void) {
    rt_fail("key not found in map");
}

static NORETURN COLD void trap_empty(void) {
    rt_fail("pop from an empty array");
}

static NORETURN COLD void trap_oom(void) {
    rt_fail("out of memory");
}

NORETURN COLD void trap_stack(void) {
    rt_fail("stack overflow");
}

NORETURN COLD void trap_unreach(const char *fn) {
    rt_fail("panic: reached unreachable code in `%s`", fn);
}

static NORETURN COLD void trap_panic_text(const char *msg) {
    rt_fail("panic: %s", msg);
}

NORETURN COLD void trap_panic(Obj *msg) {
    rt_fail("panic: %.*s", (int)STR(msg)->len, STR(msg)->s);
}

NORETURN COLD void trap_assert(Obj *msg) {
    if (msg == NULL) rt_fail("panic: assertion failed");
    rt_fail("panic: assertion failed: %.*s", (int)STR(msg)->len, STR(msg)->s);
}

static NORETURN COLD void trap_parse(Obj *s) {
    rt_fail("cannot parse `%.*s`", (int)STR(s)->len, STR(s)->s);
}

static void *rt_malloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (UNLIKELY(p == NULL)) trap_oom();
    return p;
}

static void *rt_realloc(void *p, size_t n) {
    p = realloc(p, n ? n : 1);
    if (UNLIKELY(p == NULL)) trap_oom();
    return p;
}

static Block *gc_block(size_t cls) {
    Block *b = (Block *)rt_malloc(sizeof(Block) + GC_BLOCK);
    b->next = gc_blocks;
    b->cls = cls;
    b->top = 0;
    gc_blocks = b;
    gc_cur[cls] = b;
    return b;
}

Obj *obj_new(size_t size, unsigned kind, unsigned flags) {
    Obj *o;
    size_t cls = (size + 15) >> 4;
    if (LIKELY(cls <= GC_CLASSES)) {
        o = gc_free[cls];
        if (LIKELY(o != NULL)) {
            gc_free[cls] = *(Obj **)((char *)o + 8);
        } else {
            Block *b = gc_cur[cls];
            if (UNLIKELY(b == NULL || b->top + cls * 16 > GC_BLOCK)) b = gc_block(cls);
            o = (Obj *)((char *)(b + 1) + b->top);
            b->top += cls * 16;
        }
        gc_bytes += cls * 16;
    } else {
        Big *g = (Big *)rt_malloc(sizeof(Big) + size);
        g->next = gc_big;
        g->size = size;
        gc_big = g;
        o = (Obj *)(g + 1);
        gc_bytes += size;
    }
    o->kind = (u8)kind;
    o->mark = 0;
    o->flags = (u16)flags;
    o->n = 0;
    return o;
}

void gc_mark(Obj *o) {
    if (o == NULL || o->mark) return;
    o->mark = 1;
    if (o->kind == K_STR) return;
    if (gc_nstack == gc_capstack) {
        gc_capstack = gc_capstack ? gc_capstack * 2 : 256;
        gc_stack = (Obj **)rt_realloc(gc_stack, gc_capstack * sizeof *gc_stack);
    }
    gc_stack[gc_nstack++] = o;
}

static void gc_scan(Obj *o) {
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
}

/* Bytes an object holds outside its cell (array and map buffers). */
static size_t gc_extern(const Obj *o) {
    if (o->kind == K_ARR) return ((const Arr *)o)->cap * sizeof(W);
    if (o->kind == K_MAP) return ((const Map *)o)->cap * 2 * sizeof(W) + ((const Map *)o)->ixcap * sizeof(i32);
    return 0;
}

/* Releases what a dead object owns besides its cell. */
static void gc_finish(Obj *o) {
    if (o->kind == K_ARR) free(ARR(o)->a);
    if (o->kind == K_MAP) { free(MAP(o)->k); free(MAP(o)->v); free(MAP(o)->ix); }
}

/* Frees the unmarked objects and rebuilds the free lists (in address order). */
static void gc_sweep(void) {
    size_t cls, stride, live = 0;
    Block *b;
    Big *g, **link;
    Obj **tail[GC_CLASSES + 1], *o;
    char *p, *end;
    for (cls = 1; cls <= GC_CLASSES; cls++) tail[cls] = &gc_free[cls];
    for (b = gc_blocks; b != NULL; b = b->next) {
        stride = b->cls * 16;
        end = (char *)(b + 1) + b->top;
        for (p = (char *)(b + 1); p < end; p += stride) {
            o = (Obj *)p;
            if (o->kind != K_FREE) {
                if (o->mark) {
                    o->mark = 0;
                    live += stride + gc_extern(o);
                    continue;
                }
                gc_finish(o);
                o->kind = K_FREE;
#ifdef NEON_GC_STRESS   /* make a stale reference fail loudly */
                memset(p + 8, 0xDD, stride - 8);
#endif
            }
            *tail[b->cls] = o;
            tail[b->cls] = (Obj **)(p + 8);
        }
    }
    for (cls = 1; cls <= GC_CLASSES; cls++) *tail[cls] = NULL;
    link = &gc_big;
    while ((g = *link) != NULL) {
        o = (Obj *)(g + 1);
        if (o->mark) {
            o->mark = 0;
            live += g->size + gc_extern(o);
            link = &g->next;
        } else {
            *link = g->next;
            gc_finish(o);
            free(g);
        }
    }
    gc_bytes = live;
}

/* Mark and sweep. Called only at GC_POINT()s, so no runtime function ever sees a collection. */
void gc_collect(void) {
    Frame *f;
    size_t i;
    for (f = gc_top; f != NULL; f = f->up)
        for (i = 0; i < f->n; i++) gc_mark(*f->slots[i]);
    user_roots();
    while (gc_nstack > 0) gc_scan(gc_stack[--gc_nstack]);
    gc_sweep();
    gc_next = gc_bytes * 2 > ((size_t)4 << 20) ? gc_bytes * 2 : ((size_t)4 << 20);
}

CONST_FN W f64_bits(f64 x) {
    W w;
    memcpy(&w, &x, sizeof w);
    return w;
}

CONST_FN f64 bits_f64(W w) {
    f64 x;
    memcpy(&x, &w, sizeof x);
    return x;
}

CONST_FN W f32_bits(f32 x) {
    u32 u;
    memcpy(&u, &x, sizeof u);
    return u;
}

CONST_FN f32 bits_f32(W w) {
    u32 u = (u32)w;
    f32 x;
    memcpy(&x, &u, sizeof x);
    return x;
}

/* Decodes the code point at p; returns the number of bytes it takes (a stray byte counts as one). */
static size_t utf8_dec(const char *p, const char *end, u32 *cp) {
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
}

static size_t utf8_enc(u32 c, char *out) {
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
}

/* Number of code points in n bytes. */
static PURE_FN size_t utf8_count(const char *p, size_t n) {
    size_t i, c = 0;
    for (i = 0; i < n; i++) c += ((unsigned char)p[i] & 0xC0) != 0x80;
    return c;
}

/* The position after n code points (or the end). */
const char *utf8_skip(const char *p, const char *end, size_t n) {
    u32 c;
    while (n > 0 && p < end) {
        p += utf8_dec(p, end, &c);
        n--;
    }
    return p;
}

Obj *str_new(const char *s, size_t n) {
    Str *o = (Str *)obj_new(sizeof(Str) + n + 1, K_STR, 0);
    o->len = n;
    if (n) memcpy(o->s, s, n);
    o->s[n] = 0;
    return &o->h;
}

Obj *str_cat(Obj *a, Obj *b) {
    GC_POINT();
    size_t n = STR(a)->len, m = STR(b)->len;
    Str *o = (Str *)obj_new(sizeof(Str) + n + m + 1, K_STR, 0);
    o->len = n + m;
    memcpy(o->s, STR(a)->s, n);
    memcpy(o->s + n, STR(b)->s, m);
    o->s[n + m] = 0;
    return &o->h;
}

PURE_FN int str_eq(const Obj *a, const Obj *b) {
    return ((const Str *)a)->len == ((const Str *)b)->len &&
           memcmp(((const Str *)a)->s, ((const Str *)b)->s, ((const Str *)a)->len) == 0;
}

/* Byte-wise lexicographic order: -1, 0 or 1. */
PURE_FN int str_cmp(const Obj *a, const Obj *b) {
    size_t n = ((const Str *)a)->len < ((const Str *)b)->len ? ((const Str *)a)->len : ((const Str *)b)->len;
    int c = memcmp(((const Str *)a)->s, ((const Str *)b)->s, n);
    if (c != 0) return c < 0 ? -1 : 1;
    return (((const Str *)a)->len > ((const Str *)b)->len) - (((const Str *)a)->len < ((const Str *)b)->len);
}

/* Length in characters. */
i64 str_len(Obj *o) {
    return (i64)utf8_count(STR(o)->s, STR(o)->len);
}

/* Byte offset of the first occurrence of `sub`, or -1. */
PURE_FN long str_pos(const Obj *a, const Obj *b) {
    const Str *s = (const Str *)a, *t = (const Str *)b;
    size_t i;
    if (t->len > s->len) return -1;
    for (i = 0; i + t->len <= s->len; i++)
        if (memcmp(s->s + i, t->s, t->len) == 0) return (long)i;
    return -1;
}

/* Character index of the first occurrence of `sub`, or -1. */
i64 str_find(Obj *a, Obj *b) {
    long at = str_pos(a, b);
    return at < 0 ? -1 : (i64)utf8_count(STR(a)->s, (size_t)at);
}

int str_starts(Obj *a, Obj *b) {
    return STR(b)->len <= STR(a)->len && memcmp(STR(a)->s, STR(b)->s, STR(b)->len) == 0;
}

int str_ends(Obj *a, Obj *b) {
    return STR(b)->len <= STR(a)->len && memcmp(STR(a)->s + STR(a)->len - STR(b)->len, STR(b)->s, STR(b)->len) == 0;
}

Obj *str_substring(Obj *o, i64 lo, i64 hi) {
    const Str *s = STR(o);
    size_t n = utf8_count(s->s, s->len);
    const char *end = s->s + s->len, *b, *e;
    if (lo < 0 || hi < lo || (u64)hi > n) trap_index(lo < 0 || hi < lo ? lo : hi, n);
    b = utf8_skip(s->s, end, (size_t)lo);
    e = utf8_skip(b, end, (size_t)(hi - lo));
    return str_new(b, (size_t)(e - b));
}

u32 str_char_at(Obj *o, i64 i) {
    const Str *s = STR(o);
    const char *end = s->s + s->len, *p;
    u32 c;
    if (i < 0 || (u64)i >= utf8_count(s->s, s->len)) trap_index(i, utf8_count(s->s, s->len));
    p = utf8_skip(s->s, end, (size_t)i);
    utf8_dec(p, end, &c);
    return c;
}

/* A struct or one variant of an enum: `n` word-sized fields, `mask` marks the references. */
static Obj *agg_alloc(u32 tag, u32 n, u64 mask) {
    u32 i;
    Agg *g = (Agg *)obj_new(sizeof(Agg) + n * sizeof(W), K_AGG, 0);
    g->h.n = n;
    g->mask = mask;
    g->h.flags = (u16)tag;
    for (i = 0; i < n; i++) g->f[i] = 0;
    return &g->h;
}

/* What the program calls: the collector may run here, before the new object exists. */
Obj *agg_new(u32 tag, u32 n, u64 mask) {
    GC_POINT();
    return agg_alloc(tag, n, mask);
}

static Obj *agg1(u32 tag, W payload, int is_ref) {
    Obj *o = agg_alloc(tag, 1, is_ref ? 1u : 0u);
    AGG(o)->f[0] = payload;
    return o;
}

/* Option: Some = 0, None = 1. */
Obj *opt_some(W v, int is_ref) {
    return agg1(0, v, is_ref);
}

Obj *opt_none(void) {
    return agg_alloc(1, 0, 0);
}

/* Result: Ok = 0, Err = 1. */
static Obj *res_ok(W v, int is_ref) {
    return agg1(0, v, is_ref);
}

static Obj *res_err(const char *msg) {
    return agg1(1, O2W(str_new(msg, strlen(msg))), 1);
}

static Obj *arr_alloc(size_t cap, int is_ref) {
    Arr *a = (Arr *)obj_new(sizeof(Arr), K_ARR, is_ref ? 1u : 0u);
    a->len = 0;
    a->cap = cap;
    a->a = cap ? (W *)rt_malloc(cap * sizeof(W)) : NULL;
    gc_bytes += cap * sizeof(W);
    return &a->h;
}

/* What the program calls: the collector may run here, before the new object exists. */
Obj *arr_new(size_t cap, int is_ref) {
    GC_POINT();
    return arr_alloc(cap, is_ref);
}

void arr_push(Obj *o, W v) {
    Arr *a = ARR(o);
    if (a->len == a->cap) {
        size_t cap = a->cap ? a->cap * 2 : 4;
        a->a = (W *)rt_realloc(a->a, cap * sizeof(W));
        gc_bytes += (cap - a->cap) * sizeof(W);
        a->cap = cap;
    }
    a->a[a->len++] = v;
}

W arr_pop(Obj *o) {
    Arr *a = ARR(o);
    if (a->len == 0) trap_empty();
    return a->a[--a->len];
}

void arr_insert(Obj *o, i64 i, W v) {
    Arr *a = ARR(o);
    if (i < 0 || (u64)i > a->len) trap_index(i, a->len);
    arr_push(o, 0);
    memmove(a->a + i + 1, a->a + i, (a->len - 1 - (size_t)i) * sizeof(W));
    a->a[i] = v;
}

W arr_remove(Obj *o, i64 i) {
    Arr *a = ARR(o);
    W v;
    if (i < 0 || (u64)i >= a->len) trap_index(i, a->len);
    v = a->a[i];
    memmove(a->a + i, a->a + i + 1, (a->len - 1 - (size_t)i) * sizeof(W));
    a->len--;
    return v;
}

void arr_reverse(Obj *o) {
    Arr *a = ARR(o);
    size_t i;
    for (i = 0; i < a->len / 2; i++) {
        W t = a->a[i];
        a->a[i] = a->a[a->len - 1 - i];
        a->a[a->len - 1 - i] = t;
    }
}

Obj *arr_clone(Obj *o) {
    Obj *r = arr_alloc(ARR(o)->len, ARR(o)->h.flags & 1);
    if (ARR(o)->len) memcpy(ARR(r)->a, ARR(o)->a, ARR(o)->len * sizeof(W));
    ARR(r)->len = ARR(o)->len;
    return r;
}

int arr_contains(Obj *o, W x, int (*eq)(W, W)) {
    const Arr *a = ARR(o);
    size_t i;
    for (i = 0; i < a->len; i++)
        if (eq(a->a[i], x)) return 1;
    return 0;
}

/* A stable merge sort; `cmp` returns <0, 0 or >0. */
void arr_sort(Obj *o, int (*cmp)(W, W)) {
    Arr *a = ARR(o);
    size_t n = a->len, w, lo, mid, hi, i, j, k;
    W *orig = a->a, *src = a->a, *dst, *t;
    if (n < 2) return;
    dst = (W *)rt_malloc(n * sizeof(W));
    for (w = 1; w < n; w *= 2) {
        for (lo = 0; lo < n; lo += 2 * w) {
            mid = lo + w < n ? lo + w : n;
            hi = lo + 2 * w < n ? lo + 2 * w : n;
            i = lo; j = mid; k = lo;
            while (i < mid && j < hi) dst[k++] = cmp(src[j], src[i]) < 0 ? src[j++] : src[i++];
            while (i < mid) dst[k++] = src[i++];
            while (j < hi) dst[k++] = src[j++];
        }
        t = src; src = dst; dst = t;
    }
    if (src != orig) memcpy(orig, src, n * sizeof(W));
    free(src == orig ? dst : src);
}

Obj *arr_try_get(Obj *o, i64 i) {
    const Arr *a = ARR(o);
    if (i < 0 || (u64)i >= a->len) return opt_none();
    return opt_some(a->a[i], a->h.flags & 1);
}

Obj *arr_try_pop(Obj *o) {
    Arr *a = ARR(o);
    if (a->len == 0) return opt_none();
    return opt_some(a->a[--a->len], a->h.flags & 1);
}

Obj *arr_try_remove(Obj *o, i64 i) {
    const Arr *a = ARR(o);
    if (i < 0 || (u64)i >= a->len) return opt_none();
    return opt_some(arr_remove(o, i), a->h.flags & 1);
}

PURE_FN int eq_str(W a, W b) {
    return str_eq(W2O(a), W2O(b));
}

PURE_FN int cmp_str(W a, W b) {
    return str_cmp(W2O(a), W2O(b));
}

/* Maps and sets keep their entries in insertion order; `ix` is an open-addressing hash index. */
Obj *map_new(unsigned flags) {
    GC_POINT();
    Map *m = (Map *)obj_new(sizeof(Map), K_MAP, flags);
    m->len = m->cap = m->ixcap = 0;
    m->k = m->v = NULL;
    m->ix = NULL;
    return &m->h;
}

static u64 map_hash(const Map *m, W k) {
    if (m->h.flags & MF_KSTR) {
        const Str *s = (const Str *)W2O(k);
        u64 h = 1469598103934665603ULL;
        size_t i;
        for (i = 0; i < s->len; i++) h = (h ^ (unsigned char)s->s[i]) * 1099511628211ULL;
        return h;
    }
    k ^= k >> 33; k *= 0xff51afd7ed558ccdULL;
    k ^= k >> 33; k *= 0xc4ceb9fe1a85ec53ULL;
    k ^= k >> 33;
    return k;
}

/* The entry index of `key`, or -1. */
long map_find(Obj *o, W key) {
    const Map *m = MAP(o);
    size_t mask, h;
    i32 e;
    if (m->len == 0) return -1;
    mask = m->ixcap - 1;
    h = (size_t)map_hash(m, key) & mask;
    while ((e = m->ix[h]) >= 0) {
        if ((m->h.flags & MF_KSTR) ? str_eq(W2O(m->k[e]), W2O(key)) : m->k[e] == key) return e;
        h = (h + 1) & mask;
    }
    return -1;
}

static void map_reindex(Map *m) {
    size_t i, c = 8, mask, h;
    while (c < m->cap * 2) c *= 2;
    if (c != m->ixcap) {
        m->ix = (i32 *)rt_realloc(m->ix, c * sizeof(i32));
        gc_bytes += (c > m->ixcap ? c - m->ixcap : 0) * sizeof(i32);
        m->ixcap = c;
    }
    for (i = 0; i < c; i++) m->ix[i] = -1;
    mask = c - 1;
    for (i = 0; i < m->len; i++) {
        h = (size_t)map_hash(m, m->k[i]) & mask;
        while (m->ix[h] >= 0) h = (h + 1) & mask;
        m->ix[h] = (i32)i;
    }
}

/* Inserts or overwrites; returns 1 when the key is new. */
int map_put(Obj *o, W key, W val) {
    Map *m = MAP(o);
    long at = map_find(o, key);
    size_t mask, h;
    if (at >= 0) {
        if (!(m->h.flags & MF_SET)) m->v[at] = val;
        return 0;
    }
    if (m->len == m->cap) {
        size_t cap = m->cap ? m->cap * 2 : 4;
        m->k = (W *)rt_realloc(m->k, cap * sizeof(W));
        if (!(m->h.flags & MF_SET)) m->v = (W *)rt_realloc(m->v, cap * sizeof(W));
        gc_bytes += (cap - m->cap) * 2 * sizeof(W);
        m->cap = cap;
        map_reindex(m);
    }
    m->k[m->len] = key;
    if (!(m->h.flags & MF_SET)) m->v[m->len] = val;
    mask = m->ixcap - 1;
    h = (size_t)map_hash(m, key) & mask;
    while (m->ix[h] >= 0) h = (h + 1) & mask;
    m->ix[h] = (i32)m->len;
    m->len++;
    return 1;
}

W map_get(Obj *o, W key) {
    long at = map_find(o, key);
    if (at < 0) trap_key();
    return MAP(o)->v[at];
}

W map_get_or(Obj *o, W key, W dflt) {
    long at = map_find(o, key);
    return at < 0 ? dflt : MAP(o)->v[at];
}

Obj *map_try_get(Obj *o, W key) {
    long at = map_find(o, key);
    if (at < 0) return opt_none();
    return opt_some(MAP(o)->v[at], (MAP(o)->h.flags & MF_VREF) != 0);
}

/* Removes a key, keeping the order of the others; returns 1 when it was present. */
int map_del(Obj *o, W key) {
    Map *m = MAP(o);
    long at = map_find(o, key);
    if (at < 0) return 0;
    memmove(m->k + at, m->k + at + 1, (m->len - 1 - (size_t)at) * sizeof(W));
    if (!(m->h.flags & MF_SET)) memmove(m->v + at, m->v + at + 1, (m->len - 1 - (size_t)at) * sizeof(W));
    m->len--;
    map_reindex(m);
    return 1;
}

void map_clear(Obj *o) {
    Map *m = MAP(o);
    size_t i;
    m->len = 0;
    for (i = 0; i < m->ixcap; i++) m->ix[i] = -1;
}

Obj *map_keys(Obj *o) {
    GC_POINT();
    const Map *m = MAP(o);
    Obj *r = arr_alloc(m->len, (m->h.flags & MF_KREF) != 0);
    if (m->len) memcpy(ARR(r)->a, m->k, m->len * sizeof(W));
    ARR(r)->len = m->len;
    return r;
}

Obj *map_values(Obj *o) {
    const Map *m = MAP(o);
    Obj *r = arr_alloc(m->len, (m->h.flags & MF_VREF) != 0);
    if (m->len) memcpy(ARR(r)->a, m->v, m->len * sizeof(W));
    ARR(r)->len = m->len;
    return r;
}

/* A closure: its code and the cells of the variables it captured. */
Obj *clo_new(void (*fn)(void), u32 n) {
    GC_POINT();
    u32 i;
    Clo *c = (Clo *)obj_new(sizeof(Clo) + n * sizeof(Obj *), K_CLO, 0);
    c->h.n = n;
    c->fn = fn;
    for (i = 0; i < n; i++) c->c[i] = NULL;
    return &c->h;
}

/* A captured variable lives in a heap cell shared by the function and its closures. */
Obj *cell_new(int is_ref, W v) {
    GC_POINT();
    Cell *c = (Cell *)obj_new(sizeof(Cell), K_CELL, is_ref ? 1u : 0u);
    c->v = v;
    return &c->h;
}

void sb_put(SB *b, const char *s, size_t n) {
    if (b->len + n + 1 > b->cap) {
        size_t cap = b->cap ? b->cap : 64;
        while (cap < b->len + n + 1) cap *= 2;
        b->p = (char *)rt_realloc(b->p, cap);
        b->cap = cap;
    }
    if (n) memcpy(b->p + b->len, s, n);
    b->len += n;
    b->p[b->len] = 0;
}

void sb_puts(SB *b, const char *s) {
    sb_put(b, s, strlen(s));
}

static void sb_putc(SB *b, char c) {
    sb_put(b, &c, 1);
}

/* Appends a code point as UTF-8. */
void sb_cp(SB *b, u32 c) {
    char buf[4];
    sb_put(b, buf, utf8_enc(c, buf));
}

/* The scratch buffer's text as a string object; the buffer is emptied. */
Obj *sb_take(void) {
    Obj *s = str_new(rt_sb.p ? rt_sb.p : "", rt_sb.len);
    rt_sb.len = 0;
    return s;
}

void sb_i64(SB *b, i64 v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%lld", (long long)v);
    sb_put(b, buf, (size_t)n);
}

void sb_u64(SB *b, u64 v) {
    char buf[32];
    int n = snprintf(buf, sizeof buf, "%llu", (unsigned long long)v);
    sb_put(b, buf, (size_t)n);
}

/* The shortest text that reads back as the same number, laid out like `1.0`, `0.1`, `1e16`, `NaN`, `inf`. */
void sb_float(SB *b, f64 v, int single) {
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
        if (single ? (strtof(buf, NULL) == (f32)v) : (strtod(buf, NULL) == v)) break;
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
}

/* One character of a quoted string or character literal (`quote` is the delimiter being used). */
static void sb_escaped(SB *b, u32 c, u32 quote) {
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
}

/* A string inside a collection: quoted and escaped. */
void sb_dbg_str(SB *b, Obj *o) {
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    u32 c;
    sb_putc(b, '"');
    while (p < end) {
        p += utf8_dec(p, end, &c);
        sb_escaped(b, c, '"');
    }
    sb_putc(b, '"');
}

void sb_dbg_char(SB *b, u32 c) {
    sb_putc(b, '\'');
    sb_escaped(b, c, '\'');
    sb_putc(b, '\'');
}

/* Writes the scratch buffer to stdout (and a newline) and empties it. */
void out_sb(int newline) {
    if (rt_sb.len) fwrite(rt_sb.p, 1, rt_sb.len, stdout);
    if (newline) putc('\n', stdout);
    rt_sb.len = 0;
}

void out_str(Obj *s, int newline) {
    if (STR(s)->len) fwrite(STR(s)->s, 1, STR(s)->len, stdout);
    if (newline) putc('\n', stdout);
}

void out_nl(void) {
    putc('\n', stdout);
}

/* Reads a line into the scratch buffer without its line terminator; 0 at the end of the input. */
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
}

/* input(): the next line, or an empty string at the end of the input. */
Obj *rt_input(void) {
    io_getline();
    return sb_take();
}

/* read_line(): Some(line), or None at the end of the input. */
Obj *rt_read_line(void) {
    if (!io_getline()) return opt_none();
    return opt_some(O2W(sb_take()), 1);
}

/* sys.args: the command-line arguments without the program name. */
Obj *rt_args(void) {
    int i;
    Obj *a = arr_alloc(rt_argc > 1 ? (size_t)rt_argc - 1 : 0, 1);
    for (i = 1; i < rt_argc; i++) arr_push(a, O2W(str_new(rt_argv[i], strlen(rt_argv[i]))));
    return a;
}

CONST_FN int cp_is_digit(u32 c) {
    return c >= '0' && c <= '9';
}

/* Unicode White_Space. */
CONST_FN int cp_is_space(u32 c) {
    return (c >= 9 && c <= 13) || c == 32 || c == 0x85 || c == 0xA0 || c == 0x1680 || (c >= 0x2000 && c <= 0x200A) ||
           c == 0x2028 || c == 0x2029 || c == 0x202F || c == 0x205F || c == 0x3000;
}

/* Letters of the common scripts (Latin, Greek, Cyrillic, Armenian, Hebrew, Arabic, Thai, kana, CJK, Hangul). */
CONST_FN int cp_is_alpha(u32 c) {
    if (c < 0x80) return (c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z');
    return c == 0xAA || c == 0xB5 || c == 0xBA || (c >= 0xC0 && c <= 0x2C1 && c != 0xD7 && c != 0xF7) ||
           (c >= 0x370 && c <= 0x373) || (c >= 0x376 && c <= 0x377) || (c >= 0x37B && c <= 0x37D) || c == 0x386 ||
           (c >= 0x388 && c <= 0x3FF && c != 0x3F6) || (c >= 0x400 && c <= 0x481) || (c >= 0x48A && c <= 0x52F) ||
           (c >= 0x531 && c <= 0x556) || (c >= 0x561 && c <= 0x587) || (c >= 0x5D0 && c <= 0x5EA) ||
           (c >= 0x620 && c <= 0x64A) || (c >= 0xE01 && c <= 0xE30) || (c >= 0x1E00 && c <= 0x1FFF) ||
           (c >= 0x3041 && c <= 0x3096) || (c >= 0x30A1 && c <= 0x30FA) || (c >= 0x3400 && c <= 0x4DBF) ||
           (c >= 0x4E00 && c <= 0x9FFF) || (c >= 0xAC00 && c <= 0xD7A3) || (c >= 0xFF21 && c <= 0xFF3A) ||
           (c >= 0xFF41 && c <= 0xFF5A);
}

/* Upper case of a code point: ASCII, Latin-1, Latin Extended-A, Greek and Cyrillic. */
CONST_FN u32 cp_upper(u32 c) {
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
}

CONST_FN u32 cp_lower(u32 c) {
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
}

Obj *str_chars(Obj *o) {
    GC_POINT();
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    Obj *r = arr_alloc(STR(o)->len, 0);
    u32 c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        ARR(r)->a[ARR(r)->len++] = c;
    }
    return r;
}

Obj *str_upper(Obj *o) {
    GC_POINT();
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    u32 c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        if (c == 0xDF) sb_puts(&rt_sb, "SS"); else sb_cp(&rt_sb, cp_upper(c));
    }
    return sb_take();
}

Obj *str_lower(Obj *o) {
    GC_POINT();
    const char *p = STR(o)->s, *end = p + STR(o)->len;
    u32 c;
    while (p < end) {
        p += utf8_dec(p, end, &c);
        sb_cp(&rt_sb, cp_lower(c));
    }
    return sb_take();
}

/* The part of the string without leading and trailing white space. */
static void trim_span(const Str *s, const char **b, const char **e) {
    const char *p = s->s, *end = s->s + s->len, *q;
    u32 c;
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
}

Obj *str_trim(Obj *o) {
    GC_POINT();
    const char *b, *e;
    trim_span(STR(o), &b, &e);
    return str_new(b, (size_t)(e - b));
}

/* Pieces between separators; an empty separator splits into characters. */
Obj *str_split(Obj *o, Obj *sepo) {
    GC_POINT();
    const Str *s = STR(o), *sep = STR(sepo);
    const char *p = s->s, *end = s->s + s->len, *q;
    Obj *r = arr_alloc(4, 1);
    u32 c;
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
}

Obj *str_join(Obj *arr, Obj *sep) {
    GC_POINT();
    const Arr *a = ARR(arr);
    size_t i;
    for (i = 0; i < a->len; i++) {
        if (i) sb_put(&rt_sb, STR(sep)->s, STR(sep)->len);
        sb_put(&rt_sb, STR(W2O(a->a[i]))->s, STR(W2O(a->a[i]))->len);
    }
    return sb_take();
}

/* Decimal i64 with an optional sign; surrounding white space is ignored. 1 on success. */
static int parse_i64_try(Obj *o, i64 *out) {
    const char *b, *e;
    int neg = 0;
    u64 v = 0;
    trim_span(STR(o), &b, &e);
    if (b < e && (*b == '+' || *b == '-')) { neg = *b == '-'; b++; }
    if (b == e) return 0;
    for (; b < e; b++) {
        unsigned d = (unsigned char)*b - '0';
        if (d > 9 || v > (UINT64_MAX - d) / 10) return 0;
        v = v * 10 + d;
    }
    if (neg ? v > (u64)INT64_MAX + 1 : v > (u64)INT64_MAX) return 0;
    *out = neg ? (i64)(0 - v) : (i64)v;
    return 1;
}

/* Decimal u64; a leading `+` is accepted, a `-` is not. */
static int parse_u64_try(Obj *o, u64 *out) {
    const char *b, *e;
    u64 v = 0;
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
}

/* Decimal floating point (`1`, `-2.5`, `.5`, `1e9`, `inf`, `NaN`); no hex, no stray characters. */
static int parse_f64_try(Obj *o, f64 *out) {
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
}

i64 parse_i64(Obj *o) {
    i64 v;
    if (!parse_i64_try(o, &v)) trap_parse(o);
    return v;
}

u64 parse_u64(Obj *o) {
    u64 v;
    if (!parse_u64_try(o, &v)) trap_parse(o);
    return v;
}

f64 parse_f64(Obj *o) {
    f64 v;
    if (!parse_f64_try(o, &v)) trap_parse(o);
    return v;
}

static Obj *parse_err(Obj *o) {
    char *msg = (char *)rt_malloc(STR(o)->len + 32);
    Obj *r;
    sprintf(msg, "cannot parse `%.*s`", (int)STR(o)->len, STR(o)->s);
    r = res_err(msg);
    free(msg);
    return r;
}

Obj *try_parse_i64(Obj *o) {
    i64 v;
    return parse_i64_try(o, &v) ? res_ok((W)v, 0) : parse_err(o);
}

Obj *try_parse_u64(Obj *o) {
    u64 v;
    return parse_u64_try(o, &v) ? res_ok(v, 0) : parse_err(o);
}

Obj *try_parse_f64(Obj *o) {
    f64 v;
    return parse_f64_try(o, &v) ? res_ok(f64_bits(v), 0) : parse_err(o);
}

Obj *str_get_char(Obj *o, i64 i) {
    const Str *s = STR(o);
    const char *end = s->s + s->len, *p;
    u32 c;
    if (i < 0 || (u64)i >= utf8_count(s->s, s->len)) return opt_none();
    p = utf8_skip(s->s, end, (size_t)i);
    utf8_dec(p, end, &c);
    return opt_some(c, 0);
}

Obj *str_try_substring(Obj *o, i64 lo, i64 hi) {
    size_t n = utf8_count(STR(o)->s, STR(o)->len);
    char msg[96];
    if (lo < 0 || hi < lo || (u64)hi > n) {
        snprintf(msg, sizeof msg, "index %lld out of bounds (length %zu)", (long long)(lo < 0 || hi < lo ? lo : hi), n);
        return res_err(msg);
    }
    return res_ok(O2W(str_substring(o, lo, hi)), 1);
}

Obj *str_index_of(Obj *a, Obj *b) {
    i64 at = str_find(a, b);
    return at < 0 ? opt_none() : opt_some((W)at, 0);
}

static int fs_align(u32 c) {
    return c == '<' || c == '>' || c == '^' || c == '=';
}

/* Reads a run of decimal digits; -1 when there is none (saturates for absurd values). */
static long fs_digits(const u32 *c, size_t len, size_t *i) {
    size_t start = *i;
    unsigned long v = 0;
    while (*i < len && c[*i] >= '0' && c[*i] <= '9') {
        v = v * 10 + (c[*i] - '0');
        if (v > (1ul << 24)) v = 1ul << 24;
        (*i)++;
    }
    return *i > start ? (long)v : -1;
}

/* Parses a specification; returns NULL, or a message saying what is wrong with it. */
static const char *fs_read(FSpec *s, const char *p, size_t n) {
    const char *start = p, *end = p + n, *err = NULL;
    u32 *c = (u32 *)rt_malloc((n + 1) * sizeof *c);
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
}

static const char *fs_unknown(int code, const char *what) {
    char ch[5];
    ch[utf8_enc((u32)code, ch)] = 0;
    snprintf(fs_err, sizeof fs_err, "unknown format code `%s` for %s", ch, what);
    return fs_err;
}

/* Appends sign + prefix + body to the scratch buffer, padded to the width. */
static void fs_pad(const char *sign, const char *prefix, const char *body, size_t blen, const FSpec *s, int dflt) {
    u32 fill = s->fill;
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
}

static const char *fs_sign(int negative, const FSpec *s) {
    if (negative) return "-";
    return s->sign == '+' ? "+" : s->sign == ' ' ? " " : "";
}

/* Appends `digits` to the temporary buffer with `sep` between groups of `size`, counting from the right. */
static void fs_group(const char *digits, size_t n, int sep, size_t size) {
    size_t i;
    for (i = 0; i < n; i++) {
        if (i > 0 && (n - i) % size == 0) sb_putc(&fs_tmp, (char)sep);
        sb_putc(&fs_tmp, digits[i]);
    }
}

/* An integer given as a sign and a magnitude. */
static const char *fs_int(int neg, u64 mag, const FSpec *s) {
    int code = s->ty ? s->ty : 'd';
    char digits[72], *d = digits + sizeof digits;
    const char *prefix = "";
    size_t base = 10, size = 3, n;
    if (strchr("eEfFgG%", code)) return fs_float_spec(neg ? -(f64)mag : (f64)mag, s);
    if (!strchr("dnboxXc", code)) return fs_unknown(code, "an integer");
    if (s->prec >= 0) return "a precision is not allowed for integers";
    if (code == 'c') {
        char ch[5];
        if (s->sign != '-' || s->alt || s->grouping) return "sign, `#` and grouping are not allowed with the type `c`";
        if (neg || mag > 0x10FFFF || (mag >= 0xD800 && mag <= 0xDFFF)) return "value is not a valid character";
        fs_pad("", "", ch, utf8_enc((u32)mag, ch), s, '<');
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
}

/* "1.500" -> "1.5", "2.000" -> "2"; text without a point is left alone. */
static void fs_zeros(char *t) {
    size_t n;
    if (!strchr(t, '.')) return;
    n = strlen(t);
    while (n > 0 && t[n - 1] == '0') t[--n] = 0;
    if (n > 0 && t[n - 1] == '.') t[--n] = 0;
}

/* snprintf("%.*<conv>") into a fresh buffer. */
static char *fs_printf(const char *conv, int prec, f64 v) {
    int n = snprintf(NULL, 0, conv, prec, v);
    char *buf = (char *)rt_malloc((size_t)n + 1);
    snprintf(buf, (size_t)n + 1, conv, prec, v);
    return buf;
}

/* A float formatted with an explicit type (or a precision). */
static const char *fs_float_spec(f64 v, const FSpec *s) {
    int code = s->ty ? s->ty : 'g';
    int upper = code >= 'A' && code <= 'Z', neg;
    f64 a;
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
}

/* A float formatted without type and precision prints like `print`, padded and grouped. */
static const char *fs_float(f64 v, int single, const FSpec *s) {
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
}

static const char *fs_str(const char *text, size_t n, const FSpec *s) {
    if (s->ty && s->ty != 's' && s->ty != 'c') return fs_unknown(s->ty, "a string");
    if (s->sign != '-') return "a sign is not allowed in a string format specifier";
    if (s->alt) return "`#` is not allowed in a string format specifier";
    if (s->grouping) return "grouping is not allowed in a string format specifier";
    if (s->align == '=') return "`=` alignment is not allowed in a string format specifier";
    if (s->prec >= 0) n = (size_t)(utf8_skip(text, text + n, (size_t)s->prec) - text);
    fs_pad("", "", text, n, s, '<');
    return NULL;
}

/* x.format(spec) and the fields of format strings. `bits` is the canonical word of the value. */
Obj *fmt_value(int kind, W bits, Obj *spec) {
    FSpec s;
    char msg[240], ch[5];
    const char *err = fs_read(&s, STR(spec)->s, STR(spec)->len);
    rt_sb.len = 0;
    if (!err) {
        int intcode = s.ty == 'd' || s.ty == 'b' || s.ty == 'o' || s.ty == 'x' || s.ty == 'X' || s.ty == 'n';
        switch (kind) {
        case FK_SINT: err = fs_int((i64)bits < 0, (i64)bits < 0 ? 0 - bits : bits, &s); break;
        case FK_UINT: err = fs_int(0, bits, &s); break;
        case FK_F64:  err = fs_float(bits_f64(bits), 0, &s); break;
        case FK_F32:  err = fs_float((f64)bits_f32(bits), 1, &s); break;
        case FK_STR:  err = fs_str(STR(W2O(bits))->s, STR(W2O(bits))->len, &s); break;
        case FK_BOOL:
            if (intcode) err = fs_int(0, bits != 0, &s);
            else err = fs_str(bits ? "true" : "false", bits ? 4 : 5, &s);
            break;
        default:
            if (intcode) err = fs_int(0, (u32)bits, &s);
            else { err = fs_str(ch, utf8_enc((u32)bits, ch), &s); }
            break;
        }
    }
    if (err) {
        snprintf(msg, sizeof msg, "invalid format specifier: %s", err);
        trap_panic_text(msg);
    }
    return sb_take();
}

/* sys.exit(code): the output is flushed, the terminal is restored by its atexit handler. */
NORETURN void rt_sys_exit(i64 code) {
    fflush(stdout);
    exit((int)code);
}

Obj *rt_sys_env(Obj *name) {
    const char *v = getenv(STR(name)->s);
    return v ? opt_some(O2W(str_new(v, strlen(v))), 1) : opt_none();
}

Obj *rt_sys_platform(void) {
#if defined(_WIN32)
    return str_new("windows", 7);
#elif defined(__APPLE__)
    return str_new("macos", 5);
#elif defined(__linux__)
    return str_new("linux", 5);
#elif defined(__FreeBSD__)
    return str_new("freebsd", 7);
#else
    return str_new("unknown", 7);
#endif
}

/* Milliseconds since 1970-01-01 UTC. */
i64 rt_sys_now_ms(void) {
#ifdef _WIN32
    FILETIME ft;
    u64 t;
    GetSystemTimeAsFileTime(&ft);
    t = ((u64)ft.dwHighDateTime << 32) | ft.dwLowDateTime;
    return (i64)(t / 10000u) - 11644473600000;
#else
    struct timespec ts;
    clock_gettime(CLOCK_REALTIME, &ts);
    return (i64)ts.tv_sec * 1000 + (i64)(ts.tv_nsec / 1000000);
#endif
}

/* Milliseconds since the first call. */
i64 rt_sys_ticks_ms(void) {
    static i64 start = -1;
    i64 now;
#ifdef _WIN32
    now = (i64)GetTickCount64();
#else
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    now = (i64)ts.tv_sec * 1000 + (i64)(ts.tv_nsec / 1000000);
#endif
    if (start < 0) start = now;
    return now - start;
}

void rt_sys_sleep(i64 ms) {
    fflush(stdout);
    if (ms <= 0) return;
#ifdef _WIN32
    Sleep((DWORD)ms);
#else
    {
        struct timespec ts;
        ts.tv_sec = (time_t)(ms / 1000);
        ts.tv_nsec = (long)(ms % 1000) * 1000000L;
        nanosleep(&ts, NULL);
    }
#endif
}

/* What an operating system error is called in a program's output: the same words on every platform. */
static const char *host_reason(int e) {
    switch (e) {
    case ENOENT: return "not found";
    case EACCES:
    case EPERM: return "permission denied";
    case EEXIST: return "already exists";
    case ENOTDIR: return "not a directory";
    case EISDIR: return "is a directory";
    case ENOTEMPTY: return "directory not empty";
    default: return "i/o error";
    }
}

/* Err("path: reason") */
static Obj *host_fail(const char *path, const char *reason) {
    size_t n = strlen(path) + strlen(reason) + 3;
    char *msg = (char *)rt_malloc(n);
    Obj *r;
    snprintf(msg, n, "%s: %s", path, reason);
    r = res_err(msg);
    free(msg);
    return r;
}

/* Does `s` hold well-formed UTF-8? */
static int host_utf8_ok(const unsigned char *s, size_t n) {
    size_t i = 0;
    while (i < n) {
        unsigned char c = s[i];
        size_t len = c < 0x80 ? 1 : (c >= 0xc2 && c <= 0xdf) ? 2 : (c >= 0xe0 && c <= 0xef) ? 3 : (c >= 0xf0 && c <= 0xf4) ? 4 : 0;
        size_t k;
        u32 cp;
        if (len == 0 || i + len > n) return 0;
        cp = len == 1 ? c : len == 2 ? (u32)(c & 0x1f) : len == 3 ? (u32)(c & 0x0f) : (u32)(c & 0x07);
        for (k = 1; k < len; k++) {
            if ((s[i + k] & 0xc0) != 0x80) return 0;
            cp = (cp << 6) | (u32)(s[i + k] & 0x3f);
        }
        if ((len == 3 && cp < 0x800) || (len == 4 && (cp < 0x10000 || cp > 0x10ffff)) || (cp >= 0xd800 && cp <= 0xdfff)) return 0;
        i += len;
    }
    return 1;
}

Obj *rt_fs_read(Obj *path) {
    const char *p = STR(path)->s;
    FILE *f = fopen(p, "rb");
    size_t cap = 4096, len = 0;
    char *buf;
    int e;
    Obj *r;
    if (f == NULL) return host_fail(p, host_reason(errno));
    buf = (char *)rt_realloc(NULL, cap);
    for (;;) {
        len += fread(buf + len, 1, cap - len, f);
        if (len < cap) break;
        cap *= 2;
        buf = (char *)rt_realloc(buf, cap);
    }
    e = ferror(f) ? errno : 0;
    fclose(f);
    if (e != 0) r = host_fail(p, host_reason(e));
    else if (!host_utf8_ok((const unsigned char *)buf, len)) r = host_fail(p, "not valid UTF-8 text");
    else r = res_ok(O2W(str_new(buf, len)), 1);
    free(buf);
    return r;
}

/* fs.write_file and fs.append_file: Ok(()) or Err("path: reason"). */
Obj *rt_fs_write(Obj *path, Obj *text, int append) {
    const char *p = STR(path)->s;
    FILE *f = fopen(p, append ? "ab" : "wb");
    int e;
    if (f == NULL) return host_fail(p, host_reason(errno));
    fwrite(STR(text)->s, 1, STR(text)->len, f);
    e = ferror(f) ? errno : 0;
    if (fclose(f) != 0 && e == 0) e = errno;
    return e != 0 ? host_fail(p, host_reason(e)) : res_ok(0, 0);
}

/* 0: there is nothing, 1: a file, 2: a directory, 3: something else. */
static int host_kind(const char *p) {
#ifdef _WIN32
    struct _stat st;
    if (_stat(p, &st) != 0) return 0;
    return (st.st_mode & _S_IFMT) == _S_IFDIR ? 2 : (st.st_mode & _S_IFMT) == _S_IFREG ? 1 : 3;
#else
    struct stat st;
    if (stat(p, &st) != 0) return 0;
    return S_ISDIR(st.st_mode) ? 2 : S_ISREG(st.st_mode) ? 1 : 3;
#endif
}

int rt_fs_exists(Obj *path) {
    return host_kind(STR(path)->s) != 0;
}

int rt_fs_is_file(Obj *path) {
    return host_kind(STR(path)->s) == 1;
}

int rt_fs_is_dir(Obj *path) {
    return host_kind(STR(path)->s) == 2;
}

static int host_name_cmp(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

/* fs.list_dir: Ok(names, sorted) or Err("path: reason"). */
Obj *rt_fs_list(Obj *path) {
    const char *p = STR(path)->s;
    char **names = NULL;
    size_t n = 0, cap = 0, i;
    Obj *arr;
#ifdef _WIN32
    WIN32_FIND_DATAA fd;
    size_t plen = strlen(p);
    char *pat = (char *)rt_malloc(plen + 3);
    HANDLE h;
    memcpy(pat, p, plen);
    strcpy(pat + plen, "\\*");
    h = FindFirstFileA(pat, &fd);
    free(pat);
    if (h == INVALID_HANDLE_VALUE) return host_fail(p, GetLastError() == ERROR_DIRECTORY ? "not a directory" : host_kind(p) == 1 ? "not a directory" : "not found");
    do {
        const char *name = fd.cFileName;
#else
    DIR *d = opendir(p);
    struct dirent *ent;
    if (d == NULL) return host_fail(p, host_reason(errno));
    while ((ent = readdir(d)) != NULL) {
        const char *name = ent->d_name;
#endif
        if (strcmp(name, ".") != 0 && strcmp(name, "..") != 0) {
            size_t len = strlen(name);
            char *copy = (char *)rt_malloc(len + 1);
            memcpy(copy, name, len + 1);
            if (n == cap) {
                cap = cap ? cap * 2 : 16;
                names = (char **)rt_realloc(names, cap * sizeof(char *));
            }
            names[n++] = copy;
        }
#ifdef _WIN32
    } while (FindNextFileA(h, &fd));
    FindClose(h);
#else
    }
    closedir(d);
#endif
    if (n > 1) qsort(names, n, sizeof(char *), host_name_cmp);
    arr = arr_alloc(n, 1);
    for (i = 0; i < n; i++) {
        arr_push(arr, O2W(str_new(names[i], strlen(names[i]))));
        free(names[i]);
    }
    free(names);
    return res_ok(O2W(arr), 1);
}

Obj *rt_fs_create_dir(Obj *path) {
    const char *p = STR(path)->s;
#ifdef _WIN32
    int r = _mkdir(p);
#else
    int r = mkdir(p, 0777);
#endif
    return r == 0 ? res_ok(0, 0) : host_fail(p, host_reason(errno));
}

/* A file or an empty directory. */
Obj *rt_fs_remove(Obj *path) {
    const char *p = STR(path)->s;
    int dir = host_kind(p) == 2;
    int r;
#ifdef _WIN32
    r = dir ? (RemoveDirectoryA(p) ? 0 : -1) : remove(p);
    if (r != 0 && dir) return host_fail(p, "directory not empty");
#else
    r = remove(p);
    if (r != 0 && errno == EEXIST && dir) return host_fail(p, "directory not empty");
#endif
    return r == 0 ? res_ok(0, 0) : host_fail(p, host_reason(errno));
}

Obj *rt_fs_rename(Obj *from, Obj *to) {
    const char *p = STR(from)->s;
#ifdef _WIN32
    if (host_kind(STR(to)->s) == 1) remove(STR(to)->s);
#endif
    return rename(p, STR(to)->s) == 0 ? res_ok(0, 0) : host_fail(p, host_reason(errno));
}

static int term_raw_on;        /* the keyboard is read directly */
#ifndef _WIN32
static struct termios term_saved;
#endif

static void term_restore(void) {
#ifndef _WIN32
    if (term_raw_on) tcsetattr(0, TCSANOW, &term_saved);
#endif
    term_raw_on = 0;
}

int rt_term_is_tty(void) {
#ifdef _WIN32
    return _isatty(0) && _isatty(1);
#else
    return isatty(0) && isatty(1);
#endif
}

/* term.raw(on): keys arrive one at a time, unechoed. True if the terminal is as asked (a program whose
   input is not a terminal is always fine: its keys come from the input). */
int rt_term_raw(int on) {
#ifdef _WIN32
    term_raw_on = on && _isatty(0);
    return 1;
#else
    static int registered;
    struct termios raw;
    if (!isatty(0)) return 1;
    fflush(stdout);
    if (!on) {
        term_restore();
        return 1;
    }
    if (term_raw_on) return 1;
    if (tcgetattr(0, &term_saved) != 0) return 0;
    raw = term_saved;
    raw.c_lflag &= (tcflag_t) ~(ICANON | ECHO | ISIG | IEXTEN);
    raw.c_iflag &= (tcflag_t) ~(IXON | ICRNL | INLCR | IGNCR);
    raw.c_cc[VMIN] = 1;
    raw.c_cc[VTIME] = 0;
    if (tcsetattr(0, TCSANOW, &raw) != 0) return 0;
    term_raw_on = 1;
    if (!registered) {
        registered = 1;
        atexit(term_restore);
    }
    return 1;
#endif
}

i64 rt_term_width(void) {
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO info;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &info)) return info.srWindow.Right - info.srWindow.Left + 1;
#else
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_col > 0) return ws.ws_col;
#endif
    return 80;
}

i64 rt_term_height(void) {
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO info;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &info)) return info.srWindow.Bottom - info.srWindow.Top + 1;
#else
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_row > 0) return ws.ws_row;
#endif
    return 24;
}

/* The name of the last key (see src/term.rs for the names). */
static char tk_name[40];

static int tk_set(const char *s) {
    size_t n = strlen(s);
    if (n >= sizeof tk_name) n = sizeof tk_name - 1;
    memcpy(tk_name, s, n);
    tk_name[n] = 0;
    return 1;
}

#ifdef _WIN32
/* Windows: _getwch gives characters, and a 0 / 0xE0 prefix for the others. 1: a key, 0: timeout. */
static int tk_decode(i64 wait_ms) {
    i64 waited = 0;
    int c, code;
    while (!_kbhit()) {
        if (wait_ms >= 0 && waited >= wait_ms) return 0;
        Sleep(5);
        waited += 5;
    }
    c = _getwch();
    if (c == 0 || c == 0xE0) {
        code = _getwch();
        switch (code) {
        case 72: return tk_set("up");
        case 80: return tk_set("down");
        case 75: return tk_set("left");
        case 77: return tk_set("right");
        case 71: return tk_set("home");
        case 79: return tk_set("end");
        case 73: return tk_set("pageup");
        case 81: return tk_set("pagedown");
        case 82: return tk_set("insert");
        case 83: return tk_set("delete");
        default:
            if (code >= 59 && code <= 68) {
                char f[8];
                sprintf(f, "f%d", code - 58);
                return tk_set(f);
            }
            return tk_decode(wait_ms);
        }
    }
    if (c == 13) return tk_set("enter");
    if (c == 27) return tk_set("esc");
    if (c == 9) return tk_set("tab");
    if (c == 8) return tk_set("backspace");
    if (c >= 1 && c <= 26) {
        char k[8];
        sprintf(k, "ctrl+%c", 'a' + c - 1);
        return tk_set(k);
    }
    if (c >= 32) {
        char u[8];
        int n = 0;
        if (c < 0x80) u[n++] = (char)c;
        else if (c < 0x800) { u[n++] = (char)(0xc0 | (c >> 6)); u[n++] = (char)(0x80 | (c & 0x3f)); }
        else { u[n++] = (char)(0xe0 | (c >> 12)); u[n++] = (char)(0x80 | ((c >> 6) & 0x3f)); u[n++] = (char)(0x80 | (c & 0x3f)); }
        u[n] = 0;
        return tk_set(u);
    }
    return tk_decode(wait_ms);
}
#else
/* The next byte of input within `ms` milliseconds (negative: as long as it takes): the byte, -1 for a
   timeout, -2 at the end of the input. */
static int tk_next(i64 ms) {
    if (term_raw_on) {
        fd_set fds;
        struct timeval tv, *tp = NULL;
        unsigned char c;
        int r;
        FD_ZERO(&fds);
        FD_SET(0, &fds);
        if (ms >= 0) {
            tv.tv_sec = (time_t)(ms / 1000);
            tv.tv_usec = (suseconds_t)(ms % 1000) * 1000;
            tp = &tv;
        }
        r = select(1, &fds, NULL, NULL, tp);
        if (r == 0) return -1;
        if (r < 0) return errno == EINTR ? -1 : -2;
        return read(0, &c, 1) == 1 ? c : -2;
    }
    {
        int ch = getc(stdin);
        return ch == EOF ? -2 : ch;
    }
}

static void tk_mods(int m, char *out) {
    out[0] = 0;
    m = m > 0 ? m - 1 : 0;
    if (m & 4) strcat(out, "ctrl+");
    if (m & 2) strcat(out, "alt+");
    if (m & 1) strcat(out, "shift+");
}

/* ESC [ params final */
static int tk_csi(void) {
    char params[24];
    size_t np = 0;
    int b, mods = 0, first = 0;
    char prefix[24], name[16];
    for (;;) {
        b = tk_next(30);
        if (b < 0) return 0;
        if (b >= 0x20 && b <= 0x3f) {
            if (np + 1 < sizeof params) params[np++] = (char)b;
        } else break;
    }
    params[np] = 0;
    if (np > 0 && strchr("<?>=", params[0])) return 0; /* a report from the terminal, not a key */
    first = atoi(params);
    if (strchr(params, ';')) mods = atoi(strchr(params, ';') + 1);
    prefix[0] = 0;
    if (mods > 1) tk_mods(mods, prefix);
    name[0] = 0;
    switch (b) {
    case 'A': strcpy(name, "up"); break;
    case 'B': strcpy(name, "down"); break;
    case 'C': strcpy(name, "right"); break;
    case 'D': strcpy(name, "left"); break;
    case 'H': strcpy(name, "home"); break;
    case 'F': strcpy(name, "end"); break;
    case 'Z': return tk_set("shift+tab");
    case 'P': case 'Q': case 'R': case 'S': sprintf(name, "f%d", b - 'P' + 1); break;
    case '~':
        switch (first) {
        case 1: case 7: strcpy(name, "home"); break;
        case 2: strcpy(name, "insert"); break;
        case 3: strcpy(name, "delete"); break;
        case 4: case 8: strcpy(name, "end"); break;
        case 5: strcpy(name, "pageup"); break;
        case 6: strcpy(name, "pagedown"); break;
        default:
            if (first >= 11 && first <= 15) sprintf(name, "f%d", first - 10);
            else if (first >= 17 && first <= 21) sprintf(name, "f%d", first - 11);
            else if (first == 23 || first == 24) sprintf(name, "f%d", first - 12);
            break;
        }
        break;
    default: break;
    }
    if (name[0] == 0) return 0;
    strcat(prefix, name);
    return tk_set(prefix);
}

static int tk_escape(void) {
    int b = tk_next(30);
    if (b < 0) return tk_set("esc");
    if (b == '[') return tk_csi();
    if (b == 'O') {
        b = tk_next(30);
        if (b >= 'P' && b <= 'S') {
            char f[8];
            sprintf(f, "f%d", b - 'P' + 1);
            return tk_set(f);
        }
        switch (b) {
        case 'A': return tk_set("up");
        case 'B': return tk_set("down");
        case 'C': return tk_set("right");
        case 'D': return tk_set("left");
        case 'H': return tk_set("home");
        case 'F': return tk_set("end");
        default: return 0;
        }
    }
    if (b == 0x1b) return tk_set("esc");
    if (b >= 0x20 && b <= 0x7e) {
        char k[8];
        sprintf(k, "alt+%c", b);
        return tk_set(k);
    }
    return 0;
}

/* 1: a key (in tk_name), 0: a timeout, -1: the end of the input. */
static int tk_decode(i64 wait_ms) {
    for (;;) {
        int b = tk_next(wait_ms), ok = 0;
        char k[16];
        if (b == -1) return 0;
        if (b == -2) return -1;
        if (b == 0x1b) ok = tk_escape();
        else if (b == '\r' || b == '\n') ok = tk_set("enter");
        else if (b == '\t') ok = tk_set("tab");
        else if (b == 0x7f || b == 0x08) ok = tk_set("backspace");
        else if (b == 0) ok = tk_set("ctrl+space");
        else if (b >= 1 && b <= 26) { sprintf(k, "ctrl+%c", 'a' + b - 1); ok = tk_set(k); }
        else if (b >= 0x1c && b <= 0x1f) { sprintf(k, "ctrl+%c", "\\]^_"[b - 0x1c]); ok = tk_set(k); }
        else if (b >= 0x20 && b <= 0x7e) { k[0] = (char)b; k[1] = 0; ok = tk_set(k); }
        else if (b >= 0x80) {
            int len = b >= 0xf0 ? 4 : b >= 0xe0 ? 3 : b >= 0xc0 ? 2 : 1, i;
            k[0] = (char)b;
            for (i = 1; i < len; i++) {
                int c = tk_next(30);
                if (c < 0) break;
                k[i] = (char)c;
            }
            k[i] = 0;
            ok = tk_set(k);
        }
        if (ok) return 1;
    }
}
#endif

/* term.read_key() (poll = 0, waits for ever) and term.poll_key(ms) (poll = 1: None on a timeout, and
   "eof" once the input has ended). */
Obj *rt_term_key(i64 wait_ms, int poll) {
    int r;
    fflush(stdout);
    r = tk_decode(wait_ms);
    if (r > 0) return opt_some(O2W(str_new(tk_name, strlen(tk_name))), 1);
    if (r < 0 && poll) return opt_some(O2W(str_new("eof", 3)), 1);
    return opt_none();
}
