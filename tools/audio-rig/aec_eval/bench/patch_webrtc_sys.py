#!/usr/bin/env python3
"""Copy webrtc-audio-processing-sys and patch its build.rs to use a prebuilt (cross-compiled) tree.

usage: patch_webrtc_sys.py SRC_CRATE_DIR DEST_DIR
With env WAP_PREBUILT=<dir holding src/, build/, prefix/ from a meson cross build> the patched build.rs
skips meson and symbol prefixing, and links the absl static libraries meson built. Without the variable the
build is the original bundled one. Only the (large) vendored C++ sources are left out of the copy.
"""
import os
import shutil
import sys

src, dst = sys.argv[1:3]
shutil.copytree(src, dst, ignore=shutil.ignore_patterns("webrtc-audio-processing"))
os.makedirs(os.path.join(dst, "webrtc-audio-processing"), exist_ok=True)
open(os.path.join(dst, "webrtc-audio-processing", ".keep"), "w").close()
p = os.path.join(dst, "build.rs")
t = open(p).read()


def sub(old, new):
    global t
    assert old in t, old[:60]
    t = t.replace(old, new)


sub("""    pub(super) fn build_if_necessary() -> Result<()> {
        let bundled_source_path""", """    pub(super) fn build_if_necessary() -> Result<()> {
        if std::env::var("WAP_PREBUILT").is_ok() { return Ok(()); }
        let bundled_source_path""")
sub("""    pub(super) fn get_build_paths() -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        let mut include_paths = vec![""", """    pub(super) fn get_build_paths() -> Result<(Vec<PathBuf>, Vec<PathBuf>)> {
        if let Ok(w) = std::env::var("WAP_PREBUILT") {
            let w = PathBuf::from(w);
            return Ok((vec![w.join("prefix/include"), w.join("prefix/include/webrtc-audio-processing-2"), w.join("src"), w.join("src/webrtc")],
                       vec![w.join("prefix/lib"), w.join("build/subprojects/abseil-cpp-20240722.0")]));
        }
        let mut include_paths = vec![""")
sub("""        let static_lib_filename = format!("lib{LIB_NAME}.a");
""", """        if std::env::var("WAP_PREBUILT").is_ok() { return Ok(vec![]); }
        let static_lib_filename = format!("lib{LIB_NAME}.a");
""")
sub("""        println!("cargo:rustc-link-lib=absl_strings");""", """        for l in ["absl_synchronization", "absl_strings", "absl_base", "absl_numeric", "absl_types", "absl_time", "absl_debugging",
                  "absl_log", "absl_status", "absl_container", "absl_hash", "absl_crc", "absl_profiling", "absl_random", "absl_flags"] {
            println!("cargo:rustc-link-lib=static={l}");
        }""")
open(p, "w").write(t)
