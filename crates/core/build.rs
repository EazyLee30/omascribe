//! Build identity for reproducible local benchmarks. Never collects environment contents.
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn walk(path: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(path).expect("core sources") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            walk(&path, files);
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
}
fn main() {
    let core = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap())
        .canonicalize()
        .expect("core manifest directory");
    let root = core.join("../..").canonicalize().expect("workspace root");
    let mut files = vec![core.join("build.rs"), core.join("Cargo.toml")];
    walk(&core.join("src"), &mut files);
    for path in [
        "Cargo.toml",
        "Cargo.lock",
        "models.lock.json",
        "model-catalog.json",
        "src-tauri/tauri.conf.json",
    ] {
        files.push(root.join(path));
    }
    files.sort();
    let mut hash = Sha256::new();
    for path in files {
        println!("cargo:rerun-if-changed={}", path.display());
        hash.update(
            path.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/"),
        );
        hash.update([0]);
        hash.update(fs::read(path).expect("build input"));
        hash.update([0]);
    }
    println!(
        "cargo:rustc-env=OMASCRIBE_SOURCE_SHA256={:x}",
        hash.finalize()
    );
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|r| r.status.success())
            .map(|r| String::from_utf8_lossy(&r.stdout).trim().to_owned())
    };
    let revision = if root.join(".git").exists() {
        git(&["rev-parse", "HEAD"])
    } else {
        None
    };
    println!(
        "cargo:rustc-env=OMASCRIBE_COMPILE_REVISION={}",
        revision.as_deref().unwrap_or("unversioned")
    );
    let compiler = Command::new(std::env::var_os("RUSTC").unwrap())
        .arg("--version")
        .output()
        .expect("compiler version");
    println!(
        "cargo:rustc-env=OMASCRIBE_COMPILE_RUSTC={}",
        String::from_utf8_lossy(&compiler.stdout).trim()
    );
    println!(
        "cargo:rustc-env=OMASCRIBE_COMPILE_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    println!(
        "cargo:rustc-env=OMASCRIBE_COMPILE_PROFILE={}",
        std::env::var("PROFILE").unwrap()
    );
}
