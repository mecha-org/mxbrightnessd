#[path = "fake_sys.rs"]
mod fake_sys;

use std::time::Duration;

use app::prelude::*;
use fake_sys::FakeSys;
use mxbrightnessd::core::{CoreModule, Timing};
use mxbrightnessd::devices::{Backlight, LightSensor};
use mxbrightnessd::events::{
    LuxRead, Published, SaveState, Settings, SettingsApplied, UserSetAuto, UserSetBrightness,
};
use mxbrightnessd::model::{from_raw, to_raw};
use mxbrightnessd::sensor::SensorModule;
use ring::prelude::*;

const BL: &str = "panel";
const MAX: u32 = 255;
/// Long enough for any fade in these tests (at most 1 p darkening at
/// 0.10 p/s) plus smoothing.
const SETTLE: Duration = Duration::from_secs(12);

fn raw(p: f64) -> u32 {
    to_raw(p, 2.2, MAX)
}

fn timing() -> Timing {
    Timing {
        poll: Duration::from_millis(20),
        frame: Duration::from_millis(5),
        hold: Duration::from_millis(100),
        rise: Duration::from_millis(10),
        fall: Duration::from_millis(10),
    }
}

#[derive(Default)]
struct Seen {
    published: Vec<Published>,
    saved: Vec<SaveState>,
}
impl Resource for Seen {}

fn note_published(app: &mut App, e: &Published) {
    app.resource_mut::<Seen>().published.push(e.clone());
}

fn note_saved(app: &mut App, e: &SaveState) {
    app.resource_mut::<Seen>().saved.push(*e);
}

/// A fake /sys with backlight `panel` at `cur` and, if `lux` is given, an
/// opt3001 reading it.
fn fake(cur: u32, lux: Option<&str>) -> FakeSys {
    let fake = FakeSys::new();
    fake.backlight(BL, "raw", MAX, cur);
    if let Some(lux) = lux {
        fake.sensor(0, "opt3001", lux, true);
    }
    fake
}

fn app_with(fake: &FakeSys, settings: Settings) -> App {
    app_timed(fake, settings, timing())
}

/// Smoothing slow next to the poll, as in production (1 s poll, 2 s/8 s).
fn slow_timing() -> Timing {
    Timing {
        rise: Duration::from_millis(40),
        fall: Duration::from_millis(160),
        ..timing()
    }
}

fn app_timed(fake: &FakeSys, settings: Settings, timing: Timing) -> App {
    app_full(fake, settings, timing, true)
}

fn app_full(fake: &FakeSys, settings: Settings, timing: Timing, manual_saved: bool) -> App {
    let backlight = Backlight::find(fake.root(), None).expect("backlight");
    let sensor = LightSensor::find(fake.root(), None);
    let sensor_available = sensor.is_some();
    let mut app = App::new();
    app.add_module(RingModule::default());
    if let Some(sensor) = sensor {
        app.add_module(SensorModule { sensor });
    }
    app.add_module(CoreModule {
        backlight,
        sensor_available,
        settings,
        manual_saved,
        timing,
    })
    .init_resource::<Seen>()
    .system(note_published)
    .system(note_saved);
    app
}

/// Turns the ring for `limit` of real time.
fn run_for(app: &mut App, limit: Duration) {
    let timer = app.resource_mut::<Ring>().timeout(limit);
    loop {
        Ring::turn(app);
        if app.resource_mut::<Ring>().finish(timer).is_some() {
            return;
        }
    }
}

/// Turns the ring until `done` holds or [`SETTLE`] passes.
fn settle(app: &mut App, mut done: impl FnMut(&App) -> bool) {
    let timer = app.resource_mut::<Ring>().timeout(SETTLE);
    while !done(app) {
        Ring::turn(app);
        if app.resource_mut::<Ring>().finish(timer).is_some() {
            return;
        }
    }
    let cancel = app.resource_mut::<Ring>().cancel(timer);
    let mut pending = vec![timer, cancel];
    while !pending.is_empty() {
        Ring::turn(app);
        let mut ring = app.resource_mut::<Ring>();
        pending.retain(|&t| ring.finish(t).is_none());
    }
}

/// Settles until backlight `BL` of `fake` reads `want`.
fn settle_at(app: &mut App, fake: &FakeSys, want: u32) {
    settle(app, |_| fake.raw(BL) == want);
}

fn send(app: &mut App, signal: impl Signal) {
    app.signal(signal);
    app.flush();
}

