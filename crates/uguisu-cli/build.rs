//! Gives the `uguisu` binary the main-thread stack on Windows that it has on Linux.
//!
//! `#[tokio::main]` polls the whole command future on the main thread. A debug
//! build needs about 970 KiB of stack for it on Linux, where 8 MiB is reserved;
//! MSVC reserves 1 MiB, and every command overflowed there, `--help` included.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        println!("cargo:rustc-link-arg-bin=uguisu=/STACK:8388608");
    }
}
