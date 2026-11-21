//! Fake `/sys` tree in a tempdir, laid out like the Comet's backlight and
//! opt3001 light sensor. Included by tests via
//! `#[path = "fake_sys.rs"] mod fake_sys;`.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

pub struct FakeSys {
    dir: TempDir,
}

impl Default for FakeSys {
    fn default() -> FakeSys {
        FakeSys::new()
    }
}

impl FakeSys {
    pub fn new() -> FakeSys {
        FakeSys {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    pub fn root(&self) -> &Path {
        self.dir.path()
    }

    fn backlight_dir(&self, name: &str) -> PathBuf {
        self.root().join("sys/class/backlight").join(name)
    }

    fn iio_dir(&self, n: u32) -> PathBuf {
        self.root()
            .join("sys/bus/iio/devices")
            .join(format!("iio:device{n}"))
    }

    /// Adds a backlight with `type` `ty`, `max_brightness` `max` and
    /// `brightness`/`actual_brightness` `cur`.
    pub fn backlight(&self, name: &str, ty: &str, max: u32, cur: u32) {
        let dir = self.backlight_dir(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("type"), format!("{ty}\n")).unwrap();
        fs::write(dir.join("max_brightness"), format!("{max}\n")).unwrap();
        fs::write(dir.join("brightness"), format!("{cur}\n")).unwrap();
        fs::write(dir.join("actual_brightness"), format!("{cur}\n")).unwrap();
    }

    /// Adds `iio:device{n}` named `name`. An empty `lux` leaves out
    /// `in_illuminance_input` (a non-light IIO device); `integration` adds
    /// `in_illuminance_integration_time` set to `0.800000`.
    pub fn sensor(&self, n: u32, name: &str, lux: &str, integration: bool) {
        let dir = self.iio_dir(n);
        fs::create_dir_all(dir.join("events")).unwrap();
        fs::write(dir.join("name"), format!("{name}\n")).unwrap();
        if !lux.is_empty() {
            self.set_lux(n, lux);
        }
        if integration {
            fs::write(dir.join("in_illuminance_integration_time"), "0.800000\n").unwrap();
        }
    }

    pub fn set_lux(&self, n: u32, lux: &str) {
        fs::write(
            self.iio_dir(n).join("in_illuminance_input"),
            format!("{lux}\n"),
        )
        .unwrap();
    }

    /// Current `brightness` of backlight `name`.
    pub fn raw(&self, name: &str) -> u32 {
        fs::read_to_string(self.backlight_dir(name).join("brightness"))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    }

    /// Current `in_illuminance_integration_time` of `iio:device{n}`, trimmed.
    pub fn integration(&self, n: u32) -> String {
        fs::read_to_string(self.iio_dir(n).join("in_illuminance_integration_time"))
            .unwrap()
            .trim()
            .to_string()
    }
}