fn last_published(app: &App) -> Published {
    app.resource::<Seen>()
        .published
        .last()
        .cloned()
        .expect("a Published")
}

fn near(a: u32, b: u32, tol: u32) -> bool {
    a.abs_diff(b) <= tol
}

#[test]
fn starts_from_current_backlight() {
    let fake = fake(178, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    let timer = app.resource_mut::<Ring>().timeout(SETTLE);
    let mut first = None;
    while first.is_none() {
        Ring::turn(&mut app);
        let now = fake.raw(BL);
        if now != 178 {
            first = Some(now);
        }
        assert!(
            app.resource_mut::<Ring>().finish(timer).is_none(),
            "backlight never moved"
        );
    }
    let first = first.unwrap();
    assert!(near(first, 178, 3), "first write {first} jumped from 178");
    settle_at(&mut app, &fake, raw(0.30));
    assert_eq!(fake.raw(BL), raw(0.30));
}

#[test]
fn auto_follows_light() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    assert_eq!(fake.raw(BL), raw(0.30));
    let p = last_published(&app);
    assert!(p.auto && p.auto_available);
    assert!((p.ambient_lux.unwrap() - 0.3).abs() < 1e-9, "{p:?}");
    fake.set_lux(0, "300");
    settle_at(&mut app, &fake, 255);
    assert_eq!(fake.raw(BL), 255);
}

#[test]
fn user_set_shifts_offset() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    send(&mut app, UserSetBrightness(0.40));
    run_for(&mut app, Duration::from_millis(500));
    assert_eq!(fake.raw(BL), raw(0.40));
    let saved = app.resource::<Seen>().saved.clone();
    assert_eq!(saved.len(), 1);
    assert!(saved[0].auto && (saved[0].offset - 0.10).abs() < 1e-9);
    fake.set_lux(0, "3");
    settle(&mut app, |_| near(fake.raw(BL), raw(0.60), 1));
    assert!(near(fake.raw(BL), raw(0.60), 1), "{}", fake.raw(BL));
}

#[test]
fn hold_blocks_auto_after_set() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    send(&mut app, UserSetBrightness(0.40));
    let from = app.resource::<Seen>().published.len();
    assert_eq!(last_published(&app).brightness, 0.40);
    fake.set_lux(0, "300");
    run_for(&mut app, Duration::from_millis(60));
    let during = &app.resource::<Seen>().published[from..];
    assert!(during.iter().all(|p| p.brightness == 0.40), "{during:?}");
    // Once the hold is over the light takes over again.
    settle(&mut app, |app| last_published(app).brightness > 0.9);
    assert!(last_published(&app).brightness > 0.9);
}

#[test]
fn failed_read_holds_level() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    let before = fake.raw(BL);
    let target = last_published(&app).brightness;
    fake.set_lux(0, "garbage");
    run_for(&mut app, Duration::from_millis(250)); // > 10 polls
    assert_eq!(fake.raw(BL), before);
    assert_eq!(last_published(&app).brightness, target);
}

#[test]
fn invalid_readings_and_sets_are_ignored() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    let before = fake.raw(BL);
    let published = last_published(&app);
    send(&mut app, LuxRead(Ok(f64::NAN)));
    send(&mut app, LuxRead(Ok(-5.0)));
    send(&mut app, LuxRead(Ok(f64::INFINITY)));
    send(&mut app, UserSetBrightness(f64::NAN));
    send(&mut app, UserSetBrightness(f64::INFINITY));
    run_for(&mut app, Duration::from_millis(50));
    assert_eq!(fake.raw(BL), before);
    assert_eq!(last_published(&app).brightness, published.brightness);
    assert!(app.resource::<Seen>().saved.is_empty());
}

#[test]
fn manual_ignores_sensor() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    run_for(&mut app, Duration::from_millis(100));
    send(&mut app, UserSetAuto(false));
    send(&mut app, UserSetBrightness(0.5));
    settle_at(&mut app, &fake, raw(0.5));
    assert_eq!(fake.raw(BL), raw(0.5));
    let p = last_published(&app);
    assert!(!p.auto && p.brightness == 0.5, "{p:?}");
    let saved = app.resource::<Seen>().saved.clone();
    assert_eq!(saved.len(), 2);
    assert!(!saved[1].auto && saved[1].manual == 0.5);
    fake.set_lux(0, "300");
    run_for(&mut app, Duration::from_millis(300));
    assert_eq!(fake.raw(BL), raw(0.5));
}

