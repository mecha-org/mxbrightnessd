//! Signals shared between modules.

use app::prelude::*;

use crate::model::Curve;
use crate::smooth::Range;

/// Ask the sensor for one reading. Ignored while a reading is pending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadLux;
impl Signal for ReadLux {}

/// Switch the sensor's integration time. Ignored if the sensor has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetRange(pub Range);
impl Signal for SetRange {}

/// One sensor reading in lux, or the error text.
#[derive(Debug, Clone, PartialEq)]
pub struct LuxRead(pub Result<f64, String>);
impl Signal for LuxRead {}

/// The user sets the brightness (perceived, clamped into `[minimum, 1]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UserSetBrightness(pub f64);
impl Signal for UserSetBrightness {}

/// The user turns automatic brightness on or off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserSetAuto(pub bool);
impl Signal for UserSetAuto {}

/// New settings, from the config file.
#[derive(Debug, Clone, PartialEq)]
pub struct SettingsApplied(pub Settings);
impl Signal for SettingsApplied {}

/// What the daemon shows: sent whenever any field changes.
#[derive(Debug, Clone, PartialEq)]
pub struct Published {
    /// The level being faded to (perceived).
    pub brightness: f64,
    pub auto: bool,
    pub minimum: f64,
    /// Smoothed sensor lux, `None` until a reading arrives or without a sensor.
    pub ambient_lux: Option<f64>,
    pub auto_available: bool,
}
impl Signal for Published {}

/// The user's levels, to persist: sent on every user change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaveState {
    pub auto: bool,
    pub offset: f64,
    pub manual: f64,
}
impl Signal for SaveState {}

/// Daemon settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub auto: bool,
    pub offset: f64,
    pub manual: f64,
    pub minimum: f64,
    pub gamma: f64,
    pub curve: Curve,
    /// Backlight name under `/sys/class/backlight`, else picked by type.
    pub backlight: Option<String>,
    /// IIO device name of the light sensor, else the first with lux.
    pub sensor: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            auto: true,
            offset: 0.0,
            manual: 0.5,
            minimum: 0.05,
            gamma: 2.2,
            curve: Curve::default(),
            backlight: None,
            sensor: None,
        }
    }
}
