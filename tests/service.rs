use std::collections::BTreeMap;

use app::prelude::*;
use dbus::fake::{FakeBus, UNIQUE};
use dbus::*;
use mxbrightnessd::events::{Published, UserSetAuto, UserSetBrightness};
use mxbrightnessd::service::{ExitStatus, INTERFACE, NAME, PATH, ServiceModule};
use ring::prelude::*;

const PROPS: &str = "org.freedesktop.DBus.Properties";
const DAEMON: &str = "org.freedesktop.DBus";

#[derive(Default)]
struct Seen {
    brightness: Vec<f64>,
    auto: Vec<bool>,
}
impl Resource for Seen {}

fn note_brightness(app: &mut App, e: &UserSetBrightness) {
    app.resource_mut::<Seen>().brightness.push(e.0);
}

fn note_auto(app: &mut App, e: &UserSetAuto) {
    app.resource_mut::<Seen>().auto.push(e.0);
}

fn install(f: &mut FakeBus) {
    f.app
        .add_module(ServiceModule)
        .init_resource::<Seen>()
        .system(note_brightness)
        .system(note_auto);
}

/// A fake bus with the service installed and its name granted.
fn fake() -> FakeBus {
    let mut f = FakeBus::new();
    install(&mut f);
    f.turn();
    f.received();
    f
}

fn stopped(f: &FakeBus) -> bool {
    f.app.resource::<Ring>().is_stopped()
}

fn failed(f: &FakeBus) -> bool {
    f.app.resource::<ExitStatus>().failed()
}

/// Send a call to the service and return its answer.
fn ask<B: Body>(f: &mut FakeBus, member: &str, body: &B) -> Message {
    let serial = f.send(
        Message::method_call(PATH, MemberName::new(member).unwrap())
            .with_destination(UNIQUE)
            .with_sender(":1.9")
            .with_interface(PROPS)
            .with_body(body),
    );
    f.turn();
    f.received()
        .into_iter()
        .find(|m| m.reply_serial() == Some(serial))
        .expect("an answer")
}

fn set(f: &mut FakeBus, name: &str, value: Value) -> Message {
    ask(f, "Set", &(INTERFACE, name, value))
}

fn error_name(m: &Message) -> &str {
    m.error_name().map(|e| e.as_str()).unwrap_or("none")
}

fn published(brightness: f64) -> Published {
    Published {
        brightness,
        auto: true,
        minimum: 0.05,
        ambient_lux: None,
        auto_available: false,
    }
}

/// Every property in the `PropertiesChanged` signals sent since the last
/// look, and how many signals there were.
fn changes(f: &mut FakeBus) -> (usize, BTreeMap<String, Value>) {
    let signals: Vec<Message> = f
        .received()
        .into_iter()
        .filter(|m| {
            m.member()
                .is_some_and(|x| x.as_str() == "PropertiesChanged")
        })
        .collect();
    let mut all = BTreeMap::new();
    for s in &signals {
        let (iface, props, _) = PropertiesChanged::parse(s).unwrap();
        assert_eq!(iface.as_str(), INTERFACE);
        all.extend(props);
    }
    (signals.len(), all)
}

#[test]
fn requests_the_name_without_queueing() {
    let mut f = FakeBus::new();
    f.auto_daemon = false;
    install(&mut f);
    f.turn();
    let call = f.expect_call("RequestName");
    let (name, flags): (String, u32) = call.body().unwrap();
    assert_eq!(name, NAME);
    assert_eq!(flags, NameFlags::DO_NOT_QUEUE.0);
    f.reply(&call, &(1u32,));
    f.turn();
    assert!(!stopped(&f));
    assert!(!failed(&f));
}

#[test]
fn name_taken_stops() {
    let mut f = FakeBus::new();
    f.auto_daemon = false;
    install(&mut f);
    f.turn();
    let call = f.expect_call("RequestName");
    f.reply(&call, &(3u32,)); // EXISTS
    f.turn();
    assert!(stopped(&f));
    assert!(failed(&f));
}

#[test]
fn name_request_error_stops() {
    let mut f = FakeBus::new();
    f.auto_daemon = false;
    install(&mut f);
    f.turn();
    let call = f.expect_call("RequestName");
    f.error(&call, "org.freedesktop.DBus.Error.AccessDenied", "no");
    f.turn();
    assert!(stopped(&f));
    assert!(failed(&f));
}

#[test]
fn name_lost_stops() {
    let mut f = fake();
    assert!(!stopped(&f));
    f.send(
        Message::signal("/org/freedesktop/DBus", DAEMON, "NameLost")
            .with_destination(UNIQUE)
            .with_sender(DAEMON)
            .with_body(&(NAME,)),
    );
    f.turn();
    assert!(stopped(&f));
    assert!(failed(&f));
}

#[test]
fn bus_disconnect_stops() {
    let f = fake();
    let mut app = f.hang_up();
    Ring::turn(&mut app);
    assert!(app.resource::<Ring>().is_stopped());
    assert!(app.resource::<ExitStatus>().failed());
}

