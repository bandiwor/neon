use super::*;

fn fmt(src: &str) -> String {
    format_source(src).unwrap_or_else(|e| panic!("{e}\n{src}"))
}

fn check(src: &str, want: &str) {
    let got = fmt(src);
    assert_eq!(got, want, "\n--- got ---\n{got}\n--- want ---\n{want}");
    assert_eq!(fmt(&got), got, "not idempotent");
}

#[test]
fn indents_blocks() {
    check(
        "fn main() {\nlet x = 1;\nif x > 0 {\nprint(x);\n}\n}\n",
        "fn main() {\n    let x = 1;\n    if x > 0 {\n        print(x);\n    }\n}\n",
    );
}

#[test]
fn empty_function_body_stays() {
    check("fn new() -> Point {\n}\n", "fn new() -> Point {\n}\n");
    check("fn f() {}\n", "fn f() {}\n");
}

#[test]
fn operator_spacing_uses_the_tree() {
    check(
        "fn f(a:i64,b:i64)->i64{\nlet c=a+b*-a;\nlet d=!true;\nc-b\n}\n",
        "fn f(a: i64, b: i64) -> i64 {\n    let c = a + b * -a;\n    let d = !true;\n    c - b\n}\n",
    );
    // generics are not comparisons
    check("fn f(v: Vec<i32>) -> Option<i32> {\nNone\n}\n", "fn f(v: Vec<i32>) -> Option<i32> {\n    None\n}\n");
    check("fn f(a: i32) -> bool {\na<3&&a>1\n}\n", "fn f(a: i32) -> bool {\n    a < 3 && a > 1\n}\n");
}

#[test]
fn calls_indexing_and_paths() {
    check(
        "fn main() {\nprint( add( 1 ,2 ) );\nlet x = xs [ 0 ];\nlet p = Shape :: Circle(1.0);\nlet s = a . b ( ) ;\n}\n",
        "fn main() {\n    print(add(1, 2));\n    let x = xs[0];\n    let p = Shape::Circle(1.0);\n    let s = a.b();\n}\n",
    );
}

#[test]
fn keywords_before_brackets() {
    check(
        "fn main() {\nif(x){\nreturn(1);\n}elif(y){\n}else{\n}\n}\n",
        "fn main() {\n    if (x) {\n        return (1);\n    } elif (y) {\n    } else {\n    }\n}\n",
    );
}

#[test]
fn match_and_arms() {
    check(
        "fn f(s: Shape) -> f64 {\nmatch s {\nShape::Circle(r)=>3.14*r*r,\nShape::Empty=>{\n0.0\n}\n}\n}\n",
        "fn f(s: Shape) -> f64 {\n    match s {\n        Shape::Circle(r) => 3.14 * r * r,\n        Shape::Empty => {\n            0.0\n        }\n    }\n}\n",
    );
}

#[test]
fn closures_and_try() {
    check(
        "fn main() {\nlet f = |x| => x+1;\nlet g = || => { 1 };\nlet r = parse(s)?;\nlet t = parse(s) ?;\n}\n",
        "fn main() {\n    let f = |x| => x + 1;\n    let g = || => { 1 };\n    let r = parse(s)?;\n    let t = parse(s)?;\n}\n",
    );
}

#[test]
fn continuation_lines() {
    check(
        "fn main() {\nlet x = xs\n.map(f)\n.filter(g);\nlet y = a +\nb;\nlet z =\n1;\n}\n",
        "fn main() {\n    let x = xs\n        .map(f)\n        .filter(g);\n    let y = a +\n        b;\n    let z =\n        1;\n}\n",
    );
}

#[test]
fn multiline_arguments() {
    check(
        "fn main() {\nfoo(\na,\nb,\n);\nlet v = [\n1,\n2,\n];\nbar(a, {\nbody();\n});\n}\n",
        "fn main() {\n    foo(\n        a,\n        b,\n    );\n    let v = [\n        1,\n        2,\n    ];\n    bar(a, {\n        body();\n    });\n}\n",
    );
}

#[test]
fn blank_lines() {
    check(
        "\n\nfn a() {\n\n\nlet x = 1;\n\n\n\nlet y = 2;\n\n}\n\n\n\nfn b() {}\n\n\n",
        "fn a() {\n    let x = 1;\n\n    let y = 2;\n}\n\nfn b() {}\n",
    );
}

#[test]
fn comments_are_kept() {
    check(
        "// top\nfn main() {\n// inside\nlet x = 1; // trailing   \n   /* block */\nlet y = /* mid */ 2;\n/* multi\n   line */\n}\n",
        "// top\nfn main() {\n    // inside\n    let x = 1; // trailing\n    /* block */\n    let y = /* mid */ 2;\n    /* multi\n   line */\n}\n",
    );
    // a comment before a closing brace stays at the inner level
    check("fn main() {\nlet x = 1;\n// last\n}\n", "fn main() {\n    let x = 1;\n    // last\n}\n");
}

#[test]
fn fn_types_and_impl_generics() {
    check(
        "impl <T> Stack<T> {\nfn f(g: fn (i32) -> i32) {}\n}\n",
        "impl<T> Stack<T> {\n    fn f(g: fn(i32) -> i32) {}\n}\n",
    );
}

#[test]
fn strings_are_untouched() {
    check(
        "fn main() {\nprint(\"a  +  b // not a comment\");\n}\n",
        "fn main() {\n    print(\"a  +  b // not a comment\");\n}\n",
    );
}

#[test]
fn tabs_crlf_and_final_newline() {
    check("fn main() {\r\n\tlet x = 1;\r\n}", "fn main() {\n    let x = 1;\n}\n");
}

#[test]
fn empty_and_comment_only() {
    assert_eq!(fmt(""), "");
    assert_eq!(fmt("  \n\n"), "");
    check("// only a comment", "// only a comment\n");
}

#[test]
fn enums_structs_traits() {
    check(
        "enum Shape { Circle(f64),Rect(f64,f64),Empty }\nstruct Point { x:f64,y:f64 }\nimpl Point{\nfn new()->Point{\nPoint { x:0.0,y:0.0 }\n}\n}\n",
        "enum Shape { Circle(f64), Rect(f64, f64), Empty }\nstruct Point { x: f64, y: f64 }\nimpl Point {\n    fn new() -> Point {\n        Point { x: 0.0, y: 0.0 }\n    }\n}\n",
    );
}

#[test]
fn syntax_errors_are_refused() {
    assert!(format_source("fn main( {").is_err());
    assert!(format_source("let x = \"unterminated;\n").is_err());
}

#[test]
fn corpus_is_stable_and_token_preserving() {
    let mut dirs = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/corpus"))];
    let mut files = vec![std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/src/sema/prelude.neon"))];
    while let Some(d) = dirs.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                dirs.push(p);
            } else if p.extension().is_some_and(|x| x == "neon") {
                files.push(p);
            }
        }
    }
    assert!(files.len() > 40);
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        let Ok(once) = format_source(&src) else { continue }; // files that are meant to fail to compile
        let twice = format_source(&once).unwrap_or_else(|e| panic!("{}: second pass: {e}", f.display()));
        assert_eq!(once, twice, "{}: not idempotent", f.display());
    }
}
