//! What PowerKit adds to File Explorer: preview pane handlers and thumbnail providers
//! for file types Windows does not show by itself.
//!
//! Built with the `server` feature, this crate is `powerkit_preview.dll`, the
//! in-process COM server Explorer loads (preview handlers run in its prevhost.exe
//! helper, thumbnail providers in Explorer or a dllhost.exe). Without the feature it
//! is the list of handlers powerkit.exe registers. The file decoders are plain Rust;
//! drawing SVG and PDF and decoding PNG/JPEG goes through Windows in `render`.

pub mod code;
pub mod gcode;
pub mod image;
pub mod markdown;
pub mod qoi;
pub mod rtf;
pub mod stl;
pub mod svg;

#[cfg(all(windows, any(feature = "server", test)))]
pub mod render;
#[cfg(all(windows, feature = "server"))]
mod server;

/// File name of the DLL.
pub const DLL: &str = "powerkit_preview.dll";

/// Shell extension key under `<ext>\shellex` for a preview handler.
pub const PREVIEW_IID: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";
/// Shell extension key under `<ext>\shellex` for a thumbnail provider.
pub const THUMBNAIL_IID: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
/// The 64-bit prevhost.exe surrogate preview handlers run in.
pub const PREVHOST_APPID: &str = "{6d2b5079-2f0b-48dd-ab7f-97cec514d30b}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Markdown,
    Code,
    Svg,
    Pdf,
    Qoi,
    /// G-code text or Prusa binary G-code; both carry the slicer's pictures of the print.
    Gcode,
    Stl,
}

impl Format {
    /// The most of a file read for it. Text past this is cut off; G-code keeps its
    /// pictures at the start.
    pub fn read_limit(self) -> usize {
        const MB: usize = 1 << 20;
        match self {
            Format::Code => MB,
            Format::Markdown | Format::Gcode => 4 * MB,
            Format::Svg => 32 * MB,
            Format::Pdf | Format::Qoi | Format::Stl => 512 * MB,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Preview,
    Thumbnail,
}

#[derive(Debug)]
pub struct Handler {
    /// Setting under `file_explorer` that turns it on.
    pub setting: &'static str,
    pub name: &'static str,
    pub format: Format,
    pub kind: Kind,
    pub clsid: u128,
    pub extensions: &'static [&'static str],
}

impl Handler {
    pub fn clsid_string(&self) -> String {
        let c = self.clsid;
        format!(
            "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
            c >> 96,
            (c >> 80) & 0xffff,
            (c >> 64) & 0xffff,
            (c >> 48) & 0xffff,
            c & 0xffff_ffff_ffff
        )
    }

    pub fn shellex_iid(&self) -> &'static str {
        match self.kind {
            Kind::Preview => PREVIEW_IID,
            Kind::Thumbnail => THUMBNAIL_IID,
        }
    }
}

pub const MARKDOWN_EXTENSIONS: &[&str] = &[".md", ".markdown", ".mdown", ".mkd", ".mkdn", ".mdwn", ".mdtxt"];

/// Source and configuration files the code preview takes. Plain .txt and .xml are left
/// to Windows, which previews them already.
pub const CODE_EXTENSIONS: &[&str] = &[
    ".bash", ".bat", ".c", ".cc", ".cfg", ".cjs", ".cmd", ".conf", ".cpp", ".cs", ".csproj", ".css", ".cxx", ".dart",
    ".go", ".gradle", ".h", ".hpp", ".htm", ".html", ".ini", ".java", ".js", ".json", ".jsonc", ".jsx", ".kt", ".kts",
    ".less", ".lua", ".mjs", ".php", ".ps1", ".psd1", ".psm1", ".py", ".pyw", ".r", ".rb", ".rs", ".scala", ".scss",
    ".sh", ".sql", ".swift", ".toml", ".ts", ".tsx", ".vue", ".xaml", ".yaml", ".yml", ".zsh",
];

