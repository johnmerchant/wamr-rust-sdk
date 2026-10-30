/*
 * Copyright (C) 2023 Liquid Reply GmbH. All rights reserved.
 * SPDX-License-Identifier: Apache-2.0 WITH LLVM-exception
 */

extern crate bindgen;
extern crate cmake;

use cmake::Config;
use std::{env, path::Path, path::PathBuf};

const LLVM_LIBRARIES: &[&str] = &[
    // keep alphabet order
    "LLVMOrcJIT",
    "LLVMOrcShared",
    "LLVMOrcTargetProcess",
    "LLVMPasses",
    "LLVMProfileData",
    "LLVMRuntimeDyld",
    "LLVMScalarOpts",
    "LLVMSelectionDAG",
    "LLVMSymbolize",
    "LLVMTarget",
    "LLVMTextAPI",
    "LLVMTransformUtils",
    "LLVMVectorize",
    "LLVMX86AsmParser",
    "LLVMX86CodeGen",
    "LLVMX86Desc",
    "LLVMX86Disassembler",
    "LLVMX86Info",
    "LLVMXRay",
    "LLVMipo",
];

/// What of WAMR's source the runtime's CMake build reads: the copy made in
/// OUT_DIR (to apply `patches/` to) holds these.
const SOURCE: &[&str] = &["CMakeLists.txt", "build-scripts", "core"];

fn check_is_espidf() -> bool {
    let is_espidf = env::var("CARGO_FEATURE_ESP_IDF").is_ok()
        && env::var("CARGO_CFG_TARGET_OS").unwrap() == "espidf";

    if is_espidf
        && (env::var("WAMR_BUILD_PLATFORM").is_ok()
            || env::var("WAMR_SHARED_PLATFORM_CONFIG").is_ok())
    {
        panic!("ESP-IDF build cannot use custom platform build (WAMR_BUILD_PLATFORM) or shared platform config (WAMR_SHARED_PLATFORM_CONFIG)");
    }

    is_espidf
}

/// "1" if the cargo feature is on, else "0".
fn flag(on: bool) -> String {
    if on { "1" } else { "0" }.to_string()
}

/// The WAMR_BUILD_* settings the cargo features choose. With the default
/// features they are what this crate always built: AOT, the fast
/// interpreter, SIMD and WASI.
fn feature_flags() -> Vec<(&'static str, String)> {
    vec![
        ("WAMR_BUILD_AOT", flag(cfg!(feature = "aot"))),
        ("WAMR_BUILD_INTERP", "1".to_string()),
        (
            "WAMR_BUILD_FAST_INTERP",
            flag(cfg!(feature = "fast-interp")),
        ),
        ("WAMR_BUILD_JIT", flag(cfg!(feature = "llvmjit"))),
        ("WAMR_BUILD_BULK_MEMORY", "1".to_string()),
        ("WAMR_BUILD_REF_TYPES", "1".to_string()),
        ("WAMR_BUILD_SIMD", flag(cfg!(feature = "simd"))),
        ("WAMR_BUILD_LIBC_WASI", flag(cfg!(feature = "wasi"))),
        ("WAMR_BUILD_LIBC_BUILTIN", "0".to_string()),
        (
            "WAMR_DISABLE_HW_BOUND_CHECK",
            flag(!cfg!(feature = "hw-bound-check")),
        ),
        (
            "WAMR_BUILD_MULTI_MODULE",
            flag(cfg!(feature = "multi-module")),
        ),
        (
            "WAMR_BUILD_DUMP_CALL_STACK",
            flag(cfg!(feature = "dump-call-stack")),
        ),
        (
            "WAMR_BUILD_CUSTOM_NAME_SECTION",
            flag(cfg!(feature = "name-section")),
        ),
        (
            "WAMR_BUILD_LOAD_CUSTOM_SECTION",
            flag(cfg!(feature = "custom-section")),
        ),
    ]
}

