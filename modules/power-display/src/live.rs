//! The window, and a worker thread that owns the monitor handles: every DDC/CI call
//! goes over the video cable and takes tens of milliseconds, too slow for the event
//! loop thread.

use crate::{parse_vcp, summary, validate, Feature, Monitor, Reading};
use anyhow::{bail, Context as _, Result};
use powerkit_config::PowerDisplay as Settings;
use powerkit_core::{Event, PowerDisplayEvent};
use powerkit_platform::display_window::{Control, PowerDisplayWindow, Section};
use powerkit_platform::{ddc, event_loop};
use powerkit_plugin_api::Ctx;
use std::collections::{BTreeMap, HashMap};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

/// How often the worker is checked for answers while it has work.
const POLL_MS: u32 = 50;

enum Job {
    Read,
    Set {
        display: usize,
        name: String,
        feature: Feature,
        value: u32,
    },
}

enum Reply {
    Read(Result<Vec<Monitor>, String>),
    Set {
        name: String,
        feature: Feature,
        result: Result<(), String>,
    },
}

fn read(
    displays: &mut Vec<ddc::Display>,
    caps: &mut HashMap<(String, String), BTreeMap<u8, Vec<u8>>>,
) -> Result<Vec<Monitor>, String> {
    // Release the old handles before asking for new ones.
    displays.clear();
    *displays = ddc::displays().map_err(|e| e.to_string())?;
    Ok(displays
        .iter()
        .map(|d| {
            let key = (d.device.clone(), d.name.clone());
            // The capabilities request takes seconds, and only answers change.
            if !caps.contains_key(&key) {
                match d.capabilities().map(|c| parse_vcp(&c)) {
                    Ok(Some(vcp)) => {
                        caps.insert(key.clone(), vcp);
                    }
                    Ok(None) => tracing::debug!(monitor = %d.name, "capabilities list no VCP codes"),
                    Err(e) => tracing::debug!(monitor = %d.name, "no capabilities: {e}"),
                }
            }
            let listed = caps.get(&key);
            let controls = Feature::ALL
                .into_iter()
                .filter_map(|f| {
                    // Without a capabilities list only levels are tried: picks need
                    // the list of values to offer.
                    let values = match listed {
                        Some(l) => l.get(&f.code())?.as_slice(),
                        None if f.is_level() => &[],
                        None => return None,
                    };
                    let (current, max) = d.get(f.code()).ok()?;
                    if f.is_level() {
                        (max > 0).then_some((f, Reading::Level { current, max }))
                    } else {
                        // Picks are one byte; some monitors set bits above it.
                        Some((f, Reading::pick(current & 0xFF, values)))
                    }
                })
                .collect();
            Monitor {
                name: d.name.clone(),
                device: d.device.clone(),
                controls,
            }
        })
        .collect())
}

fn run(jobs: Receiver<Job>, replies: Sender<Reply>) {
    let mut displays = Vec::new();
    let mut caps = HashMap::new();
    while let Ok(first) = jobs.recv() {
        let mut batch = vec![first];
        batch.extend(jobs.try_iter());
        for i in 0..batch.len() {
            let reply = match &batch[i] {
                Job::Read => Reply::Read(read(&mut displays, &mut caps)),
                Job::Set {
                    display,
                    name,
                    feature,
                    value,
                } => {
                    // A slider drag queues many values; only the last one is sent.
                    let superseded = batch[i + 1..]
                        .iter()
                        .any(|j| matches!(j, Job::Set { display: d, feature: f, .. } if d == display && f == feature));
                    let result = match displays.get(*display) {
                        _ if superseded => Ok(()),
                        Some(d) if d.name == *name => d.set(feature.code(), *value).map_err(|e| e.to_string()),
                        _ => Err("the monitors changed; press Refresh".to_string()),
                    };
                    Reply::Set {
                        name: name.clone(),
                        feature: *feature,
                        result,
                    }
                }
            };
            if replies.send(reply).is_err() {
                return;
            }
        }
    }
}

