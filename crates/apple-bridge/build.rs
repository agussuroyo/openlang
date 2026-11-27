use std::{env, process::Command};

const DEPLOYMENT: &str = "26.0";

fn run(command: &mut Command) -> String {
    let out = command
        .output()
        .unwrap_or_else(|e| panic!("{command:?}: {e}"));
    assert!(
        out.status.success(),
        "{command:?}:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn main() {
    println!("cargo:rerun-if-changed=swift");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    let out = env::var("OUT_DIR").expect("cargo sets OUT_DIR");
    let arch = match env::var("CARGO_CFG_TARGET_ARCH").as_deref() {
        Ok("aarch64") => "arm64",
        _ => "x86_64",
    };
    let sdk = run(Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]));
    run(Command::new("xcrun").args([
        "swiftc",
        "-O",
        "-parse-as-library",
        "-emit-library",
        "-static",
        "-swift-version",
        "5",
        "-target",
        &format!("{arch}-apple-macos{DEPLOYMENT}"),
        "-sdk",
        &sdk,
        "-module-name",
        "OpenLangApple",
        "-o",
        &format!("{out}/libopenlang_apple.a"),
        "swift/Common.swift",
        "swift/Speech.swift",
        "swift/Translation.swift",
        "swift/TranslationDownload.swift",
    ]));
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    println!("cargo:rustc-link-lib=static=openlang_apple");
    for framework in [
        "Foundation",
        "AVFoundation",
        "CoreMedia",
        "Speech",
        "Translation",
        "AppKit",
        "SwiftUI",
    ] {
        println!("cargo:rustc-link-lib=framework={framework}");
    }
}
