/* A C module for examples/c_module/main.neon.
 *
 * It is compiled into the same translation unit as the translated program (the program `#include`s it),
 * so it uses the runtime's types from neon.h: i8..u64, f32, f64, bool, chr (a Unicode scalar value).
 * Every function is declared in the Neon program with `extern fn`; the declaration is repeated in the
 * generated C file, so the C compiler rejects a definition that does not match it.
 */
#include "neon.h"
#include <ctype.h>

static i64 calls;   /* private state of the module */

i64 add(i64 a, i64 b) {
    calls++;
    return a + b;
}

f64 hypot2(f64 x, f64 y) { return sqrt(x * x + y * y); }

bool is_even(i64 n) { return n % 2 == 0; }

i64 call_count(void) { return calls; }

/* A `string` parameter is a NUL-terminated UTF-8 `const char *`, valid for the call only: do not keep it.
 * A `string` result is `const char *`, copied by the caller (so a static buffer is fine); NULL is an error. */
const char *shout(const char *text) {
    static char buf[128];
    size_t i, n = strlen(text);
    if (n > sizeof buf - 1) n = sizeof buf - 1;
    for (i = 0; i < n; i++) buf[i] = (char)toupper((unsigned char)text[i]);
    buf[n] = 0;
    return buf;
}
