#![cfg_attr(feature = "axstd", no_std)]
#![cfg_attr(feature = "axstd", no_main)]

#[cfg(feature = "axstd")]
use axstd::println;

#[cfg_attr(feature = "axstd", no_mangle)]
fn main() {
    // ANSI escape sequences: `\x1b[32m` switches the terminal foreground to
    // green, and `\x1b[0m` resets all attributes so later output is not colored.
    println!("\x1b[32m[WithColor]: Hello, Arceos!\x1b[0m");
}
