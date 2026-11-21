#[path = "fake_sys.rs"]
mod fake_sys;

use std::time::Duration;

use app::prelude::*;
use fake_sys::FakeSys;
use mxbrightnessd::devices::LightSensor;
use mxbrightnessd::events::{LuxRead, ReadLux, SetRange};
use mxbrightnessd::sensor::SensorModule;
use mxbrightnessd::smooth::Range;
use ring::prelude::*;

/// How long to wait for a reading before failing.
const DEADLINE: Duration = Duration::from_secs(2);
/// How long to watch for a reading that must not come.
const QUIET: Duration = Duration::from_millis(100);

#[derive(Default)]
struct Seen(Vec<LuxRead>);
impl Resource for Seen {}

fn note(app: &mut App, e: &LuxRead) {
    app.resource_mut::<Seen>().0.push(e.clone());
}

fn app_with(fake: &FakeSys) -> App {
    let sensor = LightSensor::find(fake.root(), None).expect("sensor");
    let mut app = App::new();
    app.add_module(RingModule::default())
        .add_module(SensorModule { sensor })
        .init_resource::<Seen>()
        .system(note);
    app
}

/// Turns the ring until `want` readings have arrived or `limit` passes.
fn turn_until(app: &mut App, want: usize, limit: Duration) {
    let timer = app.resource_mut::<Ring>().timeout(limit);
    while app.resource::<Seen>().0.len() < want {
        Ring::turn(app);
        if app.resource_mut::<Ring>().finish(timer).is_some() {
            return;
        }
    }
}

fn request(app: &mut App, signal: impl Signal) {
    app.signal(signal);
    app.flush();
}

#[test]
fn reading_arrives_as_signal() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "0.32", true);
    let mut app = app_with(&fake);
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    assert_eq!(app.resource::<Seen>().0, vec![LuxRead(Ok(0.32))]);
}

#[test]
fn error_reading_arrives() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "garbage", true);
    let mut app = app_with(&fake);
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    let seen = &app.resource::<Seen>().0;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.is_err(), "{seen:?}");
}

#[test]
fn invalid_lux_is_an_error_reading() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "-1", true);
    let mut app = app_with(&fake);
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    let seen = &app.resource::<Seen>().0;
    assert_eq!(seen.len(), 1);
    assert!(seen[0].0.is_err(), "{seen:?}");
}

#[test]
fn range_switch_discards_first_and_writes_attribute() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "20", true);
    let mut app = app_with(&fake);
    request(&mut app, SetRange(Range::Fast));
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    turn_until(&mut app, 2, QUIET);
    assert_eq!(fake.integration(0), "0.1");
    assert_eq!(app.resource::<Seen>().0, vec![LuxRead(Ok(20.0))]);
}

#[test]
fn range_ignored_without_integration_attribute() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "5", false);
    let mut app = app_with(&fake);
    request(&mut app, SetRange(Range::Fast));
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    turn_until(&mut app, 2, QUIET);
    assert!(
        !fake
            .root()
            .join("sys/bus/iio/devices/iio:device0/in_illuminance_integration_time")
            .exists()
    );
    assert_eq!(app.resource::<Seen>().0, vec![LuxRead(Ok(5.0))]);
}

#[test]
fn second_read_while_pending_is_ignored() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "3", true);
    let mut app = app_with(&fake);
    app.signal(ReadLux);
    app.signal(ReadLux);
    app.flush();
    turn_until(&mut app, 1, DEADLINE);
    turn_until(&mut app, 2, QUIET);
    assert_eq!(app.resource::<Seen>().0, vec![LuxRead(Ok(3.0))]);
}

#[test]
fn reads_again_after_a_reading() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "1", true);
    let mut app = app_with(&fake);
    request(&mut app, ReadLux);
    turn_until(&mut app, 1, DEADLINE);
    fake.set_lux(0, "2");
    request(&mut app, ReadLux);
    turn_until(&mut app, 2, DEADLINE);
    assert_eq!(
        app.resource::<Seen>().0,
        vec![LuxRead(Ok(1.0)), LuxRead(Ok(2.0))]
    );
}
