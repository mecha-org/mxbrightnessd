//! The `org.mechanix.Brightness` service on the session bus.
//!
//! Properties mirror [`Published`]; a peer's `Set` on `Brightness` or
//! `Auto` becomes [`UserSetBrightness`] or [`UserSetAuto`] and is not
//! stored here: the property changes when the core publishes. Without
//! the name the daemon has no purpose, so failing to own it, losing it,
//! or losing the session bus stops the ring and marks [`ExitStatus`]
//! failed.

use std::cell::Cell;
use std::rc::Rc;

use app::prelude::*;
use dbus::prelude::*;
use ring::prelude::*;

use crate::events::{Published, UserSetAuto, UserSetBrightness};

pub const NAME: &str = "org.mechanix.Brightness";
pub const PATH: &str = "/org/mechanix/Brightness";
pub const INTERFACE: &str = "org.mechanix.Brightness";

const INVALID_ARGS: &str = "org.freedesktop.DBus.Error.InvalidArgs";
/// `RequestName`'s reply when the name is now ours.
const PRIMARY_OWNER: u32 = 1;

/// Exports the interface and owns the name. Install after `DBusModule`
/// with the session bus.
pub struct ServiceModule;

/// Whether the daemon stopped on an error. Clones share one flag, so
/// `main` keeps a clone across `App::run` to pick its exit code.
#[derive(Clone, Debug, Default)]
pub struct ExitStatus(Rc<Cell<bool>>);
impl Resource for ExitStatus {}

impl ExitStatus {
    pub fn fail(&self) {
        self.0.set(true);
    }

    pub fn failed(&self) -> bool {
        self.0.get()
    }
}

struct Service {
    export: ExportId,
    request: CallId,
}
impl Resource for Service {}

impl Module for ServiceModule {
    fn install(self, app: &mut App) {
        let spec = InterfaceSpec::new(INTERFACE)
            .property("Brightness", "d", Access::ReadWrite, Value::F64(0.5))
            .property("Auto", "b", Access::ReadWrite, Value::Bool(true))
            .property("Minimum", "d", Access::Read, Value::F64(0.05))
            .property("AmbientLux", "d", Access::Read, Value::F64(0.0))
            .property("AutoAvailable", "b", Access::Read, Value::Bool(false));
        let service = {
            let mut dbus = app.resource_mut::<DBus>();
            let mut bus = dbus.session();
            let export = bus
                .export(PATH, spec)
                .expect("mxbrightnessd: export the brightness object");
            let request = bus.request_name(NAME, NameFlags::DO_NOT_QUEUE);
            Service { export, request }
        };
        app.insert_resource(service);
        app.init_resource::<ExitStatus>()
            .system(on_name_reply)
            .system(on_name_lost)
            .system(on_disconnected)
            .system(on_set)
            .system(on_published);
    }
}

/// Stop on an error, after logging `why`.
fn give_up(app: &mut App, why: &str) {
    eprintln!("mxbrightnessd: {why}");
    app.resource::<ExitStatus>().fail();
    app.signal(Stop);
}

fn on_name_reply(app: &mut App, e: &MethodReply) {
    if e.call != app.resource::<Service>().request {
        return;
    }
    let owned = match &e.result {
        Ok(msg) => matches!(msg.body::<(u32,)>(), Ok((PRIMARY_OWNER,))),
        Err(_) => false,
    };
    if !owned {
        give_up(app, &format!("could not own {NAME}"));
    }
}

fn on_name_lost(app: &mut App, e: &NameLost) {
    if e.bus == BusId::Session && e.name.as_str() == NAME {
        give_up(app, &format!("lost {NAME}"));
    }
}

fn on_disconnected(app: &mut App, e: &BusDisconnected) {
    if e.bus == BusId::Session {
        give_up(app, "session bus disconnected");
    }
}

/// What an accepted `Set` asks of the core.
enum UserSet {
    Brightness(f64),
    Auto(bool),
}

fn on_set(app: &mut App, e: &PropertySetRequested) {
    if e.export != app.resource::<Service>().export {
        return;
    }
    let set = match (e.name.as_str(), &e.value) {
        ("Brightness", Value::F64(v)) if v.is_finite() => UserSet::Brightness(*v),
        ("Auto", Value::Bool(v)) => UserSet::Auto(*v),
        _ => {
            let text = format!("{} cannot be {:?}", e.name, e.value);
            app.resource_mut::<DBus>()
                .reply_error(&e.call, INVALID_ARGS, &text);
            return;
        }
    };
    app.resource_mut::<DBus>().reply(&e.call, &());
    match set {
        UserSet::Brightness(v) => app.signal(UserSetBrightness(v)),
        UserSet::Auto(v) => app.signal(UserSetAuto(v)),
    }
}

fn on_published(app: &mut App, e: &Published) {
    let export = app.resource::<Service>().export;
    let values = [
        ("Brightness", Value::F64(e.brightness)),
        ("Auto", Value::Bool(e.auto)),
        ("Minimum", Value::F64(e.minimum)),
        ("AmbientLux", Value::F64(e.ambient_lux.unwrap_or(0.0))),
        ("AutoAvailable", Value::Bool(e.auto_available)),
    ];
    let mut dbus = app.resource_mut::<DBus>();
    let mut bus = dbus.session();
    // The library stores each value and sends nothing for an unchanged one.
    for (name, value) in values {
        bus.set_property_value(export, name, value)
            .expect("mxbrightnessd: a property of the brightness object");
    }
}
