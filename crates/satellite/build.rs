//! Exposes the compilation target triple to the binary.

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=WEBOS_SATELLITE_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
