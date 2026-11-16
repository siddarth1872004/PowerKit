//! The Image Resizer window: pick or type a size, then resize the pictures chosen in
//! Explorer. Resizing big photos takes a while, so it runs on a worker thread and
//! the outcome comes back by wake-up.

use crate::Host;
use image_resizer::plan::{self, Target};
use powerkit_config::{FallbackFormat, ImageResizer as Settings, ResizeFit, ResizeSize, ResizeUnit};
use powerkit_core::TableEvent;
use powerkit_platform::event_loop::Waker;
use powerkit_platform::image::{self, Format, SaveOptions};
use powerkit_platform::table_window::{Field, TableRow, TableSpec, TableWindow};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::{Duration, Instant};

pub const TABLE: &str = "image_resizer";
/// Explorer starts PowerKit once per selected picture when many are chosen; requests
/// this close together are one selection.
const SAME_SELECTION: Duration = Duration::from_secs(2);

const UNITS: [(ResizeUnit, &str); 4] = [
    (ResizeUnit::Pixel, "Pixels"),
    (ResizeUnit::Percent, "Percent"),
    (ResizeUnit::Centimeter, "Centimeters"),
    (ResizeUnit::Inch, "Inches"),
];
const FITS: [(ResizeFit, &str); 3] = [
    (ResizeFit::Fit, "Fit"),
    (ResizeFit::Fill, "Fill"),
    (ResizeFit::Stretch, "Stretch"),
];

/// The columns and boxes of a list of sizes, here and in the settings.
pub const COLUMNS: &[(&str, i32)] = &[("Size", 170), ("Width", 80), ("Height", 80), ("Unit", 100), ("Fit", 90)];
pub const FIELDS: &[Field] = &[
    Field {
        label: "Size",
        label_width: 30,
        width: 0,
        capture: false,
    },
    Field {
        label: "W",
        label_width: 16,
        width: 60,
        capture: false,
    },
    Field {
        label: "H",
        label_width: 14,
        width: 60,
        capture: false,
    },
    Field {
        label: "Unit",
        label_width: 28,
        width: 90,
        capture: false,
    },
    Field {
        label: "Fit",
        label_width: 20,
        width: 70,
        capture: false,
    },
];

static SPEC: TableSpec = TableSpec {
    id: TABLE,
    title: "Image Resizer",
    columns: COLUMNS,
    fields: FIELDS,
    checks: false,
    choices: &[
        "Save resized copies",
        "Save resized copies, never enlarge",
        "Replace the originals",
        "Replace the originals, never enlarge",
    ],
    buttons: ["Add size", "Resize", "Delete size", "", ""],
};

fn choice(s: &Settings) -> usize {
    usize::from(s.shrink_only) + 2 * usize::from(s.replace_originals)
}

fn side_text(v: f64) -> String {
    if v > 0.0 {
        plan::number(v)
    } else {
        "auto".into()
    }
}

pub fn rows(sizes: &[ResizeSize]) -> Vec<TableRow> {
    sizes
        .iter()
        .map(|s| {
            let unit = UNITS.iter().find(|u| u.0 == s.unit).map_or("", |u| u.1);
            let fit = FITS.iter().find(|f| f.0 == s.fit).map_or("", |f| f.1);
            (
                false,
                vec![
                    s.name.clone(),
                    side_text(s.width),
                    side_text(s.height),
                    unit.into(),
                    fit.into(),
                ],
            )
        })
        .collect()
}

