//! Neon command-line entry point.

#![warn(missing_debug_implementations, rust_2018_idioms)]

use std::io::{self, IsTerminal};

use neon::vfs::OsVfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let stdin = io::stdin();
    let interactive = stdin.is_terminal();
    let code = neon::cli::main_with(&args, &OsVfs::new(), stdin.lock(), io::stdout(), &mut io::stderr(), interactive);
    std::process::exit(code);
}
