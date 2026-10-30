//! Runtime pieces, part 2: structs and enums, arrays, maps and sets, closures, comparators.

use super::rt::{Kind::*, Piece};

pub static PIECES: &[Piece] = &[
    // ---- structs, enums, Option / Result ------------------------------------------------------
    Piece {
        name: "agg_new",
        deps: &["obj_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* A struct or one variant of an enum: `n` word-sized fields, `mask` marks the references. */
static Obj *agg_new(uint32_t tag, uint32_t n, uint64_t mask) {
    uint32_t i;
    Agg *g = (Agg *)obj_new(sizeof(Agg) + n * sizeof(W), K_AGG, 0);
    g->h.n = n;
    g->mask = mask;
    g->tag = tag;
    for (i = 0; i < n; i++) g->f[i] = 0;
    return &g->h;
}"##,
    },
    Piece {
        name: "agg1",
        deps: &["agg_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *agg1(uint32_t tag, W payload, int is_ref) {
    Obj *o = agg_new(tag, 1, is_ref ? 1u : 0u);
    AGG(o)->f[0] = payload;
    return o;
}"##,
    },
    Piece {
        name: "opt_some",
        deps: &["agg1"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Option: Some = 0, None = 1. */
static Obj *opt_some(W v, int is_ref) {
    return agg1(0, v, is_ref);
}"##,
    },
    Piece {
        name: "opt_none",
        deps: &["agg_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *opt_none(void) {
    return agg_new(1, 0, 0);
}"##,
    },
    Piece {
        name: "res_ok",
        deps: &["agg1"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Result: Ok = 0, Err = 1. */
static Obj *res_ok(W v, int is_ref) {
    return agg1(0, v, is_ref);
}"##,
    },
    Piece {
        name: "res_err",
        deps: &["agg1", "str_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *res_err(const char *msg) {
    return agg1(1, O2W(str_new(msg, strlen(msg))), 1);
}"##,
    },
    // ---- arrays ----------------------------------------------------------------------------------
    Piece {
        name: "arr_new",
        deps: &["obj_new", "rt_malloc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *arr_new(size_t cap, int is_ref) {
    Arr *a = (Arr *)obj_new(sizeof(Arr), K_ARR, is_ref ? 1u : 0u);
    a->len = 0;
    a->cap = cap;
    a->a = cap ? (W *)rt_malloc(cap * sizeof(W)) : NULL;
    gc_bytes += cap * sizeof(W);
    return &a->h;
}"##,
    },
    Piece {
        name: "arr_push",
        deps: &["arr_new", "rt_realloc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void arr_push(Obj *o, W v) {
    Arr *a = ARR(o);
    if (a->len == a->cap) {
        size_t cap = a->cap ? a->cap * 2 : 4;
        a->a = (W *)rt_realloc(a->a, cap * sizeof(W));
        gc_bytes += (cap - a->cap) * sizeof(W);
        a->cap = cap;
    }
    a->a[a->len++] = v;
}"##,
    },
    Piece {
        name: "arr_get",
        deps: &["objects", "trap_index", "hint_expect"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static W arr_get(Obj *o, int64_t i) {
    const Arr *a = ARR(o);
    if (UNLIKELY((uint64_t)i >= a->len)) trap_index(i, a->len);
    return a->a[i];
}"##,
    },
    Piece {
        name: "arr_set",
        deps: &["objects", "trap_index", "hint_expect"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void arr_set(Obj *o, int64_t i, W v) {
    Arr *a = ARR(o);
    if (UNLIKELY((uint64_t)i >= a->len)) trap_index(i, a->len);
    a->a[i] = v;
}"##,
    },
    Piece {
        name: "arr_pop",
        deps: &["objects", "trap_empty"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static W arr_pop(Obj *o) {
    Arr *a = ARR(o);
    if (a->len == 0) trap_empty();
    return a->a[--a->len];
}"##,
    },
    Piece {
        name: "arr_insert",
        deps: &["arr_push", "trap_index"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static void arr_insert(Obj *o, int64_t i, W v) {
    Arr *a = ARR(o);
    if (i < 0 || (uint64_t)i > a->len) trap_index(i, a->len);
    arr_push(o, 0);
    memmove(a->a + i + 1, a->a + i, (a->len - 1 - (size_t)i) * sizeof(W));
    a->a[i] = v;
}"##,
    },
    Piece {
        name: "arr_remove",
        deps: &["objects", "trap_index"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static W arr_remove(Obj *o, int64_t i) {
    Arr *a = ARR(o);
    W v;
    if (i < 0 || (uint64_t)i >= a->len) trap_index(i, a->len);
    v = a->a[i];
    memmove(a->a + i, a->a + i + 1, (a->len - 1 - (size_t)i) * sizeof(W));
    a->len--;
    return v;
}"##,
    },
    Piece {
        name: "arr_reverse",
        deps: &["objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void arr_reverse(Obj *o) {
    Arr *a = ARR(o);
    size_t i;
    for (i = 0; i < a->len / 2; i++) {
        W t = a->a[i];
        a->a[i] = a->a[a->len - 1 - i];
        a->a[a->len - 1 - i] = t;
    }
}"##,
    },
    Piece {
        name: "arr_clone",
        deps: &["arr_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *arr_clone(Obj *o) {
    Obj *r = arr_new(ARR(o)->len, ARR(o)->h.flags & 1);
    if (ARR(o)->len) memcpy(ARR(r)->a, ARR(o)->a, ARR(o)->len * sizeof(W));
    ARR(r)->len = ARR(o)->len;
    return r;
}"##,
    },
    Piece {
        name: "arr_contains",
        deps: &["objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int arr_contains(Obj *o, W x, int (*eq)(W, W)) {
    const Arr *a = ARR(o);
    size_t i;
    for (i = 0; i < a->len; i++)
        if (eq(a->a[i], x)) return 1;
    return 0;
}"##,
    },
    Piece {
        name: "arr_sort",
        deps: &["objects", "rt_malloc"],
        hdrs: &["stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"/* A stable merge sort; `cmp` returns <0, 0 or >0. */
static void arr_sort(Obj *o, int (*cmp)(W, W)) {
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
}"##,
    },
    Piece {
        name: "arr_try_get",
        deps: &["opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *arr_try_get(Obj *o, int64_t i) {
    const Arr *a = ARR(o);
    if (i < 0 || (uint64_t)i >= a->len) return opt_none();
    return opt_some(a->a[i], a->h.flags & 1);
}"##,
    },
    Piece {
        name: "arr_try_pop",
        deps: &["opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *arr_try_pop(Obj *o) {
    Arr *a = ARR(o);
    if (a->len == 0) return opt_none();
    return opt_some(a->a[--a->len], a->h.flags & 1);
}"##,
    },
    Piece {
        name: "arr_try_remove",
        deps: &["opt_some", "opt_none", "arr_remove"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *arr_try_remove(Obj *o, int64_t i) {
    const Arr *a = ARR(o);
    if (i < 0 || (uint64_t)i >= a->len) return opt_none();
    return opt_some(arr_remove(o, i), a->h.flags & 1);
}"##,
    },
    // ---- comparison and equality of words --------------------------------------------------------
    Piece {
        name: "eq_bits",
        deps: &["word", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int eq_bits(W a, W b) {
    return a == b;
}"##,
    },
    Piece {
        name: "eq_f64",
        deps: &["bits_f64", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int eq_f64(W a, W b) {
    return bits_f64(a) == bits_f64(b);
}"##,
    },
    Piece {
        name: "eq_f32",
        deps: &["bits_f32", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int eq_f32(W a, W b) {
    return bits_f32(a) == bits_f32(b);
}"##,
    },
    Piece {
        name: "eq_str",
        deps: &["str_eq", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static PURE_FN int eq_str(W a, W b) {
    return str_eq(W2O(a), W2O(b));
}"##,
    },
    Piece {
        name: "cmp_s64",
        deps: &["word", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int cmp_s64(W a, W b) {
    return ((int64_t)a > (int64_t)b) - ((int64_t)a < (int64_t)b);
}"##,
    },
    Piece {
        name: "cmp_u64",
        deps: &["word", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static CONST_FN int cmp_u64(W a, W b) {
    return (a > b) - (a < b);
}"##,
    },
    Piece {
        name: "cmp_f64",
        deps: &["bits_f64", "hint_const"],
        hdrs: &["math.h"],
        kind: Funcs,
        src: r##"/* Floats: NaN is unordered and sorts last. */
static CONST_FN int cmp_f64(W a, W b) {
    double x = bits_f64(a), y = bits_f64(b);
    if (x < y) return -1;
    if (x > y) return 1;
    if (x == y) return 0;
    return (isnan(x) != 0) - (isnan(y) != 0);
}"##,
    },
    Piece {
        name: "cmp_f32",
        deps: &["bits_f32", "hint_const"],
        hdrs: &["math.h"],
        kind: Funcs,
        src: r##"static CONST_FN int cmp_f32(W a, W b) {
    float x = bits_f32(a), y = bits_f32(b);
    if (x < y) return -1;
    if (x > y) return 1;
    if (x == y) return 0;
    return (isnan(x) != 0) - (isnan(y) != 0);
}"##,
    },
    Piece {
        name: "cmp_str",
        deps: &["str_cmp", "hint_const"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static PURE_FN int cmp_str(W a, W b) {
    return str_cmp(W2O(a), W2O(b));
}"##,
    },
    // ---- maps and sets -------------------------------------------------------------------------
    Piece {
        name: "map_new",
        deps: &["obj_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Maps and sets keep their entries in insertion order; `ix` is an open-addressing hash index. */
static Obj *map_new(unsigned flags) {
    Map *m = (Map *)obj_new(sizeof(Map), K_MAP, flags);
    m->len = m->cap = m->ixcap = 0;
    m->k = m->v = NULL;
    m->ix = NULL;
    return &m->h;
}"##,
    },
    Piece {
        name: "map_hash",
        deps: &["objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static uint64_t map_hash(const Map *m, W k) {
    if (m->h.flags & MF_KSTR) {
        const Str *s = (const Str *)W2O(k);
        uint64_t h = 1469598103934665603ULL;
        size_t i;
        for (i = 0; i < s->len; i++) h = (h ^ (unsigned char)s->s[i]) * 1099511628211ULL;
        return h;
    }
    k ^= k >> 33; k *= 0xff51afd7ed558ccdULL;
    k ^= k >> 33; k *= 0xc4ceb9fe1a85ec53ULL;
    k ^= k >> 33;
    return k;
}"##,
    },
    Piece {
        name: "map_find",
        deps: &["map_hash", "str_eq"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* The entry index of `key`, or -1. */
static long map_find(Obj *o, W key) {
    const Map *m = MAP(o);
    size_t mask, h;
    int32_t e;
    if (m->len == 0) return -1;
    mask = m->ixcap - 1;
    h = (size_t)map_hash(m, key) & mask;
    while ((e = m->ix[h]) >= 0) {
        if ((m->h.flags & MF_KSTR) ? str_eq(W2O(m->k[e]), W2O(key)) : m->k[e] == key) return e;
        h = (h + 1) & mask;
    }
    return -1;
}"##,
    },
    Piece {
        name: "map_reindex",
        deps: &["map_hash", "rt_realloc"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void map_reindex(Map *m) {
    size_t i, c = 8, mask, h;
    while (c < m->cap * 2) c *= 2;
    if (c != m->ixcap) {
        m->ix = (int32_t *)rt_realloc(m->ix, c * sizeof(int32_t));
        gc_bytes += (c > m->ixcap ? c - m->ixcap : 0) * sizeof(int32_t);
        m->ixcap = c;
    }
    for (i = 0; i < c; i++) m->ix[i] = -1;
    mask = c - 1;
    for (i = 0; i < m->len; i++) {
        h = (size_t)map_hash(m, m->k[i]) & mask;
        while (m->ix[h] >= 0) h = (h + 1) & mask;
        m->ix[h] = (int32_t)i;
    }
}"##,
    },
    Piece {
        name: "map_put",
        deps: &["map_find", "map_reindex"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Inserts or overwrites; returns 1 when the key is new. */
static int map_put(Obj *o, W key, W val) {
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
    m->ix[h] = (int32_t)m->len;
    m->len++;
    return 1;
}"##,
    },
    Piece {
        name: "map_get",
        deps: &["map_find", "trap_key"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static W map_get(Obj *o, W key) {
    long at = map_find(o, key);
    if (at < 0) trap_key();
    return MAP(o)->v[at];
}"##,
    },
    Piece {
        name: "map_get_or",
        deps: &["map_find"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static W map_get_or(Obj *o, W key, W dflt) {
    long at = map_find(o, key);
    return at < 0 ? dflt : MAP(o)->v[at];
}"##,
    },
    Piece {
        name: "map_try_get",
        deps: &["map_find", "opt_some", "opt_none"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *map_try_get(Obj *o, W key) {
    long at = map_find(o, key);
    if (at < 0) return opt_none();
    return opt_some(MAP(o)->v[at], (MAP(o)->h.flags & MF_VREF) != 0);
}"##,
    },
    Piece {
        name: "map_del",
        deps: &["map_find", "map_reindex"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"/* Removes a key, keeping the order of the others; returns 1 when it was present. */
static int map_del(Obj *o, W key) {
    Map *m = MAP(o);
    long at = map_find(o, key);
    if (at < 0) return 0;
    memmove(m->k + at, m->k + at + 1, (m->len - 1 - (size_t)at) * sizeof(W));
    if (!(m->h.flags & MF_SET)) memmove(m->v + at, m->v + at + 1, (m->len - 1 - (size_t)at) * sizeof(W));
    m->len--;
    map_reindex(m);
    return 1;
}"##,
    },
    Piece {
        name: "map_clear",
        deps: &["objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static void map_clear(Obj *o) {
    Map *m = MAP(o);
    size_t i;
    m->len = 0;
    for (i = 0; i < m->ixcap; i++) m->ix[i] = -1;
}"##,
    },
    Piece {
        name: "map_keys",
        deps: &["arr_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *map_keys(Obj *o) {
    const Map *m = MAP(o);
    Obj *r = arr_new(m->len, (m->h.flags & MF_KREF) != 0);
    if (m->len) memcpy(ARR(r)->a, m->k, m->len * sizeof(W));
    ARR(r)->len = m->len;
    return r;
}"##,
    },
    Piece {
        name: "map_values",
        deps: &["arr_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *map_values(Obj *o) {
    const Map *m = MAP(o);
    Obj *r = arr_new(m->len, (m->h.flags & MF_VREF) != 0);
    if (m->len) memcpy(ARR(r)->a, m->v, m->len * sizeof(W));
    ARR(r)->len = m->len;
    return r;
}"##,
    },
    // ---- closures ---------------------------------------------------------------------------------
    Piece {
        name: "clo_new",
        deps: &["obj_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* A closure: its code and the cells of the variables it captured. */
static Obj *clo_new(void (*fn)(void), uint32_t n) {
    uint32_t i;
    Clo *c = (Clo *)obj_new(sizeof(Clo) + n * sizeof(Obj *), K_CLO, 0);
    c->h.n = n;
    c->fn = fn;
    for (i = 0; i < n; i++) c->c[i] = NULL;
    return &c->h;
}"##,
    },
    Piece {
        name: "cell_new",
        deps: &["obj_new"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* A captured variable lives in a heap cell shared by the function and its closures. */
static Obj *cell_new(int is_ref, W v) {
    Cell *c = (Cell *)obj_new(sizeof(Cell), K_CELL, is_ref ? 1u : 0u);
    c->v = v;
    return &c->h;
}"##,
    },
];