/// WAMR's name for the CPU it is built for: WAMR_BUILD_TARGET if set, else
/// ARMV5TE for an armv5te target (WAMR's ARM assumes ARMv7 instructions),
/// else WAMR's own choice.
fn build_target() -> Option<String> {
    if let Ok(t) = env::var("WAMR_BUILD_TARGET") {
        return Some(t);
    }
    let target = env::var("TARGET").unwrap_or_default();
    target.starts_with("armv5te").then(|| "ARMV5TE".to_string())
}

fn link_llvm_libraries(llvm_cfg_path: &String) {
    if !cfg!(feature = "llvmjit") {
        return;
    }

    let llvm_cfg_path = PathBuf::from(llvm_cfg_path);
    assert!(llvm_cfg_path.exists());

    let llvm_lib_path = llvm_cfg_path.join("../../../lib").canonicalize().unwrap();
    assert!(llvm_lib_path.exists());

    println!("cargo:rustc-link-lib=dylib=dl");
    println!("cargo:rustc-link-lib=dylib=m");
    println!("cargo:rustc-link-lib=dylib=rt");
    println!("cargo:rustc-link-lib=dylib=stdc++");
    println!("cargo:rustc-link-lib=dylib=z");
    println!("cargo:libdir={}", llvm_lib_path.display());
    println!("cargo:rustc-link-search=native={}", llvm_lib_path.display());

    for &llvm_lib in LLVM_LIBRARIES {
        println!("cargo:rustc-link-lib=static={}", llvm_lib);
    }
}

fn setup_config(wamr_root: &PathBuf) -> Config {
    let mut cfg = Config::new(wamr_root);
    for (key, value) in feature_flags() {
        cfg.define(key, &value);
    }

    // always assume non-empty strings for these environment variables

    if let Ok(platform_name) = env::var("WAMR_BUILD_PLATFORM") {
        cfg.define("WAMR_BUILD_PLATFORM", &platform_name);
    }

    if let Some(target_name) = build_target() {
        cfg.define("WAMR_BUILD_TARGET", &target_name);
    }

    if let Ok(platform_config) = env::var("WAMR_SHARED_PLATFORM_CONFIG") {
        cfg.define("SHARED_PLATFORM_CONFIG", &platform_config);
    }

    if let Ok(llvm_cfg_path) = env::var("LLVM_LIB_CFG_PATH") {
        link_llvm_libraries(&llvm_cfg_path);
        cfg.define("LLVM_DIR", &llvm_cfg_path);
    }

    // STDIN/STDOUT/STDERR redirect
    if let Ok(bh_vprintf) = env::var("WAMR_BH_VPRINTF") {
        cfg.define("WAMR_BH_VPRINTF", &bh_vprintf);
    }

    cfg
}

fn build_wamr_libraries(wamr_root: &PathBuf) {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let vmbuild_path = out_dir.join("vmbuild");

    let mut cfg = setup_config(wamr_root);
    let dst = cfg.out_dir(vmbuild_path).build_target("vmlib").build();

    println!("cargo:rustc-link-search=native={}/build", dst.display());
    println!("cargo:rustc-link-lib=static=iwasm");
}

/// Builds wamrc, WAMR's AOT compiler, for the build machine: only with the
/// `wamrc` feature, as it needs LLVM (LLVM_LIB_CFG_PATH).
fn build_wamrc(wamr_root: &Path) {
    if !cfg!(feature = "wamrc") {
        return;
    }
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let wamrc_build_path = out_dir.join("wamrcbuild");

    let wamr_compiler_path = wamr_root.join("wamr-compiler");
    assert!(wamr_compiler_path.exists());

    Config::new(&wamr_compiler_path)
        .out_dir(wamrc_build_path)
        .define("WAMR_BUILD_WITH_CUSTOM_LLVM", "1")
        .define(
            "LLVM_DIR",
            env::var("LLVM_LIB_CFG_PATH")
                .expect("LLVM_LIB_CFG_PATH isn't specified in config.toml"),
        )
        .build();
}

