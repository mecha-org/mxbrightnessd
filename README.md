# mxbrightnessd

Brightness daemon for Mechanix OS. It owns the display backlight: it follows the ambient light sensor (automatic mode), applies the user's slider as an offset on the ambient curve, never goes below a minimum level, and serves all of it on D-Bus for the launcher.

It is a user service on the session bus, built on [mecha-wayland](https://github.com/mecha-org/mecha-wayland)'s `app`, `ring` and `dbus` crates (one io_uring loop, plus one thread that waits on the light sensor). Settings live in [mxconfig](https://github.com/mecha-org/mxconfig).

## D-Bus

Session bus, name `org.mechanix.Brightness`, path `/org/mechanix/Brightness`, interface `org.mechanix.Brightness`. Every property emits `PropertiesChanged`.

| Property | Type | Access | Meaning |
|---|---|---|---|
| `Brightness` | `d` | read-write | Perceived level, 0..1. Reads the level being faded to. Setting it in automatic mode shifts the curve; in manual mode it sets the level. Clamped to `[Minimum, 1]`; NaN/inf are rejected. |
| `Auto` | `b` | read-write | Automatic brightness on/off. |
| `Minimum` | `d` | read | The floor for every level. Set through settings. |
| `AmbientLux` | `d` | read | Smoothed sensor reading (sensor units, not calibrated lux), updated when it moves 5 %; 0 while automatic mode is off. |
| `AutoAvailable` | `b` | read | False when no light sensor was found; automatic mode then acts as manual. |

```sh
busctl --user get-property org.mechanix.Brightness /org/mechanix/Brightness org.mechanix.Brightness Brightness
busctl --user set-property org.mechanix.Brightness /org/mechanix/Brightness org.mechanix.Brightness Brightness d 0.4
busctl --user set-property org.mechanix.Brightness /org/mechanix/Brightness org.mechanix.Brightness Auto b false
```

## Settings

Namespace `org.mechanix.brightness` (`schema/org.mechanix.brightness.toml`). Edits apply live, e.g. `mxconfig set org.mechanix.brightness screen.minimum 0.08`.

- `screen.auto`, `screen.offset`, `screen.manual`: the user's state, written by the daemon about 2 s after the last slider change. Until `screen.manual` is set, the daemon starts at the backlight's current level and saves that as the manual level.
- `screen.minimum` (0.05), `screen.gamma` (2.2): `raw = max · p^gamma`, never below raw 1.
- `curve.lux` / `curve.level`: the ambient curve, sensor lux → perceived level, linear in log10(lux).
- `device.backlight`, `device.sensor`: pick devices by name when the automatic choice is wrong.

## Devices

The backlight is the one named in settings, else the only entry under `/sys/class/backlight` (preferring type `raw`, then `platform`, then `firmware`). The light sensor is the first IIO device with `in_illuminance_input`. On sensors with `in_illuminance_integration_time` the daemon switches between 0.8 s (dark) and 0.1 s (bright).

- **Mecha Comet:** backlight `32e60000.dsi.0` (max 255), sensor TI OPT3001. Needs kernel 7.2.4-0.202611120754 or later (the ALS interrupt fix).

## Build

```sh
cargo test
cargo build --release --target aarch64-unknown-linux-gnu   # or: cross build ...
```

## Install

As root on the device:

```sh
install -m755 mxbrightnessd /usr/bin/
install -m644 data/mxbrightnessd.service /usr/lib/systemd/user/
install -Dm644 data/brightness.conf /etc/systemd/user/mechanix-session.target.d/brightness.conf
install -m644 data/90-mxbrightnessd.rules /etc/udev/rules.d/
install -Dm644 schema/org.mechanix.brightness.toml /usr/share/mxconfig/schemas/org.mechanix.brightness.toml
usermod -aG video <user>
systemctl mask systemd-backlight@backlight:<name>.service   # mxbrightnessd restores the level itself
```

Then log in again (group membership) or reboot. Nothing else should write the backlight: on the Comet, disable `mechanix-system-service`.
