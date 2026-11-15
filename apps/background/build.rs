//! Embeds the application icon, version information and manifest into powerkit.exe.

const MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="PowerKit.Host" version="1.0.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <!-- Never auto-elevate: PowerKit runs with the user's own rights. -->
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
  <dependency>
    <dependentAssembly>
      <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0"
        processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
    </dependentAssembly>
  </dependency>
</assembly>"#;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon_with_id("../../assets/icon.ico", "1");
        res.set_manifest(MANIFEST);
        res.set("ProductName", "PowerKit");
        res.set("FileDescription", "PowerKit");
        res.set("LegalCopyright", "PowerKit contributors");
        if let Err(e) = res.compile() {
            // A missing resource compiler must not make the build impossible; the
            // executable then simply has no embedded icon.
            println!("cargo:warning=could not embed Windows resources: {e}");
        }
    }
    build_menu_server();
}

/// Build powerkit_menu.dll (the Explorer menu's COM server) for the same target and
/// leave it in OUT_DIR, where src/explorer_menu.rs embeds it. It is a separate cargo
/// run because a workspace build cannot turn one crate into both a feature-less
/// library for powerkit.exe and a DLL with the `server` feature.
fn build_menu_server() {
    println!("cargo:rerun-if-changed=../../crates/powerkit-menu/src");
    println!("cargo:rerun-if-changed=../../crates/powerkit-menu/Cargo.toml");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    let dll = out.join("powerkit_menu.dll");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        std::fs::write(&dll, []).expect("writing a placeholder into OUT_DIR");
        return;
    }
    let target = std::env::var("TARGET").expect("cargo sets TARGET");
    let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets it"));
    let root = manifest_dir.join("../..");
    // Its own target folder: the outer build holds the lock on the usual one.
    let target_dir = root.join("target").join("menu-server");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let status = std::process::Command::new(cargo)
        .current_dir(&root)
        .args([
            "build",
            "--release",
            "--offline",
            "-p",
            "powerkit-menu",
            "--features",
            "server",
            "--target",
        ])
        .arg(&target)
        .arg("--target-dir")
        .arg(&target_dir)
        .env_remove("CARGO_TARGET_DIR")
        .status()
        .expect("starting cargo to build powerkit_menu.dll");
    assert!(status.success(), "building powerkit_menu.dll failed");
    let built = target_dir.join(&target).join("release").join("powerkit_menu.dll");
    std::fs::copy(&built, &dll).unwrap_or_else(|e| panic!("copying {}: {e}", built.display()));
}
