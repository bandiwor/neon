//! Neon command-line entry point.

#![warn(missing_debug_implementations, rust_2018_idioms)]

fn main() {
    println!("neon {}", env!("CARGO_PKG_VERSION"));
}
