//! New+'s template menu: shown at the cursor when Explorer's "New+" entry is chosen,
//! it copies the picked template into the folder and selects the copy for renaming.

use new_plus::templates::{self, Options, Stamp, Template};
use powerkit_config::NewPlus;
use powerkit_platform::tray::{MenuItem, Tray};
use std::path::Path;

const OPEN_TEMPLATES: u32 = 100_000;

fn now() -> Stamp {
    let t = powerkit_platform::time::local_now();
    Stamp {
        year: t.year,
        month: t.month,
        day: t.day,
        weekday: t.weekday,
        hour: t.hour,
        minute: t.minute,
        second: t.second,
        millis: t.millis,
    }
}

/// Menus read `&` as an accelerator marker.
fn menu_text(label: &str) -> String {
    label.replace('&', "&&")
}

pub fn show(folder: &Path, settings: &NewPlus, data_dir: &Path, tray: &Tray) -> Result<(), String> {
    if !folder.is_dir() {
        return Err(format!("New+: {} is not a folder.", folder.display()));
    }
    let dir = templates::folder(&settings.templates, data_dir);
    templates::seed(&dir).map_err(|e| format!("New+: could not create {}: {e}", dir.display()))?;
    let found: Vec<Template> =
        templates::list(&dir).map_err(|e| format!("New+: could not read {}: {e}", dir.display()))?;
    let opts = Options::from(settings);
    let mut items: Vec<MenuItem> = found
        .iter()
        .enumerate()
        .map(|(i, t)| MenuItem::new(i as u32 + 1, menu_text(&t.label(opts))))
        .collect();
    if items.is_empty() {
        items.push(MenuItem::Item {
            id: 0,
            label: "No templates yet".into(),
            checked: false,
            enabled: false,
        });
    }
    items.push(MenuItem::Separator);
    items.push(MenuItem::new(OPEN_TEMPLATES, "Open templates folder"));

    let chosen = tray
        .show_menu(&items)
        .map_err(|e| format!("New+: the menu could not be shown: {e}"))?;
    let template = match chosen {
        None => return Ok(()),
        Some(OPEN_TEMPLATES) => {
            return powerkit_platform::shell::open(&dir.to_string_lossy(), None)
                .map_err(|e| format!("New+: could not open {}: {e}", dir.display()));
        }
        Some(id) => found.get(id as usize - 1).ok_or("New+: that template is gone.")?,
    };
    let env = |name: &str| std::env::var(name).ok();
    let created = templates::create(template, folder, opts, now(), &env)
        .map_err(|e| format!("New+: could not create from {}: {e}", template.path.display()))?;
    tracing::info!(path = %created.display(), "created from template");
    std::thread::spawn(move || {
        if let Err(e) = powerkit_platform::shell::select_to_rename(&created) {
            tracing::warn!("could not select {}: {e}", created.display());
        }
    });
    Ok(())
}
