//! The entries PowerKit adds to the top level of the Windows 11 Explorer context menu.
//!
//! Windows 11 only shows a program's entries there when a packaged COM server
//! implementing `IExplorerCommand` provides them. This crate is that server (built with
//! the `server` feature into `powerkit_menu.dll`) and the list of entries, which
//! powerkit.exe also uses to write the package manifest. Like powerkit-platform, it
//! contains `unsafe` code: a COM server is FFI by nature.
//!
//! Explorer and the DLL share two things, both under `HKCU\Software\PowerKit\Menu`:
//! `Exe`, the powerkit.exe to start, and one DWORD per entry, named after its module,
//! that is 1 while the entry should show.

#[cfg(windows)]
mod reg;
#[cfg(all(windows, feature = "server"))]
mod server;

/// Registry key the DLL reads, under HKEY_CURRENT_USER.
pub const KEY: &str = r"Software\PowerKit\Menu";
/// Value under [`KEY`] holding the full path of powerkit.exe.
pub const EXE_VALUE: &str = "Exe";
/// DWORD under [`KEY`]: 1 while the package is registered, so the entries show at
/// the top of the menu and the classic verbs must not be added as well.
pub const ACTIVE_VALUE: &str = "Active";

/// Record whether the entry of `module` should show (`wanted`). Returns whether it
/// shows at the top of the menu; when not, the module adds its classic verb instead.
/// A value that was never written is left absent while the entry is not wanted, so a
/// turned-off utility does not write to the registry.
#[cfg(windows)]
pub fn show(module: &str, wanted: bool) -> windows_core::Result<bool> {
    let shown = wanted && reg::dword(ACTIVE_VALUE) == Some(1);
    if shown || reg::dword(module).is_some() {
        reg::set_dword(module, shown.into())?;
    }
    Ok(shown)
}

/// What an entry passes to powerkit.exe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Takes {
    /// Every selected item.
    All,
    /// Only the first selected item, or the folder whose background was clicked.
    First,
}

#[derive(Debug)]
pub struct Entry {
    /// Module id: names the registry value and the icon file `<module>.ico` next to the DLL.
    pub module: &'static str,
    pub title: &'static str,
    /// The powerkit.exe option the paths follow.
    pub flag: &'static str,
    pub clsid: u128,
    /// Explorer item types the entry is offered for: `*` is any file, `Directory` a
    /// folder, `Directory\Background` the empty space in an open folder, `.reg` a file
    /// type.
    pub types: &'static [&'static str],
    pub takes: Takes,
}

pub const ENTRIES: [Entry; 5] = [
    Entry {
        module: "power_rename",
        title: "Rename with PowerKit",
        flag: "--rename",
        clsid: 0x1ddf8095_16f5_4beb_9f85_a972c3244b8a,
        types: &["*", "Directory", r"Directory\Background"],
        takes: Takes::All,
    },
    Entry {
        module: "new_plus",
        title: "New+",
        flag: "--new",
        clsid: 0x79e81a2a_d71f_4315_84f0_e1e0d35de66d,
        types: &["Directory", r"Directory\Background"],
        takes: Takes::First,
    },
    Entry {
        module: "registry_preview",
        title: "Preview with PowerKit",
        flag: "--reg",
        clsid: 0x35da73ab_fe59_4b83_939f_91c5f9f50cb0,
        types: &[".reg"],
        takes: Takes::First,
    },
    Entry {
        module: "file_locksmith",
        title: "Unlock with PowerKit",
        flag: "--unlock",
        clsid: 0x121ff203_8383_4256_8fb1_7d574082e886,
        types: &["*", "Directory"],
        takes: Takes::All,
    },
    Entry {
        module: "image_resizer",
        title: "Resize with PowerKit",
        flag: "--resize",
        clsid: 0x5c3e9a40_7d1b_4f6e_a2c8_93b1e4d07f25,
        types: &[
            ".bmp", ".dib", ".gif", ".heic", ".heif", ".ico", ".jfif", ".jpe", ".jpeg", ".jpg", ".jxr", ".png", ".tif",
            ".tiff", ".wdp", ".webp",
        ],
        takes: Takes::All,
    },
];

