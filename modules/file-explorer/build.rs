//! Builds powerkit_preview.dll (the preview handlers' and thumbnail providers' COM
//! server) for the same target and leaves it in OUT_DIR, where src/live.rs embeds it.
//! It is a separate cargo run because a workspace build cannot turn one crate into
//! both a feature-less library for powerkit.exe and a DLL with the `server` feature.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../crates/powerkit-preview/src");
    println!("cargo:rerun-if-changed=../../crates/powerkit-preview/Cargo.toml");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let dll = out.join("powerkit_preview.dll");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        std::fs::write(&dll, []).expect("writing a placeholder into OUT_DIR");
        return;
    }
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest_dir.join("../..");
    // Its own target folder: the outer build holds the lock on the usual one.
    let target_dir = root.join("target").join("preview-server");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--offline",
            "-p",
            "powerkit-preview",
            "--features",
            "server",
            "--target",
        ])
        .arg(&target)
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .expect("starting cargo to build powerkit_preview.dll");
    assert!(status.success(), "building powerkit_preview.dll failed");
    let built = target_dir.join(&target).join("release").join("powerkit_preview.dll");
    std::fs::copy(&built, &dll).unwrap_or_else(|e| panic!("copying {}: {e}", built.display()));
}
