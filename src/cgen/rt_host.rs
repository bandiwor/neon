//! Runtime pieces, part 4: the standard modules `sys`, `fs` and `term`. POSIX and Windows in one source:
//! the platform headers are chosen by the preprocessor (see `HOST_INCLUDES`).

use super::rt::{Kind::*, Piece};

/// The headers the pieces below need, emitted at the top of the file when any of them is used. The
/// feature macro must come before the first include.
pub const HOST_INCLUDES: &str = "#ifndef _WIN32\n#define _POSIX_C_SOURCE 200809L\n#endif\n";

pub const HOST_SYSTEM_HEADERS: &str = "#include <errno.h>
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
";

pub static PIECES: &[Piece] = &[
    Piece { name: "host_headers", deps: &[], hdrs: &["stdio.h", "stdlib.h", "string.h"], kind: Decl, src: "" },
    // ---- sys -------------------------------------------------------------------------------------
    Piece {
        name: "rt_sys_exit",
        deps: &["host_headers", "hint_cold"],
        hdrs: &["stdlib.h", "stdio.h"],
        kind: Funcs,
        src: r##"/* sys.exit(code): the output is flushed, the terminal is restored by its atexit handler. */
static NORETURN void rt_sys_exit(i64 code) {
    fflush(stdout);
    exit((int)code);
}"##,
    },
    Piece {
        name: "rt_sys_env",
        deps: &["host_headers", "opt_some", "opt_none", "str_new", "objects"],
        hdrs: &["stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"static Obj *rt_sys_env(Obj *name) {
    const char *v = getenv(STR(name)->s);
    return v ? opt_some(O2W(str_new(v, strlen(v))), 1) : opt_none();
}"##,
    },
    Piece {
        name: "rt_sys_platform",
        deps: &["host_headers", "str_new"],
        hdrs: &["string.h"],
        kind: Funcs,
        src: r##"static Obj *rt_sys_platform(void) {
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
}"##,
    },
    Piece {
        name: "rt_sys_now_ms",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Milliseconds since 1970-01-01 UTC. */
static i64 rt_sys_now_ms(void) {
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
}"##,
    },
    Piece {
        name: "rt_sys_ticks_ms",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* Milliseconds since the first call. */
static i64 rt_sys_ticks_ms(void) {
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
}"##,
    },
    Piece {
        name: "rt_sys_sleep",
        deps: &["host_headers"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static void rt_sys_sleep(i64 ms) {
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
}"##,
    },
    // ---- fs --------------------------------------------------------------------------------------
    Piece {
        name: "host_fail",
        deps: &["host_headers", "res_err", "rt_malloc"],
        hdrs: &["stdio.h", "stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"/* What an operating system error is called in a program's output: the same words on every platform. */
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
}"##,
    },
    Piece {
        name: "rt_fs_read",
        deps: &["host_fail", "res_ok", "str_new", "rt_realloc", "objects"],
        hdrs: &["stdio.h", "stdlib.h"],
        kind: Funcs,
        src: r##"/* Does `s` hold well-formed UTF-8? */
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

static Obj *rt_fs_read(Obj *path) {
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
}"##,
    },
    Piece {
        name: "rt_fs_write",
        deps: &["host_fail", "res_ok", "objects"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* fs.write_file and fs.append_file: Ok(()) or Err("path: reason"). */
static Obj *rt_fs_write(Obj *path, Obj *text, int append) {
    const char *p = STR(path)->s;
    FILE *f = fopen(p, append ? "ab" : "wb");
    int e;
    if (f == NULL) return host_fail(p, host_reason(errno));
    fwrite(STR(text)->s, 1, STR(text)->len, f);
    e = ferror(f) ? errno : 0;
    if (fclose(f) != 0 && e == 0) e = errno;
    return e != 0 ? host_fail(p, host_reason(e)) : res_ok(0, 0);
}"##,
    },
    Piece {
        name: "host_kind",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"/* 0: there is nothing, 1: a file, 2: a directory, 3: something else. */
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
}"##,
    },
    Piece {
        name: "rt_fs_exists",
        deps: &["host_kind", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int rt_fs_exists(Obj *path) {
    return host_kind(STR(path)->s) != 0;
}"##,
    },
    Piece {
        name: "rt_fs_is_file",
        deps: &["host_kind", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int rt_fs_is_file(Obj *path) {
    return host_kind(STR(path)->s) == 1;
}"##,
    },
    Piece {
        name: "rt_fs_is_dir",
        deps: &["host_kind", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int rt_fs_is_dir(Obj *path) {
    return host_kind(STR(path)->s) == 2;
}"##,
    },
    Piece {
        name: "rt_fs_list",
        deps: &[
            "host_fail",
            "host_kind",
            "res_ok",
            "arr_alloc",
            "arr_push",
            "str_new",
            "rt_malloc",
            "rt_realloc",
            "objects",
        ],
        hdrs: &["stdlib.h", "string.h"],
        kind: Funcs,
        src: r##"static int host_name_cmp(const void *a, const void *b) {
    return strcmp(*(char *const *)a, *(char *const *)b);
}

/* fs.list_dir: Ok(names, sorted) or Err("path: reason"). */
static Obj *rt_fs_list(Obj *path) {
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
}"##,
    },
    Piece {
        name: "rt_fs_create_dir",
        deps: &["host_fail", "res_ok", "objects"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static Obj *rt_fs_create_dir(Obj *path) {
    const char *p = STR(path)->s;
#ifdef _WIN32
    int r = _mkdir(p);
#else
    int r = mkdir(p, 0777);
#endif
    return r == 0 ? res_ok(0, 0) : host_fail(p, host_reason(errno));
}"##,
    },
    Piece {
        name: "rt_fs_remove",
        deps: &["host_fail", "host_kind", "res_ok", "objects"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"/* A file or an empty directory. */
static Obj *rt_fs_remove(Obj *path) {
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
}"##,
    },
    Piece {
        name: "rt_fs_rename",
        deps: &["host_fail", "host_kind", "res_ok", "objects"],
        hdrs: &["stdio.h"],
        kind: Funcs,
        src: r##"static Obj *rt_fs_rename(Obj *from, Obj *to) {
    const char *p = STR(from)->s;
#ifdef _WIN32
    if (host_kind(STR(to)->s) == 1) remove(STR(to)->s);
#endif
    return rename(p, STR(to)->s) == 0 ? res_ok(0, 0) : host_fail(p, host_reason(errno));
}"##,
    },
    // ---- term ------------------------------------------------------------------------------------
    Piece {
        name: "term_state",
        deps: &["host_headers"],
        hdrs: &["stdlib.h", "stdio.h"],
        kind: Funcs,
        src: r##"static int term_raw_on;        /* the keyboard is read directly */
#ifndef _WIN32
static struct termios term_saved;
#endif

static void term_restore(void) {
#ifndef _WIN32
    if (term_raw_on) tcsetattr(0, TCSANOW, &term_saved);
#endif
    term_raw_on = 0;
}"##,
    },
    Piece {
        name: "rt_term_is_tty",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static int rt_term_is_tty(void) {
#ifdef _WIN32
    return _isatty(0) && _isatty(1);
#else
    return isatty(0) && isatty(1);
#endif
}"##,
    },
    Piece {
        name: "rt_term_raw",
        deps: &["term_state"],
        hdrs: &["stdlib.h", "stdio.h"],
        kind: Funcs,
        src: r##"/* term.raw(on): keys arrive one at a time, unechoed. True if the terminal is as asked (a program whose
   input is not a terminal is always fine: its keys come from the input). */
static int rt_term_raw(int on) {
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
}"##,
    },
    Piece {
        name: "rt_term_width",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static i64 rt_term_width(void) {
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO info;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &info)) return info.srWindow.Right - info.srWindow.Left + 1;
#else
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_col > 0) return ws.ws_col;
#endif
    return 80;
}"##,
    },
    Piece {
        name: "rt_term_height",
        deps: &["host_headers"],
        hdrs: &[],
        kind: Funcs,
        src: r##"static i64 rt_term_height(void) {
#ifdef _WIN32
    CONSOLE_SCREEN_BUFFER_INFO info;
    if (GetConsoleScreenBufferInfo(GetStdHandle(STD_OUTPUT_HANDLE), &info)) return info.srWindow.Bottom - info.srWindow.Top + 1;
#else
    struct winsize ws;
    if (ioctl(1, TIOCGWINSZ, &ws) == 0 && ws.ws_row > 0) return ws.ws_row;
#endif
    return 24;
}"##,
    },
    Piece {
        name: "rt_term_key",
        deps: &["term_state", "opt_some", "opt_none", "str_new"],
        hdrs: &["stdio.h", "string.h"],
        kind: Funcs,
        src: r##"/* The name of the last key (see src/term.rs for the names). */
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
static Obj *rt_term_key(i64 wait_ms, int poll) {
    int r;
    fflush(stdout);
    r = tk_decode(wait_ms);
    if (r > 0) return opt_some(O2W(str_new(tk_name, strlen(tk_name))), 1);
    if (r < 0 && poll) return opt_some(O2W(str_new("eof", 3)), 1);
    return opt_none();
}"##,
    },
];
