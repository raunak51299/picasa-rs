//! Embeds the Windows icon resource in the executable.
//!
//! The GNU toolchain uses `windres`; the MSVC toolchain uses the Windows SDK
//! `rc.exe`. The icon is loaded again at startup with
//! `winit::window::Icon::from_resource(1, ...)` so the title bar and taskbar
//! use it too.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=packaging/windows/picasa-rs.rc");
    println!("cargo:rerun-if-changed=packaging/windows/picasa-rs.ico");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    let rc = root.join("packaging/windows/picasa-rs.rc");

    if env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
        compile_msvc(&root, &out, &rc);
    } else {
        compile_gnu(&root, &out, &rc);
    }
}

fn compile_gnu(root: &Path, out: &Path, rc: &Path) {
    let object = out.join("picasa-rs-resource.o");
    let status = Command::new("windres")
        .arg("--input-format=rc")
        .arg("--output-format=coff")
        .arg("-I")
        .arg(root.join("packaging/windows"))
        .arg(rc)
        .arg("-o")
        .arg(&object)
        .status()
        .expect("could not run windres; install MinGW-w64 binutils");

    assert!(status.success(), "windres failed with {status}");
    println!("cargo:rustc-link-arg={}", object.display());
}

fn compile_msvc(root: &Path, out: &Path, rc: &Path) {
    let rc_exe =
        find_rc().expect("could not find rc.exe; install the Windows SDK or the MSVC build tools");
    let resource = out.join("picasa-rs-resource.res");
    let status = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/i")
        .arg(root.join("packaging/windows"))
        .arg("/fo")
        .arg(&resource)
        .arg(rc)
        .status()
        .unwrap_or_else(|error| panic!("could not run {}: {error}", rc_exe.display()));

    assert!(status.success(), "rc.exe failed with {status}");
    println!("cargo:rustc-link-arg={}", resource.display());
}

fn find_rc() -> Option<PathBuf> {
    if let Ok(output) = Command::new("where").arg("rc.exe").output()
        && output.status.success()
        && let Some(line) = String::from_utf8_lossy(&output.stdout).lines().next()
    {
        let path = PathBuf::from(line.trim());
        if path.is_file() {
            return Some(path);
        }
    }

    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        let Some(base) = env::var_os(variable) else {
            continue;
        };
        let kits = PathBuf::from(base).join("Windows Kits/10/bin");
        let Ok(entries) = std::fs::read_dir(kits) else {
            continue;
        };
        let mut versions: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect();
        versions.sort();
        for version in versions.iter().rev() {
            for architecture in ["x64", "x86", "arm64"] {
                let candidate = version.join(architecture).join("rc.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }

    None
}