fn generate_bindings(wamr_root: &Path) {
    let wamr_header = wamr_root.join("core/iwasm/include/wasm_export.h");
    assert!(wamr_header.exists());

    // Freestanding: wasm_export.h needs only stdint.h and stdbool.h, which
    // clang has for any target, so the bindings generate when
    // cross-compiling without the target's C library headers.
    let bindings = bindgen::Builder::default()
        .clang_arg("-ffreestanding")
        .ctypes_prefix("::core::ffi")
        .use_core()
        .header(wamr_header.into_os_string().into_string().unwrap())
        .derive_default(true)
        .generate()
        .expect("Unable to generate bindings");
    let out_path = PathBuf::from(env::var("OUT_DIR").unwrap());
    bindings
        .write_to_file(out_path.join("bindings.rs"))
        .expect("Couldn't write bindings");
}

/// Copies `from` into `to`, recursively.
fn copy_tree(from: &Path, to: &Path) {
    if from.is_dir() {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            copy_tree(&e.path(), &to.join(e.file_name()));
        }
    } else {
        std::fs::create_dir_all(to.parent().unwrap()).unwrap();
        std::fs::copy(from, to).unwrap_or_else(|e| panic!("{}: {e}", from.display()));
    }
}

/// WAMR's source with `patches/` applied, in order: a copy of what the
/// build reads, in OUT_DIR, so that the submodule itself is never changed.
/// With no patches, the submodule as it is. A patch that doesn't apply
/// fails the build.
fn patched_source(wamr_root: &Path) -> PathBuf {
    let mut patches: Vec<PathBuf> = std::fs::read_dir("patches")
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "patch"))
                .collect()
        })
        .unwrap_or_default();
    println!("cargo:rerun-if-changed=patches");
    if patches.is_empty() {
        return wamr_root.to_path_buf();
    }
    patches.sort();
    let copy = PathBuf::from(env::var("OUT_DIR").unwrap()).join("wasm-micro-runtime");
    let _ = std::fs::remove_dir_all(&copy);
    for part in SOURCE {
        copy_tree(&wamr_root.join(part), &copy.join(part));
    }
    for p in &patches {
        println!("cargo:rerun-if-changed={}", p.display());
        let text = std::fs::read_to_string(p).unwrap();
        for (file, diff) in file_diffs(&text) {
            let patch =
                diffy::Patch::from_str(&diff).unwrap_or_else(|e| panic!("{}: {e}", p.display()));
            let path = copy.join(&file);
            let original = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{file}: {e}"));
            let fixed = diffy::apply(&original, &patch)
                .unwrap_or_else(|e| panic!("{} doesn't apply to {file}: {e}", p.display()));
            std::fs::write(&path, fixed).unwrap();
        }
    }
    copy
}

/// A patch's diffs, a file at a time: (its path, its `---`, `+++` and
/// hunks). The DEP-3 header before the first, and git's `diff` and
/// `index` lines, are left out.
fn file_diffs(text: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if let Some(file) = line.strip_prefix("--- a/") {
            out.push((file.trim().to_string(), String::new()));
        }
        let Some((_, diff)) = out.last_mut() else {
            continue;
        };
        let hunk = line.starts_with([' ', '+', '-', '@', '\\']) || line.is_empty();
        if hunk && !line.starts_with("diff --git") {
            diff.push_str(line);
            diff.push('\n');
        }
    }
    out
}

fn main() {
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_ESP_IDF");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
    println!("cargo:rerun-if-env-changed=WAMR_BUILD_PLATFORM");
    println!("cargo:rerun-if-env-changed=WAMR_BUILD_TARGET");
    println!("cargo:rerun-if-env-changed=WAMR_SHARED_PLATFORM_CONFIG");
    println!("cargo:rerun-if-env-changed=LLVM_LIB_CFG_PATH");
    println!("cargo:rerun-if-env-changed=WAMR_BH_VPRINTF");

    let wamr_root = env::current_dir().unwrap();
    let wamr_root = wamr_root.join("wasm-micro-runtime");
    assert!(wamr_root.exists());

    if !check_is_espidf() {
        // because the ESP-IDF build procedure differs from the regular one
        // (build internally by esp-idf-sys),
        let source = patched_source(&wamr_root);
        build_wamr_libraries(&source);
        // wamrc builds from the submodule: it is a host tool, unpatched.
        build_wamrc(&wamr_root);
    }

    generate_bindings(&wamr_root);
}
