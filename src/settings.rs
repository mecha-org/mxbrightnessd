//! Settings from mxconfig: the `org.mechanix.brightness` app namespace,
//! read from files (no bus), followed live through its inotify fd on the
//! ring, and written back debounced.
//!
//! The user's levels arrive as [`SaveState`] on every change, dozens per
//! second during a slider drag; they are written once, `save_delay` after
//! the last one. A write comes back as a change like any other edit. The
//! module only raises [`SettingsApplied`] when the settings differ from
//! what the core already holds, so its own writes never retarget the
//! fade, and while a save is pending the file's levels are older than the
//! core's, so the pending ones are kept.

use std::fs::File;
use std::io::{ErrorKind, Read};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use app::prelude::*;
use mxconfig::runtime::Drive;
use mxconfig::{Diagnostic, Edit, Event, Origin, Source, Value};
use ring::prelude::*;

use crate::events::{SaveState, Settings, SettingsApplied};
use crate::model::Curve;

mod schema {
    mxconfig::client!("schema/org.mechanix.brightness.toml");
}

use schema::Brightness;

/// The keys a save writes, in one write.
const AUTO: &str = "screen.auto";
const OFFSET: &str = "screen.offset";
const MANUAL: &str = "screen.manual";

/// `poll(2)`'s `POLLIN`.
const POLLIN: u32 = 1;

/// What [`load`] found.
pub struct Loaded {
    /// For [`SettingsModule`].
    pub source: Source,
    pub settings: Settings,
    /// Some layer sets `screen.manual`; else its value is the schema's
    /// default, not a level anyone chose.
    pub manual_saved: bool,
}

/// Open the settings from this system's files, synchronously.
pub fn load() -> Loaded {
    load_from(Source::new())
}

/// Open the settings through `source`. Without the namespace (it failed
/// to open) the settings are the defaults and nothing is saved.
pub fn load_from(source: Source) -> Loaded {
    // Watching before the request reads the files once, not twice;
    // `SettingsModule` takes its own copy of the fd, or logs the failure.
    let _ = source.inotify_fd();
    let config = open(&source);
    let settings = config.as_ref().map_or_else(Settings::default, read);
    let manual_saved = config.as_ref().is_some_and(|config| {
        let origin = config.settings().resolved().origin(MANUAL).cloned();
        !matches!(origin, None | Some(Origin::Schema))
    });
    Loaded {
        source,
        settings,
        manual_saved,
    }
}

/// Request the namespace and take its answer: from files it comes at once.
fn open(source: &Source) -> Option<Brightness> {
    source.request::<Brightness>();
    let mut config = None;
    for outcome in source.outcomes() {
        match outcome.event::<Brightness>() {
            Some(Event::Opened(opened)) => config = Some(opened),
            Some(Event::OpenFailed(e)) => eprintln!("mxbrightnessd: settings did not open: {e}"),
            Some(Event::Diagnosed(diagnostics)) => log(&diagnostics),
            _ => {}
        }
    }
    config
}

/// Log the problems the settings report.
fn log(diagnostics: &[Diagnostic]) {
    for diagnostic in diagnostics {
        eprintln!("mxbrightnessd: settings: {diagnostic}");
    }
}

/// `value` of `key` if finite, else `default`.
fn finite(key: &str, value: f64, default: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        eprintln!("mxbrightnessd: settings: {key} = {value} is not finite; using {default}");
        default
    }
}

/// A list of floats; `None` if any element is not a number.
fn floats(values: &[Value]) -> Option<Vec<f64>> {
    let number = |v: &Value| v.as_float().or_else(|| v.as_integer().map(|i| i as f64));
    values.iter().map(number).collect()
}

/// The curve from `curve.lux` and `curve.level`, else the default one.
fn curve(config: &Brightness) -> Curve {
    let curve = config.curve();
    let built = match (floats(&curve.lux()), floats(&curve.level())) {
        (Some(lux), Some(level)) if lux.len() == level.len() => {
            Curve::new(lux.into_iter().zip(level).collect())
        }
        (Some(lux), Some(level)) => Err(format!(
            "curve.lux has {} points and curve.level {}",
            lux.len(),
            level.len()
        )),
        _ => Err("curve.lux and curve.level must be lists of numbers".to_string()),
    };
    built.unwrap_or_else(|e| {
        eprintln!("mxbrightnessd: settings: bad curve, using the default: {e}");
        Curve::default()
    })
}

/// The settings `config` holds now.
fn read(config: &Brightness) -> Settings {
    let d = Settings::default();
    let screen = config.screen();
    let device = config.device();
    Settings {
        auto: screen.auto(),
        offset: finite(OFFSET, screen.offset(), d.offset),
        manual: finite(MANUAL, screen.manual(), d.manual),
        minimum: finite("screen.minimum", screen.minimum(), d.minimum),
        gamma: finite("screen.gamma", screen.gamma(), d.gamma),
        curve: curve(config),
        backlight: device.backlight(),
        sensor: device.sensor(),
    }
}

/// Follows the settings files and saves the user's levels. Install after
/// `RingModule`, with the source [`load`] returned.
pub struct SettingsModule {
    pub source: Source,
    /// How long after the last [`SaveState`] the levels are written.
    pub save_delay: Duration,
}