struct Worker {
    jobs: Sender<Job>,
    replies: Receiver<Reply>,
}

#[derive(Default)]
pub struct Live {
    window: Option<PowerDisplayWindow>,
    worker: Option<Worker>,
    monitors: Vec<Monitor>,
    /// The monitor and the control in each shown section and row.
    rows: Vec<(usize, Vec<Feature>)>,
    message: String,
    /// Jobs sent that have not been answered yet.
    pending: usize,
    timer: Option<usize>,
}

impl Live {
    pub fn monitors(&self) -> &[Monitor] {
        &self.monitors
    }

    pub fn busy(&self) -> bool {
        self.pending > 0
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    fn send(&mut self, job: Job) -> Result<()> {
        let worker = match &mut self.worker {
            Some(w) => w,
            None => {
                let (jobs, rx) = mpsc::channel();
                let (tx, replies) = mpsc::channel();
                std::thread::Builder::new()
                    .name("power-display".into())
                    .spawn(move || run(rx, tx))
                    .context("starting the Power Display worker")?;
                self.worker.insert(Worker { jobs, replies })
            }
        };
        if worker.jobs.send(job).is_err() {
            self.worker = None;
            self.pending = 0;
            bail!("the Power Display worker stopped");
        }
        self.pending += 1;
        if self.timer.is_none() {
            self.timer = Some(event_loop::set_timer(POLL_MS)?);
        }
        Ok(())
    }

    pub fn open(&mut self, settings: &Settings) -> Result<()> {
        let window = match &mut self.window {
            Some(w) => w,
            None => self.window.insert(PowerDisplayWindow::new()?),
        };
        window.show();
        self.refresh()?;
        self.render(settings)
    }

    pub fn toggle(&mut self, settings: &Settings) -> Result<()> {
        match &mut self.window {
            Some(w) if w.is_visible() => {
                w.hide();
                Ok(())
            }
            _ => self.open(settings),
        }
    }

    pub fn refresh(&mut self) -> Result<()> {
        self.send(Job::Read)?;
        self.message = "Reading the monitors...".into();
        Ok(())
    }

    pub fn close(&mut self) {
        self.window = None;
        // The worker releases the monitor handles when its job channel closes.
        self.worker = None;
        self.pending = 0;
        if let Some(t) = self.timer.take() {
            event_loop::kill_timer(t);
        }
    }

    /// Sets `feature` on the monitor at `display` in [`Live::monitors`].
    pub fn set(&mut self, display: usize, feature: Feature, value: u32) -> Result<()> {
        let m = self
            .monitors
            .get(display)
            .with_context(|| format!("there is no monitor {display}; open Power Display or refresh first"))?;
        validate(m, feature, value)?;
        let name = m.name.clone();
        self.send(Job::Set {
            display,
            name,
            feature,
            value,
        })?;
        self.monitors[display].update(feature, value);
        Ok(())
    }

    /// Shows the monitors and the message in the window, if it is open.
    pub fn render(&mut self, settings: &Settings) -> Result<()> {
        let Some(window) = &mut self.window else {
            return Ok(());
        };
        self.rows.clear();
        let mut sections = Vec::new();
        for (i, m) in self.monitors.iter().enumerate() {
            let shown: Vec<_> = m.shown(settings).collect();
            if shown.is_empty() {
                continue;
            }
            let controls = shown
                .iter()
                .map(|(f, r)| match r {
                    Reading::Level { current, max } => Control::Slider {
                        label: f.label().into(),
                        value: *current,
                        max: *max,
                    },
                    Reading::Pick { current, options } => Control::Choice {
                        label: f.label().into(),
                        options: options.iter().map(|v| f.value_name(*v)).collect(),
                        selected: options.iter().position(|v| v == current),
                        warning: f.warning().map(String::from),
                    },
                })
                .collect();
            self.rows.push((i, shown.iter().map(|(f, _)| *f).collect()));
            sections.push(Section {
                title: m.name.clone(),
                controls,
            });
        }
        window.set_sections(sections)?;
        window.set_status(&self.message);
        Ok(())
    }

    fn poll(&mut self, settings: &Settings) -> Result<()> {
        let Some(worker) = &self.worker else { return Ok(()) };
        let mut stopped = false;
        loop {
            match worker.replies.try_recv() {
                Ok(reply) => {
                    self.pending = self.pending.saturating_sub(1);
                    match reply {
                        Reply::Read(Ok(monitors)) => {
                            self.message = summary(&monitors);
                            self.monitors = monitors;
                        }
                        Reply::Read(Err(e)) => self.message = format!("The monitors could not be read: {e}"),
                        Reply::Set {
                            name,
                            feature,
                            result: Err(e),
                        } => self.message = format!("{} could not be set on {name}: {e}", feature.label()),
                        Reply::Set { .. } => {}
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    stopped = true;
                    break;
                }
            }
        }
        if stopped {
            self.worker = None;
            self.pending = 0;
            self.message = "Power Display stopped talking to the monitors; press Refresh to try again.".into();
        }
        if self.pending == 0 {
            if let Some(t) = self.timer.take() {
                event_loop::kill_timer(t);
            }
        }
        self.render(settings)
    }

    fn on_window(&mut self, e: &PowerDisplayEvent, settings: &Settings) -> Result<()> {
        let at = |section: usize, row: usize| -> Option<(usize, Feature)> {
            let (m, features) = self.rows.get(section)?;
            Some((*m, *features.get(row)?))
        };
        match *e {
            PowerDisplayEvent::Slider { section, row, value } => {
                if let Some((m, f)) = at(section, row) {
                    self.set(m, f, value)?;
                }
            }
            PowerDisplayEvent::Choice { section, row, index } => {
                if let Some((m, f)) = at(section, row) {
                    let value = match self.monitors[m].reading(f) {
                        Some(Reading::Pick { options, .. }) => options.get(index).copied(),
                        _ => None,
                    };
                    if let Some(v) = value {
                        self.set(m, f, v)?;
                    }
                }
            }
            PowerDisplayEvent::Refresh => {
                self.refresh()?;
                self.render(settings)?;
            }
            PowerDisplayEvent::Closed => {}
        }
        Ok(())
    }

    pub fn on_event(&mut self, event: &Event, settings: &Settings, ctx: &mut Ctx) -> Result<()> {
        let result = match event {
            Event::Timer(id) if Some(*id) == self.timer => self.poll(settings),
            Event::PowerDisplay(e) if self.window.is_some() => self.on_window(e, settings),
            Event::DisplayChanged if self.window.as_ref().is_some_and(|w| w.is_visible()) => {
                self.refresh().and_then(|()| self.render(settings))
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            // Shown where the user is looking when the window is open.
            match &mut self.window {
                Some(w) if w.is_visible() => w.set_status(&format!("{e:#}")),
                _ => ctx.notify("Power Display", format!("{e:#}")),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Talks to the real monitors and writes each one's brightness back unchanged.
    /// External monitors answer only with DDC/CI on in their menus. Run with `--ignored`.
    #[test]
    #[ignore]
    fn reads_and_writes_the_attached_monitors() {
        let mut displays = Vec::new();
        let monitors = read(&mut displays, &mut HashMap::new()).unwrap();
        for (d, m) in displays.iter().zip(&monitors) {
            println!("{}: {:?}", m.name, m.controls);
            if let Some(Reading::Level { current, .. }) = m.reading(Feature::Brightness) {
                d.set(Feature::Brightness.code(), *current).unwrap();
                assert_eq!(d.get(Feature::Brightness.code()).unwrap().0, *current);
            }
        }
    }
}