/// A size from the boxes under the list, read leniently: units and fits by any
/// prefix of their name (or `px`, `%`, `cm`, `in`), a blank side or `auto` as 0.
pub fn parse_size(fields: &[String]) -> Result<ResizeSize, String> {
    let field = |i: usize| fields.get(i).map_or("", |f| f.trim());
    let side = |i: usize, what: &str| -> Result<f64, String> {
        let t = field(i);
        if t.is_empty() || t.eq_ignore_ascii_case("auto") {
            return Ok(0.0);
        }
        t.replace(',', ".")
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite() && *v >= 0.0)
            .ok_or_else(|| format!("The {what} \"{t}\" is not a number."))
    };
    let (width, height) = (side(1, "width")?, side(2, "height")?);
    if width == 0.0 && height == 0.0 {
        return Err("Give a width, a height or both.".into());
    }
    let unit_text = field(3).to_ascii_lowercase();
    let unit = match unit_text.as_str() {
        "" | "px" => Some(ResizeUnit::Pixel),
        "%" => Some(ResizeUnit::Percent),
        "cm" => Some(ResizeUnit::Centimeter),
        "in" | "\"" => Some(ResizeUnit::Inch),
        t => UNITS
            .iter()
            .find(|u| u.1.to_ascii_lowercase().starts_with(t))
            .map(|u| u.0),
    }
    .ok_or_else(|| {
        format!(
            "Unknown unit \"{}\": use pixels, percent, centimeters or inches.",
            field(3)
        )
    })?;
    let fit_text = field(4).to_ascii_lowercase();
    let fit = if fit_text.is_empty() {
        ResizeFit::Fit
    } else {
        FITS.iter()
            .find(|f| f.1.to_ascii_lowercase().starts_with(&fit_text))
            .map(|f| f.0)
            .ok_or_else(|| format!("Unknown fit \"{}\": use fit, fill or stretch.", field(4)))?
    };
    let name = match field(0) {
        "" => format!("{} x {}", side_text(width), side_text(height)),
        n => n.to_string(),
    };
    Ok(ResizeSize {
        name,
        fit,
        width,
        height,
        unit,
    })
}

fn fallback(f: FallbackFormat) -> Format {
    match f {
        FallbackFormat::Bmp => Format::Bmp,
        FallbackFormat::Gif => Format::Gif,
        FallbackFormat::Jpeg => Format::Jpeg,
        FallbackFormat::Png => Format::Png,
        FallbackFormat::Tiff => Format::Tiff,
    }
}

/// What one resize run did.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    pub resized: usize,
    /// Pictures already small enough when enlarging is not allowed.
    pub skipped: usize,
    pub errors: Vec<String>,
}

