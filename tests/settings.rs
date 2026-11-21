use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use app::prelude::*;
use mxbrightnessd::events::{SaveState, Settings, SettingsApplied};
use mxbrightnessd::model::Curve;
use mxbrightnessd::settings::{SettingsModule, load_from};
use mxconfig::host::Roots;
use mxconfig::{Source, Table};
use ring::prelude::*;
use tempfile::TempDir;

const DELAY: Duration = Duration::from_millis(50);

/// A source over its own roots in a tempdir: no env, so tests run in parallel.
fn source(dir: &TempDir) -> Source {
    Source::with_roots(Roots::under(dir.path()))
}

fn user_file(dir: &TempDir) -> PathBuf {
    dir.path().join("user/mxbrightnessd/config.toml")
}

fn write_user(dir: &TempDir, text: &str) {
    let path = user_file(dir);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

fn read_user(dir: &TempDir) -> Table {
    fs::read_to_string(user_file(dir)).unwrap().parse().unwrap()
}

fn float(table: &Table, group: &str, key: &str) -> f64 {
    table[group][key].as_float().unwrap()
}

#[derive(Default)]
struct Seen {
    applied: Vec<Settings>,
}
impl Resource for Seen {}

fn note_applied(app: &mut App, e: &SettingsApplied) {
    app.resource_mut::<Seen>().applied.push(e.0.clone());
}

fn app(dir: &TempDir) -> App {
    let source = load_from(source(dir)).source;
    let mut app = App::new();
    app.add_module(RingModule::default())
        .add_module(SettingsModule {
            source,
            save_delay: DELAY,
        })
        .init_resource::<Seen>()
        .system(note_applied);
    app
}

fn run_for(app: &mut App, limit: Duration) {
    let timer = app.resource_mut::<Ring>().timeout(limit);
    loop {
        Ring::turn(app);
        if app.resource_mut::<Ring>().finish(timer).is_some() {
            return;
        }
    }
}

/// Turn the ring until `done` holds, or panic after `limit`.
fn run_until(app: &mut App, limit: Duration, done: impl Fn(&App) -> bool) {
    let end = Instant::now() + limit;
    while !done(app) {
        assert!(Instant::now() < end, "timed out");
        run_for(app, Duration::from_millis(5));
    }
}

fn send(app: &mut App, signal: impl Signal) {
    app.signal(signal);
    app.flush();
}

fn state(manual: f64, offset: f64) -> SaveState {
    SaveState {
        auto: false,
        offset,
        manual,
    }
}

fn applied(app: &App) -> &[Settings] {
    &app.resource::<Seen>().applied
}

fn inode(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|m| m.ino())
}

#[test]
fn defaults_load() {
    let dir = TempDir::new().unwrap();
    let settings = load_from(source(&dir)).settings;
    assert_eq!(settings, Settings::default());
}

#[test]
fn user_file_overrides() {
    let dir = TempDir::new().unwrap();
    write_user(&dir, "[screen]\nminimum = 0.1\n");
    let settings = load_from(source(&dir)).settings;
    assert_eq!(settings.minimum, 0.1);
    assert_eq!(settings.gamma, 2.2);
}

#[test]
fn every_key_is_read() {
    let dir = TempDir::new().unwrap();
    write_user(
        &dir,
        "[screen]\nauto = false\noffset = -0.2\nmanual = 0.7\nminimum = 0.02\ngamma = 2.5\n\
         [curve]\nlux = [1.0, 100.0]\nlevel = [0.2, 0.9]\n\
         [device]\nbacklight = \"panel\"\nsensor = \"opt3001\"\n",
    );
    let settings = load_from(source(&dir)).settings;
    let expected = Settings {
        auto: false,
        offset: -0.2,
        manual: 0.7,
        minimum: 0.02,
        gamma: 2.5,
        curve: Curve::new(vec![(1.0, 0.2), (100.0, 0.9)]).unwrap(),
        backlight: Some("panel".into()),
        sensor: Some("opt3001".into()),
    };
    assert_eq!(settings, expected);
}

#[test]
fn bad_curve_falls_back() {
    let dir = TempDir::new().unwrap();
    write_user(&dir, "[curve]\nlux = [0.1, 1.0, 10.0]\n");
    let settings = load_from(source(&dir)).settings;
    assert_eq!(settings.curve, Curve::default());
}

#[test]
fn unsorted_curve_falls_back() {
    let dir = TempDir::new().unwrap();
    write_user(&dir, "[curve]\nlux = [10.0, 1.0]\nlevel = [0.5, 0.6]\n");
    let settings = load_from(source(&dir)).settings;
    assert_eq!(settings.curve, Curve::default());
}