pub const HANDLERS: [Handler; 11] = [
    Handler {
        setting: "markdown_preview",
        name: "PowerKit Markdown preview",
        format: Format::Markdown,
        kind: Kind::Preview,
        clsid: 0x27687599_3840_4022_bde6_28c222a51acf,
        extensions: MARKDOWN_EXTENSIONS,
    },
    Handler {
        setting: "code_preview",
        name: "PowerKit source code preview",
        format: Format::Code,
        kind: Kind::Preview,
        clsid: 0x8773ff7f_919d_4ae9_9984_1a8eae201d66,
        extensions: CODE_EXTENSIONS,
    },
    Handler {
        setting: "svg_preview",
        name: "PowerKit SVG preview",
        format: Format::Svg,
        kind: Kind::Preview,
        clsid: 0x29e0213f_db2c_4f22_a88a_bc679ad95327,
        extensions: &[".svg"],
    },
    Handler {
        setting: "svg_thumbnails",
        name: "PowerKit SVG thumbnails",
        format: Format::Svg,
        kind: Kind::Thumbnail,
        clsid: 0x9a113a1e_a3f1_4f9b_ba00_992691b1fa45,
        extensions: &[".svg"],
    },
    Handler {
        setting: "pdf_preview",
        name: "PowerKit PDF preview",
        format: Format::Pdf,
        kind: Kind::Preview,
        clsid: 0xdc1176a7_c5f9_4859_b32c_7371071c5d89,
        extensions: &[".pdf"],
    },
    Handler {
        setting: "pdf_thumbnails",
        name: "PowerKit PDF thumbnails",
        format: Format::Pdf,
        kind: Kind::Thumbnail,
        clsid: 0xf6a070e5_c346_4cf3_89cf_2d3e0bdd2068,
        extensions: &[".pdf"],
    },
    Handler {
        setting: "qoi_preview",
        name: "PowerKit QOI preview",
        format: Format::Qoi,
        kind: Kind::Preview,
        clsid: 0xad73b28b_1089_461a_add5_cdd0c33b7100,
        extensions: &[".qoi"],
    },
    Handler {
        setting: "qoi_thumbnails",
        name: "PowerKit QOI thumbnails",
        format: Format::Qoi,
        kind: Kind::Thumbnail,
        clsid: 0x9146f83e_3453_4356_8e0f_0fc72690726f,
        extensions: &[".qoi"],
    },
    Handler {
        setting: "gcode_preview",
        name: "PowerKit G-code preview",
        format: Format::Gcode,
        kind: Kind::Preview,
        clsid: 0xa1df490e_a791_425a_a82a_e1509aad179d,
        extensions: &[".gcode", ".bgcode"],
    },
    Handler {
        setting: "gcode_thumbnails",
        name: "PowerKit G-code thumbnails",
        format: Format::Gcode,
        kind: Kind::Thumbnail,
        clsid: 0x10e4f0ee_7efa_45dd_b412_a584ae89e277,
        extensions: &[".gcode", ".bgcode"],
    },
    Handler {
        setting: "stl_thumbnails",
        name: "PowerKit STL thumbnails",
        format: Format::Stl,
        kind: Kind::Thumbnail,
        clsid: 0x07a8ef30_2433_46ac_924d_7a1a98913185,
        extensions: &[".stl"],
    },
];

/// Text from file bytes: UTF-8 or UTF-16 with a byte order mark, otherwise UTF-8 with
/// bad sequences replaced.
pub fn decode_text(bytes: &[u8]) -> String {
    let utf16 = |be: bool| {
        let units: Vec<u16> = bytes[2..]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| {
                if be {
                    u16::from_be_bytes([c[0], c[1]])
                } else {
                    u16::from_le_bytes([c[0], c[1]])
                }
            })
            .collect();
        String::from_utf16_lossy(&units)
    };
    match bytes {
        [0xef, 0xbb, 0xbf, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        [0xff, 0xfe, ..] => utf16(false),
        [0xfe, 0xff, ..] => utf16(true),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handlers_have_distinct_class_ids_and_dotted_extensions() {
        for (i, h) in HANDLERS.iter().enumerate() {
            assert!(HANDLERS[i + 1..].iter().all(|o| o.clsid != h.clsid), "{}", h.name);
            assert!(h.extensions.iter().all(|e| e.starts_with('.') && e.len() > 1));
        }
        assert_eq!(HANDLERS[0].clsid_string(), "{27687599-3840-4022-bde6-28c222a51acf}");
    }

    #[test]
    fn text_is_decoded_from_any_byte_order_mark() {
        assert_eq!(decode_text(b"\xef\xbb\xbfhi"), "hi");
        assert_eq!(decode_text(b"\xff\xfeh\0i\0"), "hi");
        assert_eq!(decode_text(b"\xfe\xff\0h\0i"), "hi");
        assert_eq!(decode_text(b"h\xffi"), "h\u{fffd}i");
    }
}