fn name_of(p: &Path) -> String {
    p.file_name()
        .map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// Where the resized picture goes, and whether the original is removed afterwards
/// (it is replaced in another format, so under another name).
fn destination(
    src: &Path,
    own_format: bool,
    format: Format,
    target: &Target,
    size: &ResizeSize,
    s: &Settings,
) -> (PathBuf, bool) {
    let folder = src.parent().unwrap_or(Path::new(""));
    let stem = src
        .file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = if own_format {
        src.extension()
            .map(|e| e.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        format.extension().to_string()
    };
    let taken = |p: &Path| p.exists();
    if s.replace_originals {
        if own_format {
            (src.to_path_buf(), false)
        } else {
            (plan::free_path(folder, &stem, &ext, taken), true)
        }
    } else {
        let stem = plan::file_stem(&s.file_name, &stem, size, target.final_size());
        (plan::free_path(folder, &stem, &ext, taken), false)
    }
}

fn resize_one(src: &Path, size: &ResizeSize, s: &Settings) -> Result<bool, String> {
    let picture = image::open(src).map_err(|e| e.to_string())?;
    let Some(target) = plan::target(
        picture.width,
        picture.height,
        picture.dpi,
        size,
        s.shrink_only,
        s.ignore_orientation,
    ) else {
        return Ok(false);
    };
    let format = picture.format.unwrap_or_else(|| fallback(s.fallback_format));
    let (dst, remove_original) = destination(src, picture.format.is_some(), format, &target, size, s);
    let modified = if s.keep_date_modified {
        Some(
            std::fs::metadata(src)
                .and_then(|m| m.modified())
                .map_err(|e| format!("reading its date: {e}"))?,
        )
    } else {
        None
    };
    let options = SaveOptions {
        format,
        size: (target.width, target.height),
        crop: target.crop,
        quality: s.jpeg_quality,
        keep_metadata: !s.remove_metadata,
    };
    picture.save(&dst, &options).map_err(|e| e.to_string())?;
    if let Some(t) = modified {
        std::fs::File::options()
            .write(true)
            .open(&dst)
            .and_then(|f| f.set_modified(t))
            .map_err(|e| format!("saved, but its date was not kept: {e}"))?;
    }
    if remove_original {
        std::fs::remove_file(src)
            .map_err(|e| format!("saved as {}, but the original was not removed: {e}", name_of(&dst)))?;
    }
    Ok(true)
}

pub fn resize_all(paths: &[PathBuf], size: &ResizeSize, s: &Settings) -> Report {
    let mut report = Report::default();
    for p in paths {
        match resize_one(p, size, s) {
            Ok(true) => report.resized += 1,
            Ok(false) => report.skipped += 1,
            Err(e) => report.errors.push(format!("{}: {e}", name_of(p))),
        }
    }
    report
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

pub fn summary(r: &Report) -> String {
    let mut parts = vec![format!("Resized {}.", plural(r.resized, "picture", "pictures"))];
    if r.skipped > 0 {
        parts.push(format!("{} already small enough.", plural(r.skipped, "was", "were")));
    }
    if !r.errors.is_empty() {
        parts.push(format!(
            "{} failed: {}",
            plural(r.errors.len(), "picture", "pictures"),
            r.errors.join("; ")
        ));
    }
    parts.join(" ")
}

fn what(paths: &[PathBuf]) -> String {
    match paths {
        [one] => name_of(one),
        _ => format!("{} pictures", paths.len()),
    }
}

pub struct ResizerUi {
    waker: Waker,
    window: Option<TableWindow>,
    paths: Vec<PathBuf>,
    opened: Option<Instant>,
    working: Option<Receiver<Report>>,
}

impl ResizerUi {
    pub fn new(waker: Waker) -> ResizerUi {
        ResizerUi {
            waker,
            window: None,
            paths: Vec::new(),
            opened: None,
            working: None,
        }
    }

    pub fn open(&mut self, paths: Vec<PathBuf>, host: &Host) -> Result<(), String> {
        if self.window.is_none() {
            let mut w = TableWindow::new(&SPEC).map_err(|e| format!("could not open Image Resizer: {e}"))?;
            crate::icons::apply(image_resizer::ID, |ico| w.set_icon(ico));
            self.window = Some(w);
        }
        // Pictures chosen while a resize runs wait for the next selection.
        if self.working.is_some() {
            return Ok(());
        }
        if !self.opened.is_some_and(|t| t.elapsed() < SAME_SELECTION) {
            self.paths.clear();
        }
        for p in paths {
            if !self.paths.contains(&p) {
                self.paths.push(p);
            }
        }
        self.opened = Some(Instant::now());
        let settings = &host.config().image_resizer;
        let w = self.window.as_mut().expect("created above");
        w.set_title(&format!("Image Resizer - {}", what(&self.paths)));
        w.set_rows(&rows(&settings.sizes), (!settings.sizes.is_empty()).then_some(0));
        w.set_choice(choice(settings));
        w.set_status("Choose a size, change it in the boxes below if you like, then click Resize.");
        w.show();
        Ok(())
    }

    pub fn poll(&mut self) {
        let Some(rx) = &self.working else { return };
        let outcome = match rx.try_recv() {
            Err(TryRecvError::Empty) => return,
            Ok(report) => summary(&report),
            Err(TryRecvError::Disconnected) => "Resizing stopped unexpectedly.".into(),
        };
        self.working = None;
        if let Some(w) = self.window.as_mut() {
            w.set_status(&outcome);
        }
    }

    fn status(&mut self, text: &str) {
        if let Some(w) = self.window.as_mut() {
            w.set_status(text);
        }
    }

    /// Save changed settings, then show the sizes as saved.
    fn save(&mut self, host: &mut Host, change: impl FnOnce(&mut Settings)) {
        let mut config = host.config().clone();
        change(&mut config.image_resizer);
        if let Err(e) = host.handle_request("config.set", &json!({ "config": config })) {
            return self.status(&format!("Not saved: {e}"));
        }
        let sizes = &host.config().image_resizer.sizes;
        if let Some(w) = self.window.as_mut() {
            w.set_rows(&rows(sizes), None);
        }
    }

    pub fn on_event(&mut self, event: TableEvent, host: &mut Host) {
        match event {
            TableEvent::Choice(i) => self.save(host, |s| {
                s.shrink_only = i % 2 == 1;
                s.replace_originals = i >= 2;
            }),
            TableEvent::Add(fields) => match parse_size(&fields) {
                Ok(size) => {
                    let name = size.name.clone();
                    self.save(host, |s| s.sizes.push(size));
                    self.status(&format!("Added the size {name}."));
                }
                Err(e) => self.status(&e),
            },
            TableEvent::Delete(row) => self.save(host, |s| {
                if row < s.sizes.len() {
                    s.sizes.remove(row);
                }
            }),
            TableEvent::Update { fields, .. } => {
                if self.working.is_some() {
                    return self.status("Still resizing the last pictures...");
                }
                let size = match parse_size(&fields) {
                    Ok(s) => s,
                    Err(e) => return self.status(&e),
                };
                let (tx, rx) = mpsc::channel();
                let (waker, paths, settings) = (self.waker, self.paths.clone(), host.config().image_resizer.clone());
                std::thread::spawn(move || {
                    if tx.send(resize_all(&paths, &size, &settings)).is_ok() {
                        waker.wake();
                    }
                });
                self.working = Some(rx);
                let text = format!("Resizing {}...", what(&self.paths));
                self.status(&text);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn sizes_are_read_leniently() {
        let s = parse_size(&fields(&["Web", "1200", "", "px", "fi"])).unwrap();
        assert_eq!(
            (s.name.as_str(), s.width, s.height, s.unit, s.fit),
            ("Web", 1200.0, 0.0, ResizeUnit::Pixel, ResizeFit::Fit)
        );
        let s = parse_size(&fields(&["", "2,5", "auto", "cent", "STRETCH"])).unwrap();
        assert_eq!(
            (s.name.as_str(), s.width, s.unit, s.fit),
            ("2.5 x auto", 2.5, ResizeUnit::Centimeter, ResizeFit::Stretch)
        );
        assert_eq!(
            parse_size(&fields(&["", "50", "50", "%", ""])).unwrap().unit,
            ResizeUnit::Percent
        );
        assert!(parse_size(&fields(&["x", "", "", "", ""])).is_err());
        assert!(parse_size(&fields(&["x", "abc", "1", "", ""])).is_err());
        assert!(parse_size(&fields(&["x", "1", "1", "furlongs", ""])).is_err());
    }

    #[test]
    fn rows_show_the_sizes_as_typed() {
        let s = parse_size(&fields(&["Web", "1200", "", "in", "fill"])).unwrap();
        assert_eq!(rows(&[s])[0].1, ["Web", "1200", "auto", "Inches", "Fill"]);
        assert_eq!(
            rows(
                &parse_size(&fields(&["a", "1", "1", "", ""]))
                    .ok()
                    .into_iter()
                    .collect::<Vec<_>>()
            )[0]
            .1[3],
            "Pixels"
        );
    }

    #[test]
    fn the_summary_counts_what_happened() {
        let r = Report {
            resized: 2,
            skipped: 1,
            errors: vec!["a.png: broken".into()],
        };
        assert_eq!(
            summary(&r),
            "Resized 2 pictures. 1 was already small enough. 1 picture failed: a.png: broken"
        );
    }
}
