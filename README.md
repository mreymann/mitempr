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
mitempr [--config PATH] [--only-known] [--min-rssi DBM]
        [--exec PATH] [--exec-interval SECS]
        [--metrics-addr ADDR] [--pushgateway-url URL] [--push-interval SECS]
        [--format text|json] [-v|-vv] [-q]
        [--watchdog SECS] [--cooldown SECS]
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

## Configuration

Without a configuration file every decodable sensor is reported under whatever
name it advertises. `--config` adds names, calibration offsets and filtering —
see [`mitempr.toml.example`](mitempr.toml.example):

```toml
[general]
only_known = true   # ignore devices with no [[sensor]] block below
min_rssi = -90      # ignore advertisements weaker than this

[[sensor]]
mac = "A4:C1:38:00:11:22"
name = "Living Room"
temperature_offset = -0.3   # added to every reading from this sensor
humidity_offset = 1.5
bindkey = "231d..."         # only for sensors that encrypt (see below)
```

A configured `name` replaces the advertised one, so readings are labelled the
way you think about the room rather than `LYWSD03MMC`. `--only-known` and
`--min-rssi` on the command line override the `[general]` section, and
`only_known` is checked before any properties are read, so ignored devices cost
nothing but the event.

Unknown keys are rejected rather than ignored, so a typo like `temp_offset`
tells you about itself instead of silently doing nothing.

## Calling an external script

`--exec /path/to/script` runs a program once per reading. The reading arrives
twice, so the script can use whichever is more convenient: as `MITEMPR_*`
environment variables, and as one JSON object on standard input.

```sh
#!/bin/sh
# Every variable is always set; an empty one means the sensor did not report it.
[ -n "$MITEMPR_TEMPERATURE" ] || exit 0
echo "$MITEMPR_NAME is at ${MITEMPR_TEMPERATURE} C"
```

Available: `MITEMPR_MAC`, `MITEMPR_NAME`, `MITEMPR_FORMAT`,
`MITEMPR_TIMESTAMP`, `MITEMPR_RSSI`, `MITEMPR_TEMPERATURE`,
`MITEMPR_HUMIDITY`, `MITEMPR_BATTERY`, `MITEMPR_VOLTAGE`, `MITEMPR_PRESSURE`,
`MITEMPR_ILLUMINANCE`, `MITEMPR_MOISTURE`.

Sensors advertise every second or two, which is usually more often than you want
to call out to something: `--exec-interval 60` runs the script at most once a
minute per sensor. At most four hooks run at once and one that has not finished
in 30 seconds is killed, so a slow script cannot pile up processes on a Pi Zero.
When the hook cannot keep up its readings are dropped rather than queued, with a
warning — a backlog of stale temperatures is worse than a gap.

The script's stdout goes to `/dev/null` so a chatty script cannot corrupt
`--format json`; its stderr is left alone so you can see it complain.

## Prometheus

Two ways to get the readings into Prometheus.

**Scraping** (`--metrics-addr 0.0.0.0:9184`) serves `/metrics`:

```console
$ curl -s localhost:9184/metrics
# HELP mitempr_temperature_celsius Last temperature reported by the sensor, in degrees Celsius.
# TYPE mitempr_temperature_celsius gauge
mitempr_temperature_celsius{mac="A4:C1:38:A0:7B:03",name="Living Room",format="pvvx"} 22.5
```

Exported per sensor: `mitempr_temperature_celsius`, `mitempr_humidity_percent`,
`mitempr_pressure_hpa`, `mitempr_illuminance_lux`, `mitempr_moisture_percent`,
`mitempr_battery_percent`, `mitempr_battery_volts`, `mitempr_rssi_dbm`,
`mitempr_last_seen_timestamp_seconds` and the counter
`mitempr_readings_total`. Only measurements a sensor actually reports get a
series.

A gauge keeps its last value, so use `mitempr_last_seen_timestamp_seconds` to
tell a quiet sensor from a fresh one:

```promql
time() - mitempr_last_seen_timestamp_seconds > 600
```

**Pushing** (`--pushgateway-url http://gateway:9091`) POSTs the same text every
`--push-interval` seconds (30 by default), which is what you want when the Pi
cannot be reached from the Prometheus server. The URL defaults to job
`mitempr`; spell out the path (`http://gateway:9091/metrics/job/attic`) to
choose the job name or add grouping labels. Plain HTTP only — there is no TLS
client here, so put a reverse proxy in front of it or use scraping instead. A
Pushgateway that is down is logged and retried, never fatal.

## Encrypted sensors

BTHome v2 and MiBeacon can both encrypt their advertisements with AES-CCM and a
per-device bind key. Give the key in the sensor's config block and the
advertisement is decrypted before it is decoded:

```toml
[[sensor]]
mac = "A4:C1:38:00:11:22"
bindkey = "231d39c1d7cc1ab1aee224cd096db932"
```

The key is 32 hex characters. BTHome devices print theirs when you set one;
Xiaomi keys come out of the Mi Home account, which is what tools like
[Xiaomi-cloud-tokens-extractor](https://github.com/PiotrMachowski/Xiaomi-cloud-tokens-extractor)
are for.

Without a key an encrypted advertisement is skipped with a message rather than
parsed as if it were plaintext — ciphertext read as plaintext produces
believable-looking nonsense. A wrong key fails the packet's MIC check, which is
also reported rather than guessed at.

## TODOs

 - ... many more things to fiddle with ;-)

## Cross compiling

### Pi Zero W 1
- `cross build --release --target=arm-unknown-linux-musleabihf`

### Pi Zero W 2
- `cross build --release --target aarch64-unknown-linux-musl`
