#[path = "fake_sys.rs"]
mod fake_sys;

use std::fs;
use std::io::ErrorKind;

use fake_sys::FakeSys;
use mxbrightnessd::devices::{Backlight, LightSensor};
use mxbrightnessd::smooth::Range;

const DSI: &str = "32e60000.dsi.0";

#[test]
fn finds_single_backlight() {
    let fake = FakeSys::new();
    fake.backlight(DSI, "raw", 255, 178);
    let bl = Backlight::find(fake.root(), None).unwrap();
    assert_eq!(bl.name, DSI);
    assert_eq!(bl.max, 255);
    assert_eq!(bl.read_raw().unwrap(), 178);
}

#[test]
fn prefers_raw_over_firmware() {
    let fake = FakeSys::new();
    fake.backlight("acpi_video0", "firmware", 100, 50);
    fake.backlight("zz_panel", "raw", 255, 10);
    fake.backlight("pwm", "platform", 1023, 10);
    assert_eq!(Backlight::find(fake.root(), None).unwrap().name, "zz_panel");
}

#[test]
fn same_type_ties_by_name() {
    let fake = FakeSys::new();
    fake.backlight("b", "raw", 255, 10);
    fake.backlight("a", "raw", 255, 10);
    assert_eq!(Backlight::find(fake.root(), None).unwrap().name, "a");
}

#[test]
fn named_backlight_is_used() {
    let fake = FakeSys::new();
    fake.backlight("a", "raw", 255, 10);
    fake.backlight("b", "firmware", 100, 10);
    let bl = Backlight::find(fake.root(), Some("b")).unwrap();
    assert_eq!(bl.name, "b");
    assert_eq!(bl.max, 100);
}

#[test]
fn named_backlight_missing_is_error() {
    let fake = FakeSys::new();
    fake.backlight(DSI, "raw", 255, 178);
    assert!(Backlight::find(fake.root(), Some("nope")).is_err());
}

#[test]
fn no_backlight_is_not_found() {
    let fake = FakeSys::new();
    let err = Backlight::find(fake.root(), None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::NotFound);
}

#[test]
fn write_raw_writes_text() {
    let fake = FakeSys::new();
    fake.backlight(DSI, "raw", 255, 178);
    let bl = Backlight::find(fake.root(), None).unwrap();
    bl.write_raw(40).unwrap();
    assert_eq!(fake.raw(DSI), 40);
    assert_eq!(bl.read_raw().unwrap(), 40);
}

#[test]
fn finds_sensor_with_illuminance_skipping_others() {
    let fake = FakeSys::new();
    fake.sensor(0, "lis2mdl", "", false);
    fake.sensor(3, "opt3001", "0.320000", true);
    let s = LightSensor::find(fake.root(), None).unwrap();
    assert_eq!(s.name, "opt3001");
    assert_eq!(s.read_lux().unwrap(), 0.32);
}

#[test]
fn sensors_in_numeric_order() {
    let fake = FakeSys::new();
    fake.sensor(10, "ten", "1", false);
    fake.sensor(2, "two", "1", false);
    assert_eq!(LightSensor::find(fake.root(), None).unwrap().name, "two");
}

#[test]
fn named_sensor_is_used() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "1", false);
    fake.sensor(1, "tsl2591", "2", false);
    let s = LightSensor::find(fake.root(), Some("tsl2591")).unwrap();
    assert_eq!(s.name, "tsl2591");
    assert_eq!(s.read_lux().unwrap(), 2.0);
    assert!(LightSensor::find(fake.root(), Some("nope")).is_none());
}

#[test]
fn no_sensor_is_none() {
    let fake = FakeSys::new();
    assert!(LightSensor::find(fake.root(), None).is_none());
    fake.sensor(0, "lis2mdl", "", false);
    assert!(LightSensor::find(fake.root(), None).is_none());
}

#[test]
fn bad_lux_text_is_invalid_data() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "garbage", false);
    let s = LightSensor::find(fake.root(), None).unwrap();
    assert_eq!(s.read_lux().unwrap_err().kind(), ErrorKind::InvalidData);
}

#[test]
fn lux_read_error_is_passed_through() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "1", false);
    let s = LightSensor::find(fake.root(), None).unwrap();
    let input = fake
        .root()
        .join("sys/bus/iio/devices/iio:device0/in_illuminance_input");
    fs::remove_file(&input).unwrap();
    assert_eq!(s.read_lux().unwrap_err().kind(), ErrorKind::NotFound);
}

#[test]
fn integration_time_written() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "0.320000", true);
    let s = LightSensor::find(fake.root(), None).unwrap();
    assert!(s.has_integration_time());
    s.set_integration_time(Range::Fast).unwrap();
    assert_eq!(fake.integration(0), "0.1");
    s.set_integration_time(Range::Sensitive).unwrap();
    assert_eq!(fake.integration(0), "0.8");
}

#[test]
fn no_integration_time() {
    let fake = FakeSys::new();
    fake.sensor(0, "opt3001", "1", false);
    let s = LightSensor::find(fake.root(), None).unwrap();
    assert!(!s.has_integration_time());
}
