//! Backlight and ambient light sensor discovery and sysfs I/O.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::smooth::Range;

const BACKLIGHT_CLASS: &str = "sys/class/backlight";
const IIO_DEVICES: &str = "sys/bus/iio/devices";
const IIO_PREFIX: &str = "iio:device";
const LUX_INPUT: &str = "in_illuminance_input";
const INTEGRATION_TIME: &str = "in_illuminance_integration_time";

/// A display backlight under `/sys/class/backlight`.
#[derive(Clone, Debug)]
pub struct Backlight {
    pub name: String,
    pub max: u32,
    dir: PathBuf,
}

impl Backlight {
    /// Finds the backlight under `root/sys/class/backlight`: the entry
    /// `name` when given, else the only one, else the best by `type`
    /// (`raw`, then `platform`, then `firmware`), ties broken by name.
    pub fn find(root: &Path, name: Option<&str>) -> io::Result<Backlight> {
        let class = root.join(BACKLIGHT_CLASS);
        let name = match name {
            Some(name) => name.to_string(),
            None => {
                let mut found = Vec::new();
                match fs::read_dir(&class) {
                    Ok(entries) => {
                        for entry in entries {
                            let entry = entry?;
                            let name = entry.file_name().to_string_lossy().into_owned();
                            let ty =
                                fs::read_to_string(entry.path().join("type")).unwrap_or_default();
                            found.push((type_rank(ty.trim()), name));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                found.sort();
                match found.into_iter().next() {
                    Some((_, name)) => name,
                    None => {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("no backlight in {}", class.display()),
                        ));
                    }
                }
            }
        };
        let dir = class.join(&name);
        let max = read_u32(&dir.join("max_brightness"))?;
        Ok(Backlight { name, max, dir })
    }

    /// Reads the current `brightness`.
    pub fn read_raw(&self) -> io::Result<u32> {
        read_u32(&self.dir.join("brightness"))
    }

    /// Writes `raw` to `brightness`.
    pub fn write_raw(&self, raw: u32) -> io::Result<()> {
        fs::write(self.dir.join("brightness"), raw.to_string())
    }
}

/// Preference of a backlight `type`: lower is better.
fn type_rank(ty: &str) -> u8 {
    match ty {
        "raw" => 0,
        "platform" => 1,
        "firmware" => 2,
        _ => 3,
    }
}

fn read_u32(path: &Path) -> io::Result<u32> {
    let text = fs::read_to_string(path)?;
    text.trim().parse().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {e}", path.display()),
        )
    })
}

/// An IIO ambient light sensor with `in_illuminance_input`.
#[derive(Clone, Debug)]
pub struct LightSensor {
    pub name: String,
    dir: PathBuf,
}

impl LightSensor {
    /// Finds the first `iio:deviceN` under `root/sys/bus/iio/devices`, in
    /// order of N, that has `in_illuminance_input` (and, when `name` is
    /// given, whose `name` file matches it).
    pub fn find(root: &Path, name: Option<&str>) -> Option<LightSensor> {
        let mut devices: Vec<(u32, PathBuf)> = fs::read_dir(root.join(IIO_DEVICES))
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let n = entry
                    .file_name()
                    .to_str()?
                    .strip_prefix(IIO_PREFIX)?
                    .parse()
                    .ok()?;
                Some((n, entry.path()))
            })
            .collect();
        devices.sort();
        devices.into_iter().find_map(|(_, dir)| {
            if !dir.join(LUX_INPUT).is_file() {
                return None;
            }
            let found = fs::read_to_string(dir.join("name"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            if name.is_some_and(|want| want != found) {
                return None;
            }
            Some(LightSensor { name: found, dir })
        })
    }

    /// Reads `in_illuminance_input` in lux. Read errors are returned as-is;
    /// unparsable text is `InvalidData`.
    pub fn read_lux(&self) -> io::Result<f64> {
        let text = fs::read_to_string(self.dir.join(LUX_INPUT))?;
        text.trim().parse().map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{LUX_INPUT}: {:?}: {e}", text.trim()),
            )
        })
    }

    /// Whether the sensor exposes `in_illuminance_integration_time`.
    pub fn has_integration_time(&self) -> bool {
        self.dir.join(INTEGRATION_TIME).is_file()
    }

    /// Writes the integration time for `r`.
    pub fn set_integration_time(&self, r: Range) -> io::Result<()> {
        fs::write(self.dir.join(INTEGRATION_TIME), r.sysfs())
    }
}
