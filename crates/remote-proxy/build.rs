//! Exposes the compilation target triple to the binary (`--version` prints
//! it; install.sh checks it as for the daemon).

fn main() {
    let target = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_owned());
    println!("cargo:rustc-env=REMOTE_PROXY_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
