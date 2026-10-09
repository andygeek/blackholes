fn main() {
    bundle_release_notes();
    println!("cargo:rerun-if-changed=native/updater.m");
    println!("cargo:rerun-if-changed=native/notifications.m");
    println!("cargo:rerun-if-changed=native/quick_open_keyboard.m");
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
    cc::Build::new()
        .file("native/notifications.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=13.0")
        .compile("blackholes_notifications");
    cc::Build::new()
        .file("native/quick_open_keyboard.m")
        .flag("-fobjc-arc")
        .flag("-fblocks")
        .flag("-mmacosx-version-min=13.0")
        .compile("blackholes_quick_open_keyboard");
    println!("cargo:rustc-link-lib=framework=Foundation");
    println!("cargo:rustc-link-lib=framework=AppKit");
    println!("cargo:rustc-link-lib=framework=WebKit");
    println!("cargo:rustc-link-lib=framework=UserNotifications");
}

fn bundle_release_notes() {
    use std::{fmt::Write, fs, path::PathBuf};

    println!("cargo:rerun-if-changed=docs/releases");
    println!("cargo:rerun-if-changed=Cargo.toml");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let parse_version = |value: &str| -> Option<(u64, u64, u64)> {
        let mut parts = value.split('.');
        let parsed = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
        parts.next().is_none().then_some(parsed)
    };
    let current = parse_version(&version).expect("Expected a stable release version");
    let mut releases = fs::read_dir("docs/releases").expect("Could not read release notes")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter_map(|path| {
            let version = path.file_stem()?.to_str()?.to_owned();
            let parsed = parse_version(&version)?;
            (parsed <= current).then_some((parsed, version, path))
        })
        .collect::<Vec<_>>();
    releases.sort_by(|a, b| b.0.cmp(&a.0));
    assert_eq!(releases.first().map(|release| release.1.as_str()), Some(version.as_str()),
        "Add English release notes for the current version");
    let mut source = String::from("pub const RECENT_RELEASES: &[(&str, &str, &str)] = &[\n");
    for (_, version, path) in releases.into_iter().take(3) {
        let english = fs::read_to_string(&path).expect("Could not read English release notes");
        let spanish = fs::read_to_string(path.with_file_name(format!("{version}.es.md")))
            .unwrap_or_else(|_| panic!("Add Spanish release notes for {version}"));
        writeln!(source, "({version:?}, {english:?}, {spanish:?}),").unwrap();
    }
    source.push_str("];\n");
    fs::write(PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("release_notes.rs"), source)
        .expect("Could not bundle release notes");
}