/// `{XXXXXXXX-XXXX-XXXX-XXXX-XXXXXXXXXXXX}` in upper case, as manifests write CLSIDs.
pub fn clsid_text(clsid: u128) -> String {
    let h = format!("{clsid:032X}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

/// The command line that opens `paths` with `entry`. Each path is quoted; a trailing
/// backslash (a drive root) is doubled so it does not escape the closing quote.
pub fn command_line(exe: &str, entry: &Entry, paths: &[String]) -> String {
    let quote = |s: &str| {
        if s.ends_with('\\') {
            format!("\"{s}\\\"")
        } else {
            format!("\"{s}\"")
        }
    };
    let paths = match entry.takes {
        Takes::All => paths,
        Takes::First => &paths[..paths.len().min(1)],
    };
    let mut line = format!("{} {}", quote(exe), entry.flag);
    for p in paths {
        line.push(' ');
        line.push_str(&quote(p));
    }
    line
}

/// Package name in the manifest; Windows lists the installed package under it.
pub const PACKAGE: &str = "PowerKit.ExplorerMenu";
/// File name of the server inside the package's external folder.
pub const DLL: &str = "powerkit_menu.dll";
/// File name of the logo inside the package.
pub const LOGO: &str = "logo.png";

/// The package manifest that hands [`ENTRIES`] to Explorer. `publisher` must equal
/// the subject of the certificate that signs the package; `version` has four parts.
pub fn manifest(publisher: &str, version: &str) -> String {
    let mut types = String::new();
    for e in &ENTRIES {
        for t in e.types {
            types += &format!(
                "            <desktop5:ItemType Type=\"{}\"><desktop5:Verb Id=\"{}\" Clsid=\"{}\" /></desktop5:ItemType>\n",
                xml(t),
                // Verb ids may only hold letters and digits.
                e.module.replace('_', ""),
                clsid_text(e.clsid)
            );
        }
    }
    let classes: String = ENTRIES
        .iter()
        .map(|e| {
            format!(
                "              <com:Class Id=\"{}\" Path=\"{DLL}\" ThreadingModel=\"STA\" />\n",
                clsid_text(e.clsid)
            )
        })
        .collect();
    let publisher = xml(publisher);
    // The Application is never started (no list entry, and the external folder holds no
    // powerkit.exe); Windows only accepts the menu extension as part of one.
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10"
  xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10"
  xmlns:uap10="http://schemas.microsoft.com/appx/manifest/uap/windows10/10"
  xmlns:rescap="http://schemas.microsoft.com/appx/manifest/foundation/windows10/restrictedcapabilities"
  xmlns:desktop4="http://schemas.microsoft.com/appx/manifest/desktop/windows10/4"
  xmlns:desktop5="http://schemas.microsoft.com/appx/manifest/desktop/windows10/5"
  xmlns:com="http://schemas.microsoft.com/appx/manifest/com/windows10"
  IgnorableNamespaces="uap uap10 rescap desktop4 desktop5 com">
  <Identity Name="{PACKAGE}" Publisher="{publisher}" Version="{version}" ProcessorArchitecture="neutral" />
  <Properties>
    <DisplayName>PowerKit Explorer menu</DisplayName>
    <PublisherDisplayName>PowerKit</PublisherDisplayName>
    <Logo>{LOGO}</Logo>
    <uap10:AllowExternalContent>true</uap10:AllowExternalContent>
  </Properties>
  <Resources><Resource Language="en-us" /></Resources>
  <Dependencies><TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.22000.0" MaxVersionTested="10.0.26100.0" /></Dependencies>
  <Capabilities><rescap:Capability Name="runFullTrust" /><rescap:Capability Name="unvirtualizedResources" /></Capabilities>
  <Applications>
    <Application Id="Menu" Executable="powerkit.exe" uap10:TrustLevel="mediumIL" uap10:RuntimeBehavior="win32App">
      <uap:VisualElements AppListEntry="none" DisplayName="PowerKit Explorer menu" Description="PowerKit entries in the Explorer context menu" BackgroundColor="transparent" Square150x150Logo="{LOGO}" Square44x44Logo="{LOGO}" />
      <Extensions>
        <desktop4:Extension Category="windows.fileExplorerContextMenus">
          <desktop4:FileExplorerContextMenus>
{types}          </desktop4:FileExplorerContextMenus>
        </desktop4:Extension>
        <com:Extension Category="windows.comServer">
          <com:ComServer>
            <com:SurrogateServer DisplayName="PowerKit Explorer menu">
{classes}            </com:SurrogateServer>
          </com:ComServer>
        </com:Extension>
      </Extensions>
    </Application>
  </Applications>
</Package>
"#
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsids_are_written_the_way_manifests_expect() {
        assert_eq!(clsid_text(ENTRIES[0].clsid), "1DDF8095-16F5-4BEB-9F85-A972C3244B8A");
    }

    #[test]
    fn the_manifest_offers_every_entry_for_its_item_types() {
        let m = manifest("CN=A & B", "1.2.3.0");
        assert!(m.contains(r#"Publisher="CN=A &amp; B" Version="1.2.3.0""#));
        assert_eq!(m.matches("<desktop5:ItemType ").count(), 24);
        assert!(m.contains(r#"<desktop5:ItemType Type=".reg"><desktop5:Verb Id="registrypreview" Clsid="35DA73AB-FE59-4B83-939F-91C5F9F50CB0" />"#));
        assert_eq!(m.matches(r#"Path="powerkit_menu.dll""#).count(), 5);
    }

    #[test]
    fn entries_have_distinct_clsids() {
        for (i, a) in ENTRIES.iter().enumerate() {
            assert!(ENTRIES[i + 1..].iter().all(|b| b.clsid != a.clsid), "{}", a.module);
        }
    }

    #[test]
    fn command_lines_quote_paths_and_keep_drive_roots_intact() {
        let paths = vec![r"C:\a b\x.txt".to_string(), r"D:\".to_string()];
        assert_eq!(
            command_line(r"C:\p\powerkit.exe", &ENTRIES[3], &paths),
            r#""C:\p\powerkit.exe" --unlock "C:\a b\x.txt" "D:\\""#
        );
        assert_eq!(
            command_line(r"C:\p\powerkit.exe", &ENTRIES[1], &paths),
            r#""C:\p\powerkit.exe" --new "C:\a b\x.txt""#
        );
    }
}
