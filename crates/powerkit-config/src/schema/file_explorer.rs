use serde::{Deserialize, Serialize};

/// Which preview pane handlers and thumbnail providers PowerKit adds to File Explorer.
/// PDF is off by default: Windows or a PDF reader usually brings its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FileExplorer {
    pub enabled: bool,
    pub markdown_preview: bool,
    pub code_preview: bool,
    pub svg_preview: bool,
    pub svg_thumbnails: bool,
    pub pdf_preview: bool,
    pub pdf_thumbnails: bool,
    pub qoi_preview: bool,
    pub qoi_thumbnails: bool,
    pub gcode_preview: bool,
    pub gcode_thumbnails: bool,
    pub stl_thumbnails: bool,
}

impl Default for FileExplorer {
    fn default() -> Self {
        Self {
            enabled: true,
            markdown_preview: true,
            code_preview: true,
            svg_preview: true,
            svg_thumbnails: true,
            pdf_preview: false,
            pdf_thumbnails: false,
            qoi_preview: true,
            qoi_thumbnails: true,
            gcode_preview: true,
            gcode_thumbnails: true,
            stl_thumbnails: true,
        }
    }
}

impl FileExplorer {
    pub(crate) fn sanitize(&mut self, _w: &mut Vec<String>) {}

    /// The toggle named `setting`; `None` for a name that is not one.
    pub fn handler_on(&self, setting: &str) -> Option<bool> {
        Some(match setting {
            "markdown_preview" => self.markdown_preview,
            "code_preview" => self.code_preview,
            "svg_preview" => self.svg_preview,
            "svg_thumbnails" => self.svg_thumbnails,
            "pdf_preview" => self.pdf_preview,
            "pdf_thumbnails" => self.pdf_thumbnails,
            "qoi_preview" => self.qoi_preview,
            "qoi_thumbnails" => self.qoi_thumbnails,
            "gcode_preview" => self.gcode_preview,
            "gcode_thumbnails" => self.gcode_thumbnails,
            "stl_thumbnails" => self.stl_thumbnails,
            _ => return None,
        })
    }
}
