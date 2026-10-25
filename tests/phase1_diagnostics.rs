//! Phase 1 end-to-end: sources come from a `Vfs`, positions from the
//! `SourceMap`, and errors are collected in a `DiagBag` and rendered.

use neon::diag::{DiagBag, Diagnostic, Emitter, LineCol, SourceMap, Span};
use neon::vfs::{MemVfs, Vfs};

#[test]
fn load_from_memvfs_and_report_multiple_errors() {
    let vfs = MemVfs::new().with_file("src/main.neon", "let a = ;\nlet b = 2;\nlet c = );\n");
    let mut map = SourceMap::new();
    let id = map.load(&vfs, "src/main.neon").unwrap();

    // Two problems are found in one pass: nothing aborts at the first.
    let mut bag = DiagBag::new();
    bag.error(Span::new(id, 8, 9), "expected expression, found `;`");
    bag.error(Span::new(id, 29, 30), "unexpected `)`");
    assert_eq!(bag.error_count(), 2);

    let first = bag.as_slice()[0].primary_span().unwrap();
    assert_eq!(map.resolve(first).unwrap().start, LineCol { line: 0, col: 8 });
    let second = bag.as_slice()[1].primary_span().unwrap();
    assert_eq!(map.resolve(second).unwrap().start, LineCol { line: 2, col: 8 });

    let out = Emitter::new(&map).render_all(&bag);
    assert_eq!(
        out,
        "error: expected expression, found `;`\n\
         \x20--> src/main.neon:1:9\n\
         \x20 |\n\
         1 | let a = ;\n\
         \x20 |         ^ expected expression, found `;`\n\
         \n\
         error: unexpected `)`\n\
         \x20--> src/main.neon:3:9\n\
         \x20 |\n\
         3 | let c = );\n\
         \x20 |         ^ unexpected `)`\n\
         \n\
         error: aborting due to 2 previous errors\n"
    );
}

#[test]
fn imports_resolve_through_the_vfs() {
    let vfs = MemVfs::new()
        .with_file("app/main.neon", "import { f } from \"../lib/util\";")
        .with_file("lib/util.neon", "export fn f() {}");
    let spec = "../lib/util";
    let resolved = vfs.resolve("app/main.neon", spec);
    assert_eq!(resolved, "lib/util");
    let with_ext = neon::vfs::path::with_default_extension(&resolved, "neon");
    assert!(vfs.is_file(&with_ext));

    let mut map = SourceMap::new();
    let id = map.load(&vfs, &with_ext).unwrap();
    assert_eq!(map.file(id).name(), "lib/util.neon");
}

#[test]
fn missing_module_is_reportable_without_a_span() {
    let vfs = MemVfs::new();
    let mut map = SourceMap::new();
    let err = map.load(&vfs, "gone.neon").unwrap_err();

    let mut bag = DiagBag::new();
    bag.report(Diagnostic::error("cannot import `gone`").with_note(err.to_string()));
    assert_eq!(
        Emitter::new(&map).render_all(&bag),
        "error: cannot import `gone`\n\
         \x20 = note: no such file: gone.neon\n\
         \n\
         error: aborting due to 1 previous error\n"
    );
}