#[test]
fn no_sensor_is_manual_and_unavailable() {
    let fake = fake(128, None);
    let settings = Settings {
        manual: 0.7,
        ..Settings::default()
    };
    let mut app = app_with(&fake, settings);
    settle_at(&mut app, &fake, raw(0.7));
    let p = last_published(&app);
    assert!(!p.auto_available, "{p:?}");
    assert_eq!(p.brightness, 0.7);
    assert_eq!(p.ambient_lux, None);
    assert_eq!(fake.raw(BL), raw(0.7));
}

#[test]
fn drag_retargets_fade() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    for i in 1..=20 {
        send(&mut app, UserSetBrightness(0.40 + 0.02 * i as f64));
        run_for(&mut app, Duration::from_millis(5));
    }
    assert!((last_published(&app).brightness - 0.80).abs() < 1e-9);
    run_for(&mut app, Duration::from_millis(500));
    assert_eq!(fake.raw(BL), raw(0.80));
    assert_eq!(app.resource::<Seen>().saved.len(), 20);
}

#[test]
fn settings_apply_and_bad_values_are_clamped() {
    let fake = fake(128, None);
    let mut app = app_with(&fake, Settings::default());
    run_for(&mut app, Duration::from_millis(100));
    let bad = Settings {
        manual: 0.6,
        minimum: f64::NAN,
        gamma: f64::NAN,
        ..Settings::default()
    };
    send(&mut app, SettingsApplied(bad));
    settle_at(&mut app, &fake, raw(0.6));
    let p = last_published(&app);
    assert_eq!((p.brightness, p.minimum), (0.6, 0.05));
    assert_eq!(fake.raw(BL), raw(0.6));
    send(
        &mut app,
        SettingsApplied(Settings {
            manual: 0.0,
            minimum: 7.0,
            ..Settings::default()
        }),
    );
    settle_at(&mut app, &fake, 255);
    let p = last_published(&app);
    assert_eq!((p.brightness, p.minimum), (1.0, 1.0));
    assert_eq!(fake.raw(BL), 255);
    assert!(app.resource::<Seen>().saved.is_empty());
}

#[test]
fn auto_start_holds_until_first_reading() {
    let fake = fake(178, Some("garbage"));
    let mut app = app_with(&fake, Settings::default());
    run_for(&mut app, Duration::from_millis(300));
    assert_eq!(fake.raw(BL), 178);
    let start = from_raw(178, 2.2, MAX);
    assert!((last_published(&app).brightness - start).abs() < 1e-9);
    fake.set_lux(0, "0.3");
    settle_at(&mut app, &fake, raw(0.30));
    assert_eq!(fake.raw(BL), raw(0.30));
}

#[test]
fn slow_smoothing_darkening_settles_on_curve() {
    let fake = fake(128, Some("300"));
    let mut app = app_timed(&fake, Settings::default(), slow_timing());
    settle_at(&mut app, &fake, 255);
    assert_eq!(fake.raw(BL), 255);
    fake.set_lux(0, "3");
    settle(&mut app, |_| near(fake.raw(BL), raw(0.50), 1));
    assert!(near(fake.raw(BL), raw(0.50), 1), "{}", fake.raw(BL));
}

#[test]
fn slow_smoothing_brightening_settles_on_curve() {
    let fake = fake(128, Some("3"));
    let mut app = app_timed(&fake, Settings::default(), slow_timing());
    settle_at(&mut app, &fake, raw(0.50));
    assert_eq!(fake.raw(BL), raw(0.50));
    fake.set_lux(0, "30");
    settle(&mut app, |_| near(fake.raw(BL), raw(0.75), 1));
    assert!(near(fake.raw(BL), raw(0.75), 1), "{}", fake.raw(BL));
}

#[test]
fn steady_noisy_light_stops_moving() {
    let fake = fake(128, Some("100"));
    let mut app = app_timed(&fake, Settings::default(), slow_timing());
    let flip = |app: &mut App, ms: u64| {
        let end = std::time::Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        let mut high = false;
        while std::time::Instant::now() < end {
            high = !high;
            fake.set_lux(0, if high { "110" } else { "100" });
            run_for(app, Duration::from_millis(30));
            out.push((fake.raw(BL), last_published(app).brightness));
        }
        out
    };
    flip(&mut app, 1500);
    let after = flip(&mut app, 1000);
    assert!(after.windows(2).all(|w| w[0] == w[1]), "{after:?}");
}