struct Follow {
    source: Source,
    /// `None` if the namespace did not open: nothing is followed or saved.
    config: Option<Brightness>,
    /// The inotify fd and the poll armed on it, while it is watched.
    inotify: Option<(File, Token)>,
    save_delay: Duration,
    /// What the core holds, as far as this module knows: the last
    /// settings applied, with the levels of the last [`SaveState`].
    applied: Settings,
    /// Levels not yet written, and when to write them.
    pending: Option<(SaveState, Instant)>,
    /// The save timer in flight.
    timer: Option<Token>,
}
impl Resource for Follow {}

impl Module for SettingsModule {
    /// # Panics
    ///
    /// If no [`Ring`] is installed.
    fn install(self, app: &mut App) {
        let inotify = match self.source.inotify_fd() {
            Ok(fd) => {
                let token = app.resource_mut::<Ring>().poll(fd.as_fd(), POLLIN);
                Some((File::from(fd), token))
            }
            Err(e) => {
                eprintln!("mxbrightnessd: not following the settings files: {e}");
                None
            }
        };
        // Opening again returns the handle `load` opened.
        let config = open(&self.source);
        let applied = config.as_ref().map_or_else(Settings::default, read);
        app.insert_resource(Follow {
            source: self.source,
            config,
            inotify,
            save_delay: self.save_delay,
            applied,
            pending: None,
            timer: None,
        });
        app.system(on_save_state).system(on_io);
    }
}

fn on_save_state(app: &mut App, e: &SaveState) {
    let (mut follow, mut ring) = app.query::<(ResMut<Follow>, ResMut<Ring>)>();
    if follow.config.is_none() {
        return;
    }
    follow.applied.auto = e.auto;
    follow.applied.offset = e.offset;
    follow.applied.manual = e.manual;
    let due = Instant::now() + follow.save_delay;
    follow.pending = Some((*e, due));
    // One timer at a time: when it fires early for a later save, it is armed again.
    if follow.timer.is_none() {
        follow.timer = Some(ring.timeout(follow.save_delay));
    }
}

fn on_io(app: &mut App, e: &IoEvent) {
    let applied = {
        let (mut follow, mut ring) = app.query::<(ResMut<Follow>, ResMut<Ring>)>();
        if follow.timer == Some(e.token) {
            ring.finish(e.token);
            follow.timer = None;
            save_if_due(&mut follow, &mut ring);
        } else if follow.inotify.as_ref().is_some_and(|(_, t)| *t == e.token) {
            ring.finish(e.token);
            watch(&mut follow, &mut ring, e.result);
        } else {
            return;
        }
        follow.drain()
    };
    if let Some(settings) = applied {
        app.signal(SettingsApplied(settings));
    }
}

/// Write the pending levels if they are due, else wait for them again.
fn save_if_due(follow: &mut Follow, ring: &mut Ring) {
    let Some((state, due)) = follow.pending else {
        return;
    };
    let now = Instant::now();
    if now < due {
        follow.timer = Some(ring.timeout(due - now));
        return;
    }
    follow.pending = None;
    let Some(config) = &follow.config else {
        return;
    };
    config.settings().write(&[
        Edit::set(AUTO, Value::Boolean(state.auto)),
        Edit::set(OFFSET, Value::Float(state.offset)),
        Edit::set(MANUAL, Value::Float(state.manual)),
    ]);
}

/// Read the inotify fd after its poll completed with `result`, feed the
/// source, and poll again; on an error stop watching.
fn watch(follow: &mut Follow, ring: &mut Ring, result: i32) {
    let Some((file, _)) = follow.inotify.take() else {
        return;
    };
    if result < 0 {
        let error = std::io::Error::from_raw_os_error(-result);
        eprintln!("mxbrightnessd: settings: watching the files: {error}");
        follow.source.inotify_lost(error);
        return;
    }
    let mut buf = [0u8; 4096];
    loop {
        match (&file).read(&mut buf) {
            Ok(0) => break,
            Ok(n) => follow.source.feed_inotify(&buf[..n]),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => {
                eprintln!("mxbrightnessd: settings: reading inotify: {e}");
                follow.source.inotify_lost(e);
                return;
            }
        }
    }
    let token = ring.poll(file.as_fd(), POLLIN);
    follow.inotify = Some((file, token));
}

impl Follow {
    /// Handle the source's outcomes; the settings to apply, if they
    /// changed from what the core holds.
    fn drain(&mut self) -> Option<Settings> {
        let mut changed = false;
        for outcome in self.source.outcomes() {
            match outcome.event::<Brightness>() {
                Some(Event::Changed(_)) => changed = true,
                Some(Event::Diagnosed(diagnostics)) => log(&diagnostics),
                Some(Event::WriteFailed { error, .. }) => {
                    eprintln!("mxbrightnessd: settings: not saved: {error}");
                }
                Some(Event::Detached(cause)) => {
                    eprintln!("mxbrightnessd: settings: no longer followed: {cause:?}");
                }
                _ => {}
            }
        }
        let config = self.config.as_ref().filter(|_| changed)?;
        let mut settings = read(config);
        // The file's levels are older than the ones waiting to be saved.
        if self.pending.is_some() {
            settings.auto = self.applied.auto;
            settings.offset = self.applied.offset;
            settings.manual = self.applied.manual;
        }
        if settings == self.applied {
            return None;
        }
        self.applied = settings.clone();
        Some(settings)
    }
}