#[test]
fn exports_properties() {
    let mut f = fake();
    let all = ask(&mut f, "GetAll", &(INTERFACE,));
    let (props,): (PropMap,) = all.body().unwrap();
    let sigs: BTreeMap<String, String> = props
        .iter()
        .map(|(k, v)| (k.clone(), v.signature().as_str().to_owned()))
        .collect();
    let want: BTreeMap<String, String> = [
        ("AmbientLux", "d"),
        ("Auto", "b"),
        ("AutoAvailable", "b"),
        ("Brightness", "d"),
        ("Minimum", "d"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    assert_eq!(sigs, want);
    assert_eq!(props.get_as::<f64>("Brightness"), Some(Ok(0.5)));
    assert_eq!(props.get_as::<bool>("Auto"), Some(Ok(true)));
    assert_eq!(props.get_as::<f64>("Minimum"), Some(Ok(0.05)));
    assert_eq!(props.get_as::<f64>("AmbientLux"), Some(Ok(0.0)));
    assert_eq!(props.get_as::<bool>("AutoAvailable"), Some(Ok(false)));
}

#[test]
fn set_brightness_emits_user_set() {
    let mut f = fake();
    let answer = set(&mut f, "Brightness", Value::F64(0.7));
    assert_eq!(answer.kind(), MessageKind::MethodReturn);
    assert_eq!(f.app.resource::<Seen>().brightness, vec![0.7]);
}

#[test]
fn set_brightness_out_of_range_is_passed_on() {
    let mut f = fake();
    for v in [5.0, -1.0] {
        let answer = set(&mut f, "Brightness", Value::F64(v));
        assert_eq!(answer.kind(), MessageKind::MethodReturn);
    }
    assert_eq!(f.app.resource::<Seen>().brightness, vec![5.0, -1.0]);
}

#[test]
fn set_brightness_nan_is_invalid_args() {
    let mut f = fake();
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let answer = set(&mut f, "Brightness", Value::F64(v));
        assert_eq!(
            error_name(&answer),
            "org.freedesktop.DBus.Error.InvalidArgs"
        );
    }
    assert!(f.app.resource::<Seen>().brightness.is_empty());
}

#[test]
fn set_brightness_wrong_type_is_invalid_args() {
    let mut f = fake();
    let answer = set(&mut f, "Brightness", Value::U32(5));
    assert_eq!(
        error_name(&answer),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    assert!(f.app.resource::<Seen>().brightness.is_empty());
}

#[test]
fn set_auto_emits() {
    let mut f = fake();
    let answer = set(&mut f, "Auto", Value::Bool(false));
    assert_eq!(answer.kind(), MessageKind::MethodReturn);
    let answer = set(&mut f, "Auto", Value::Bool(true));
    assert_eq!(answer.kind(), MessageKind::MethodReturn);
    assert_eq!(f.app.resource::<Seen>().auto, vec![false, true]);
}

#[test]
fn set_auto_wrong_type_is_invalid_args() {
    let mut f = fake();
    let answer = set(&mut f, "Auto", Value::U32(1));
    assert_eq!(
        error_name(&answer),
        "org.freedesktop.DBus.Error.InvalidArgs"
    );
    assert!(f.app.resource::<Seen>().auto.is_empty());
}

#[test]
fn minimum_is_read_only() {
    let mut f = fake();
    for name in ["Minimum", "AmbientLux"] {
        let answer = set(&mut f, name, Value::F64(0.2));
        assert_eq!(
            error_name(&answer),
            "org.freedesktop.DBus.Error.PropertyReadOnly"
        );
    }
    let answer = set(&mut f, "AutoAvailable", Value::Bool(true));
    assert_eq!(
        error_name(&answer),
        "org.freedesktop.DBus.Error.PropertyReadOnly"
    );
}

#[test]
fn set_does_not_change_the_property_until_published() {
    let mut f = fake();
    set(&mut f, "Brightness", Value::F64(0.7));
    let get = ask(&mut f, "Get", &(INTERFACE, "Brightness"));
    assert_eq!(get.body::<(Value,)>().unwrap().0, Value::F64(0.5));
}

#[test]
fn published_updates_properties() {
    let mut f = fake();
    f.app.signal(published(0.3));
    f.turn();
    let (n, props) = changes(&mut f);
    assert_eq!(n, 1);
    assert_eq!(
        props,
        BTreeMap::from([("Brightness".to_owned(), Value::F64(0.3))])
    );
}

#[test]
fn published_sends_only_what_changed() {
    let mut f = fake();
    f.app.signal(Published {
        brightness: 0.4,
        auto: false,
        minimum: 0.1,
        ambient_lux: Some(12.5),
        auto_available: true,
    });
    f.turn();
    let (_, props) = changes(&mut f);
    assert_eq!(
        props,
        BTreeMap::from([
            ("AmbientLux".to_owned(), Value::F64(12.5)),
            ("Auto".to_owned(), Value::Bool(false)),
            ("AutoAvailable".to_owned(), Value::Bool(true)),
            ("Brightness".to_owned(), Value::F64(0.4)),
            ("Minimum".to_owned(), Value::F64(0.1)),
        ])
    );
    f.app.signal(Published {
        brightness: 0.4,
        auto: false,
        minimum: 0.1,
        ambient_lux: Some(13.0),
        auto_available: true,
    });
    f.turn();
    let (_, props) = changes(&mut f);
    assert_eq!(
        props,
        BTreeMap::from([("AmbientLux".to_owned(), Value::F64(13.0))])
    );
    f.app.signal(Published {
        brightness: 0.4,
        auto: false,
        minimum: 0.1,
        ambient_lux: Some(13.0),
        auto_available: true,
    });
    f.turn();
    assert_eq!(changes(&mut f).0, 0, "nothing changed, nothing sent");
}

#[test]
fn ambient_lux_none_is_zero() {
    let mut f = fake();
    f.app.signal(Published {
        ambient_lux: Some(40.0),
        ..published(0.5)
    });
    f.turn();
    f.received();
    f.app.signal(published(0.5));
    f.turn();
    let (_, props) = changes(&mut f);
    assert_eq!(
        props,
        BTreeMap::from([("AmbientLux".to_owned(), Value::F64(0.0))])
    );
}
