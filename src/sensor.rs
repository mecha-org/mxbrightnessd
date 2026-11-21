//! The light sensor on its own thread, bridged to the ring.
//!
//! A sysfs lux read blocks for up to the integration time (0.8 s), so one
//! thread owns the sensor and answers requests over a socketpair; the
//! main end keeps one `recvmsg` armed on the ring.
//!
//! Wire format. Main to thread, 1 byte: `b'r'` read, `b'f'` set
//! [`Range::Fast`], `b's'` set [`Range::Sensitive`]. Thread to main,
//! 9 bytes: byte 0 is 0 for a reading, 1 for an error; bytes 1..9 are
//! the lux as `f64` LE, or the errno as `i64` LE. After a range change
//! the thread discards its next reading. The thread exits when the main
//! end closes.

use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::thread;

use app::prelude::*;
use ring::prelude::*;

use crate::devices::LightSensor;
use crate::events::{LuxRead, ReadLux, SetRange};
use crate::smooth::Range;

const READ: u8 = b'r';
const FAST: u8 = b'f';
const SENSITIVE: u8 = b's';
const REPLY_LEN: usize = 9;
const OK: u8 = 0;
const ERROR: u8 = 1;
/// Linux `EINVAL`: what the thread sends for unparsable or invalid lux.
const EINVAL: i32 = 22;

/// Owns the light sensor: spawns its reader thread and turns
/// [`ReadLux`] and [`SetRange`] into requests and replies into
/// [`LuxRead`]. Install after `RingModule`.
pub struct SensorModule {
    pub sensor: LightSensor,
}

impl Module for SensorModule {
    /// # Panics
    ///
    /// If the socketpair or the thread cannot be created, or if no
    /// [`Ring`] is installed.
    fn install(self, app: &mut App) {
        let (main, theirs) = UnixStream::pair().expect("sensor: socketpair");
        let sensor = self.sensor.clone();
        thread::Builder::new()
            .name("sensor".into())
            .spawn(move || serve(sensor, theirs))
            .expect("sensor: spawn thread");
        let reading = arm(&mut app.resource_mut::<Ring>(), &main);
        app.insert_resource(SensorLink {
            stream: main,
            sensor: self.sensor,
            reading,
            pending: false,
            closed: false,
        });
        app.system(on_read_lux).system(on_set_range).system(on_io);
    }
}

/// The main end of the socketpair and the one `recvmsg` armed on it.
struct SensorLink {
    stream: UnixStream,
    sensor: LightSensor,
    reading: Token,
    /// A `b'r'` was sent and its reply has not come.
    pending: bool,
    /// The thread is gone: its end closed or a request failed to send.
    closed: bool,
}
impl Resource for SensorLink {}

fn arm(ring: &mut Ring, stream: &UnixStream) -> Token {
    ring.recvmsg(stream.as_fd(), Vec::with_capacity(REPLY_LEN), 0)
}

fn on_read_lux(app: &mut App, _: &ReadLux) {
    let mut link = app.resource_mut::<SensorLink>();
    if link.pending {
        return;
    }
    let failed = if link.closed {
        Some("sensor thread gone".to_string())
    } else {
        match (&link.stream).write_all(&[READ]) {
            Ok(()) => {
                link.pending = true;
                None
            }
            Err(e) => {
                link.closed = true;
                Some(format!("sensor thread: {e}"))
            }
        }
    };
    if let Some(text) = failed {
        app.signal(LuxRead(Err(text)));
    }
}

fn on_set_range(app: &mut App, e: &SetRange) {
    let link = app.resource::<SensorLink>();
    if link.closed || !link.sensor.has_integration_time() {
        return;
    }
    let byte = match e.0 {
        Range::Fast => FAST,
        Range::Sensitive => SENSITIVE,
    };
    if let Err(err) = (&link.stream).write_all(&[byte]) {
        eprintln!("mxbrightnessd: sensor range: {err}");
    }
}

fn on_io(app: &mut App, e: &IoEvent) {
    let reply = {
        let (mut link, mut ring) = app.query::<(ResMut<SensorLink>, ResMut<Ring>)>();
        if e.token != link.reading {
            return;
        }
        let Some(done) = ring.finish(e.token) else {
            return;
        };
        let reply = if e.result == 0 {
            link.closed = true;
            if !link.pending {
                return;
            }
            Err("sensor thread gone".to_string())
        } else {
            let reply = if e.result < 0 {
                Err(format!(
                    "sensor link: {}",
                    io::Error::from_raw_os_error(-e.result)
                ))
            } else {
                decode(&done.buf)
            };
            link.reading = arm(&mut ring, &link.stream);
            reply
        };
        link.pending = false;
        reply
    };
    app.signal(LuxRead(reply));
}

/// A reply from the thread as a reading or error text.
fn decode(buf: &[u8]) -> Result<f64, String> {
    let Ok(reply) = <[u8; REPLY_LEN]>::try_from(buf) else {
        return Err(format!("short sensor reply: {} bytes", buf.len()));
    };
    let value: [u8; 8] = reply[1..].try_into().unwrap();
    match reply[0] {
        OK => Ok(f64::from_le_bytes(value)),
        ERROR => {
            let errno = i64::from_le_bytes(value) as i32;
            let err = io::Error::from_raw_os_error(errno);
            if errno == EINVAL {
                Err(format!("invalid lux: {err}"))
            } else {
                Err(format!("lux read: {err}"))
            }
        }
        other => Err(format!("bad sensor reply: status {other}")),
    }
}

/// The thread: answer requests on `stream` until the main end closes.
fn serve(sensor: LightSensor, mut stream: UnixStream) {
    let mut discard = false;
    loop {
        let mut request = [0u8; 1];
        if stream.read_exact(&mut request).is_err() {
            return;
        }
        let range = match request[0] {
            READ => {
                if discard {
                    discard = false;
                    let _ = read(&sensor);
                }
                if stream.write_all(&encode(read(&sensor))).is_err() {
                    return;
                }
                continue;
            }
            FAST => Range::Fast,
            SENSITIVE => Range::Sensitive,
            _ => continue,
        };
        match sensor.set_integration_time(range) {
            Ok(()) => discard = true,
            Err(e) => eprintln!("mxbrightnessd: sensor integration time: {e}"),
        }
    }
}

/// One reading: finite and non-negative lux, else the errno to report
/// (`EINVAL` for text that is not a valid lux value).
fn read(sensor: &LightSensor) -> Result<f64, i32> {
    match sensor.read_lux() {
        Ok(lux) if lux.is_finite() && lux >= 0.0 => Ok(lux),
        Ok(_) => Err(EINVAL),
        Err(e) => Err(e.raw_os_error().unwrap_or(EINVAL)),
    }
}

fn encode(reading: Result<f64, i32>) -> [u8; REPLY_LEN] {
    let mut reply = [0u8; REPLY_LEN];
    match reading {
        Ok(lux) => {
            reply[0] = OK;
            reply[1..].copy_from_slice(&lux.to_le_bytes());
        }
        Err(errno) => {
            reply[0] = ERROR;
            reply[1..].copy_from_slice(&i64::from(errno).to_le_bytes());
        }
    }
    reply
}
