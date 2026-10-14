// SPDX-License-Identifier: GPL-2.0-or-later
use sha2::{Digest, Sha256};
use std::{env, fs, path::PathBuf};

fn main() {
    assert_eq!(env::var("CARGO_CFG_TARGET_ENDIAN").unwrap(), "little");
    println!("cargo:rerun-if-changed=vendor/qemu/manifest.json");
    let manifest: serde_json::Value =
        serde_json::from_slice(&fs::read("vendor/qemu/manifest.json").unwrap()).unwrap();
    for (path, digest) in manifest["files"].as_object().unwrap() {
        let path = PathBuf::from("vendor/qemu").join(path);
        println!("cargo:rerun-if-changed={}", path.display());
        assert_eq!(
            format!("{:x}", Sha256::digest(fs::read(path).unwrap())),
            digest.as_str().unwrap()
        );
    }
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    for name in ["vars", "policy", "auth", "pkcs7"] {
        let file = format!("var-service-{name}.c");
        fs::copy(
            PathBuf::from("vendor/qemu/hw/uefi").join(&file),
            out.join(file),
        )
        .unwrap();
    }
    for patch in [
        "0001-host-state-codec.patch",
        "0002-authenticated-variable-validation.patch",
    ] {
        let path = fs::canonicalize(PathBuf::from("patches").join(patch)).unwrap();
        println!("cargo:rerun-if-changed={}", path.display());
        let result = std::process::Command::new("patch")
            .args(["--batch", "--forward", "--fuzz=0", "-p1", "-i"])
            .arg(path)
            .arg("-d")
            .arg(&out)
            .output()
            .expect("patch is required to build the variable helper");
        assert!(
            result.status.success(),
            "{patch}: {}{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let mut build = cc::Build::new();
    build
        .std("gnu11")
        .extra_warnings(false)
        .include("engine/include")
        .include("vendor/qemu/include")
        .include(&out);
    for library in ["glib-2.0", "gnutls"] {
        for include in pkg_config::probe_library(library).unwrap().include_paths {
            build.include(include);
        }
    }
    build.file("engine/engine.c");
    build.file(out.join("var-service-pkcs7.c"));
    build.file(out.join("var-service-auth.c"));
    for name in ["siglist", "utils", "guid"] {
        build.file(format!("vendor/qemu/hw/uefi/var-service-{name}.c"));
    }
    build.compile("uefi_vars_engine");
    println!("cargo:rerun-if-changed=engine");
}
