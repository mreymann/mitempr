# mitempr

Read data from Bluetooth environmental sensors in BTHome v2, PVVX and LYWSDCGQ formats.  
Strongly inspired by [Mitemperature2](https://github.com/JsBergbau/MiTemperature2). Thank you, JsBergbau!

## Why

 - learn a bit of Rust
 - prove how much AI can help an idiot (like me)
 - try to use less resources on my poor Pi Zero W

## Status

 - nicely cross compiles to armv6 (Pi Zero W), armv7 (Pi Zero W 2)
 - continuous scanning now works!

## Usage

```
mitempr [--format text|json] [-v|-vv] [-q] [--watchdog SECS] [--cooldown SECS]
```

`--format text` (the default) writes one line per reading through the logger:

```
[2026-07-31T10:12:44Z INFO  mitempr::output] A4:C1:38:A0:7B:03 [pvvx] LYWSD03MMC -67 dBm | 22.90 C 64.25 %RH 2.333 V 16 % battery
```

`--format json` writes one JSON object per reading straight to stdout, so it can
be piped somewhere useful. Measurements a sensor does not report are left out
rather than sent as `null`:

```console
$ mitempr --format json | jq -c '{address, temperature_celsius}'
{"address":"A4:C1:38:A0:7B:03","temperature_celsius":22.9}
```

Log verbosity: `-v` adds why a payload could not be decoded, `-vv` adds the raw
service-data bytes, `-q` leaves only warnings and errors. `RUST_LOG` overrides
all of them. Readings written with `--format json` go to stdout and are never
suppressed by `-q`.

`--watchdog` restarts discovery when no reading has arrived for that many
seconds, which recovers a wedged adapter; `--cooldown` is the pause before the
restart.

## TODOs

 - also decode **encrypted** data
 - URL callback to Prometheus Push Gateway
 - call external scripts
 - define sensors in a config file & filter defined sensors
 - and many more things to fiddle with ;-)

## Cross compiling

### Pi Zero W 1
- `cross build --release --target=arm-unknown-linux-musleabihf`

### Pi Zero W 2
- `cross build --release --target aarch64-unknown-linux-musl`