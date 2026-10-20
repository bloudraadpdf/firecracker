// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: GPL-2.0-only

use std::env;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let version = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("x86") => "5.01",
        Ok("x86_64") => "6.01",
        _ => return,
    };
    if env::var_os("CARGO_FEATURE_MINIPORT").is_some() {
        for argument in ["/DRIVER", "/NODEFAULTLIB", "/ENTRY:DriverEntry", "/RELEASE"] {
            println!("cargo:rustc-link-arg-bin=qxl-miniport={argument}");
        }
        println!("cargo:rustc-link-arg-bin=qxl-miniport=/SUBSYSTEM:NATIVE,{version}");
        println!("cargo:rustc-link-arg-bin=qxl-miniport=/OSVERSION:{version}");
    }
    if env::var_os("CARGO_FEATURE_DISPLAY").is_some() {
        for argument in [
            "/DLL",
            "/NODEFAULTLIB",
            "/ENTRY:DrvEnableDriver",
            "/EXPORT:DrvEnableDriver",
            "/RELEASE",
        ] {
            println!("cargo:rustc-link-arg-bin=qxl-display={argument}");
        }
        println!("cargo:rustc-link-arg-bin=qxl-display=/SUBSYSTEM:NATIVE,{version}");
        println!("cargo:rustc-link-arg-bin=qxl-display=/OSVERSION:{version}");
    }
}
