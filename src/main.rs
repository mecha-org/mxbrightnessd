use std::path::Path;
use std::process::ExitCode;
use std::time::Duration;

use app::prelude::*;
use dbus::prelude::*;
use mxbrightnessd::core::{CoreModule, Timing};
use mxbrightnessd::devices::{Backlight, LightSensor};
use mxbrightnessd::sensor::SensorModule;
use mxbrightnessd::service::{ExitStatus, ServiceModule};
use mxbrightnessd::settings::{self, SettingsModule};
use ring::prelude::*;

/// How long after the last change the user's levels are written.
const SAVE_DELAY: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    let settings::Loaded {
        source,
        settings,
        manual_saved,
    } = settings::load();
    let root = Path::new("/");
    let backlight = match Backlight::find(root, settings.backlight.as_deref()) {
        Ok(backlight) => backlight,
        Err(e) => {
            eprintln!("mxbrightnessd: no backlight: {e}");
            return ExitCode::FAILURE;
        }
    };
    let sensor = LightSensor::find(root, settings.sensor.as_deref());
    if sensor.is_none() {
        eprintln!("mxbrightnessd: no light sensor; automatic brightness is off");
    }

    let mut app = App::new();
    app.add_module(RingModule::default())
        .add_module(DBusModule::new().session());
    let sensor_available = sensor.is_some();
    if let Some(sensor) = sensor {
        app.add_module(SensorModule { sensor });
    }
    app.add_module(CoreModule {
        backlight,
        sensor_available,
        settings,
        manual_saved,
        timing: Timing::default(),
    })
    .add_module(ServiceModule)
    .add_module(SettingsModule {
        source,
        save_delay: SAVE_DELAY,
    });
    let status = app.resource::<ExitStatus>().clone();
    app.run();
    if status.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
