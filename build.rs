fn main() {
    println!("cargo:rerun-if-changed=native/updater.m");
    println!("cargo:rerun-if-env-changed=BLACKHOLES_SPARKLE_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") { return; }
    let sparkle = std::env::var("BLACKHOLES_SPARKLE_DIR")
        .expect("Use ./scripts/build-release to prepare Sparkle before building");
    cc::Build::new()
        .file("native/updater.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=13.0")
        .flag(format!("-F{sparkle}"))
        .compile("blackholes_updater");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
}