#[test]
fn unsaved_manual_keeps_the_current_level() {
    let fake = fake(178, None);
    let mut app = app_full(&fake, Settings::default(), timing(), false);
    run_for(&mut app, Duration::from_secs(2));
    assert_eq!(fake.raw(BL), 178);
    let start = from_raw(178, 2.2, MAX);
    assert!((last_published(&app).brightness - start).abs() < 1e-9);
    let saved = app.resource::<Seen>().saved.clone();
    assert_eq!(saved.len(), 1, "{saved:?}");
    assert!((saved[0].manual - start).abs() < 1e-9, "{saved:?}");
    assert!(saved[0].auto && saved[0].offset == 0.0, "{saved:?}");
}

#[test]
fn auto_back_on_waits_for_a_fresh_reading() {
    let fake = fake(128, Some("0.3"));
    let mut app = app_with(&fake, Settings::default());
    settle_at(&mut app, &fake, raw(0.30));
    assert_eq!(fake.raw(BL), raw(0.30));
    send(&mut app, UserSetAuto(false));
    settle_at(&mut app, &fake, raw(0.5));
    assert_eq!(fake.raw(BL), raw(0.5));
    assert_eq!(last_published(&app).ambient_lux, None);
    fake.set_lux(0, "garbage");
    send(&mut app, UserSetAuto(true));
    run_for(&mut app, Duration::from_millis(300));
    // The old 0.3 lux would say 0.30; without a reading it holds.
    assert_eq!(fake.raw(BL), raw(0.5));
    let p = last_published(&app);
    assert_eq!((p.brightness, p.ambient_lux), (0.5, None), "{p:?}");
    fake.set_lux(0, "30");
    settle(&mut app, |_| near(fake.raw(BL), raw(0.75), 1));
    assert!(near(fake.raw(BL), raw(0.75), 1), "{}", fake.raw(BL));
}

#[test]
fn ambient_lux_publishes_only_real_changes() {
    let fake = fake(128, Some("100"));
    let mut app = app_with(&fake, Settings::default());
    settle(&mut app, |app| {
        let p = app.resource::<Seen>().published.last().cloned();
        p.is_some_and(|p| {
            p.ambient_lux.is_some_and(|l| (l - 100.0).abs() < 1e-6)
                && fake.raw(BL) == raw(p.brightness)
        })
    });
    let shown = last_published(&app).ambient_lux.unwrap();
    assert!((shown - 100.0).abs() < 1e-6, "{shown}");
    let from = app.resource::<Seen>().published.len();
    // 3 % more light moves neither the level nor the published lux.
    fake.set_lux(0, "103");
    run_for(&mut app, Duration::from_millis(300));
    let during = &app.resource::<Seen>().published[from..];
    assert!(during.is_empty(), "{during:?}");
    fake.set_lux(0, "110");
    run_for(&mut app, Duration::from_millis(300));
    let shown = last_published(&app).ambient_lux.unwrap();
    assert!((105.0..=110.0).contains(&shown), "{shown}");
}

#[test]
fn slider_drag_moves_smoothly() {
    let start = 0.30;
    let fake = fake(raw(start), None);
    let settings = Settings {
        manual: start,
        ..Settings::default()
    };
    // Frames as in production.
    let timing = Timing {
        frame: Duration::from_millis(16),
        ..timing()
    };
    let mut app = app_timed(&fake, settings, timing);
    run_for(&mut app, Duration::from_millis(100));
    assert_eq!(fake.raw(BL), raw(start));
    // The launcher sends slider values about every 50 ms; the last one
    // gets 400 ms to land.
    let mut seen = vec![fake.raw(BL)];
    for i in 1..=6 {
        send(&mut app, UserSetBrightness(start + 0.05 * i as f64));
        let wait = if i == 6 { 400 } else { 50 };
        let timer = app
            .resource_mut::<Ring>()
            .timeout(Duration::from_millis(wait));
        while app.resource_mut::<Ring>().finish(timer).is_none() {
            Ring::turn(&mut app);
            seen.push(fake.raw(BL));
        }
    }
    let jumps: Vec<u32> = seen.windows(2).map(|w| w[0].abs_diff(w[1])).collect();
    assert!(jumps.iter().all(|&j| j <= 12), "{seen:?}");
    assert_eq!(fake.raw(BL), raw(0.60));
}