#[test]
fn non_finite_falls_back() {
    let dir = TempDir::new().unwrap();
    write_user(&dir, "[screen]\ngamma = nan\noffset = inf\nminimum = 0.1\n");
    let settings = load_from(source(&dir)).settings;
    assert_eq!(settings.gamma, 2.2);
    assert_eq!(settings.offset, 0.0);
    assert_eq!(settings.minimum, 0.1);
}

#[test]
fn drag_saves_once() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    let path = user_file(&dir);
    for i in 0..20 {
        send(&mut app, state(0.3 + 0.01 * i as f64, -0.1));
        run_for(&mut app, Duration::from_millis(5));
        assert!(!path.exists(), "written during the drag, at step {i}");
    }
    // Every write replaces the file, so each one is a new inode.
    let mut inodes = Vec::new();
    let end = Instant::now() + Duration::from_millis(200);
    while Instant::now() < end {
        run_for(&mut app, Duration::from_millis(2));
        if let Some(ino) = inode(&path)
            && inodes.last() != Some(&ino)
        {
            inodes.push(ino);
        }
    }
    assert_eq!(inodes.len(), 1, "written once");
    let file = read_user(&dir);
    assert_eq!(float(&file, "screen", "manual"), 0.3 + 0.01 * 19.0);
    assert_eq!(float(&file, "screen", "offset"), -0.1);
    assert_eq!(file["screen"]["auto"].as_bool(), Some(false));
    assert!(
        applied(&app).is_empty(),
        "our own write is not applied back"
    );
    assert_eq!(load_from(source(&dir)).settings.manual, 0.3 + 0.01 * 19.0);
}

#[test]
fn drag_after_save() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    send(&mut app, state(0.4, 0.0));
    run_until(&mut app, Duration::from_secs(1), |_| {
        user_file(&dir).exists()
    });
    send(&mut app, state(0.8, 0.0));
    run_for(&mut app, Duration::from_millis(10));
    assert_eq!(float(&read_user(&dir), "screen", "manual"), 0.4);
    run_for(&mut app, Duration::from_millis(200));
    assert_eq!(float(&read_user(&dir), "screen", "manual"), 0.8);
    assert!(applied(&app).is_empty(), "the echo of 0.4 is not applied");
}

#[test]
fn file_edit_applies_live() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    run_for(&mut app, Duration::from_millis(5));
    write_user(&dir, "[screen]\nminimum = 0.2\n");
    run_until(&mut app, Duration::from_secs(2), |app| {
        !applied(app).is_empty()
    });
    let got = applied(&app).last().unwrap().clone();
    assert_eq!(
        got,
        Settings {
            minimum: 0.2,
            ..Settings::default()
        }
    );
}

#[test]
fn file_edit_while_saving_keeps_the_pending_state() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    send(&mut app, state(0.6, 0.1));
    write_user(&dir, "[screen]\nminimum = 0.2\nmanual = 0.9\n");
    run_until(&mut app, Duration::from_secs(2), |app| {
        !applied(app).is_empty()
    });
    let got = applied(&app)[0].clone();
    assert_eq!(got.minimum, 0.2);
    assert_eq!((got.auto, got.offset, got.manual), (false, 0.1, 0.6));
    run_for(&mut app, Duration::from_millis(200));
    let file = read_user(&dir);
    assert_eq!(float(&file, "screen", "manual"), 0.6);
    assert_eq!(float(&file, "screen", "minimum"), 0.2);
    assert_eq!(applied(&app).len(), 1, "the save's echo is not applied");
}

#[test]
fn manual_saved_only_when_set() {
    let dir = TempDir::new().unwrap();
    assert!(!load_from(source(&dir)).manual_saved);
    write_user(&dir, "[screen]\nminimum = 0.1\n");
    assert!(!load_from(source(&dir)).manual_saved);
    write_user(&dir, "[screen]\nmanual = 0.5\n");
    assert!(load_from(source(&dir)).manual_saved);
}

#[test]
fn seeded_manual_survives_an_unrelated_edit() {
    let dir = TempDir::new().unwrap();
    let mut app = app(&dir);
    // What the core sends at start when it seeds manual from the backlight.
    send(
        &mut app,
        SaveState {
            auto: true,
            offset: 0.0,
            manual: 0.83,
        },
    );
    run_until(&mut app, Duration::from_secs(1), |_| {
        user_file(&dir).exists()
    });
    let mut file = read_user(&dir);
    file["screen"]
        .as_table_mut()
        .unwrap()
        .insert("minimum".into(), 0.2.into());
    fs::write(user_file(&dir), file.to_string()).unwrap();
    run_until(&mut app, Duration::from_secs(2), |app| {
        !applied(app).is_empty()
    });
    let got = applied(&app).last().unwrap().clone();
    assert_eq!(got.minimum, 0.2);
    assert_eq!(got.manual, 0.83, "{got:?}");
}
