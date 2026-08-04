//! Prometheus support: a `/metrics` endpoint to scrape, and pushing the same
//! exposition text to a Pushgateway.
//!
//! The exposition format is written by hand rather than pulled in from the
//! `prometheus` crate. There are ten metrics with two labels, the format is a
//! handful of lines of text, and the binary has to fit comfortably on a Pi Zero.

use crate::output::Reading;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// The exposition format this writes. Version 0.0.4 is what Prometheus and the
/// Pushgateway both understand.
const CONTENT_TYPE: &str = "text/plain; version=0.0.4; charset=utf-8";

/// Long enough for any request line worth reading; the rest of the request is
/// ignored.
const REQUEST_PEEK: usize = 1024;

/// Job name used when the Pushgateway URL does not name one itself.
const DEFAULT_JOB: &str = "mitempr";

/// One metric family: its name, what it means, and how to read a value out of a
/// sensor's last reading.
struct Metric {
    name: &'static str,
    help: &'static str,
    value: fn(&Snapshot) -> Option<f64>,
}

/// Every gauge that is derived from a reading. Adding a measurement means adding
/// one row here.
const GAUGES: &[Metric] = &[
    Metric {
        name: "mitempr_temperature_celsius",
        help: "Last temperature reported by the sensor, in degrees Celsius.",
        value: |s| s.temperature_celsius.map(f64::from),
    },
    Metric {
        name: "mitempr_humidity_percent",
        help: "Last relative humidity reported by the sensor, in percent.",
        value: |s| s.humidity_percent.map(f64::from),
    },
    Metric {
        name: "mitempr_pressure_hpa",
        help: "Last air pressure reported by the sensor, in hectopascal.",
        value: |s| s.pressure_hpa.map(f64::from),
    },
    Metric {
        name: "mitempr_illuminance_lux",
        help: "Last illuminance reported by the sensor, in lux.",
        value: |s| s.illuminance_lux.map(f64::from),
    },
    Metric {
        name: "mitempr_moisture_percent",
        help: "Last moisture reported by the sensor, in percent.",
        value: |s| s.moisture_percent.map(f64::from),
    },
    Metric {
        name: "mitempr_battery_percent",
        help: "Last battery charge reported by the sensor, in percent.",
        value: |s| s.battery_percent.map(f64::from),
    },
    Metric {
        name: "mitempr_battery_volts",
        help: "Last battery voltage reported by the sensor, in volts.",
        value: |s| s.voltage_volts.map(f64::from),
    },
    Metric {
        name: "mitempr_rssi_dbm",
        help: "Signal strength of the sensor's last advertisement, in dBm.",
        value: |s| s.rssi_dbm.map(f64::from),
    },
    Metric {
        name: "mitempr_last_seen_timestamp_seconds",
        help: "When the sensor was last decoded, in seconds since the Unix epoch.",
        value: |s| Some(s.last_seen as f64),
    },
];

/// What is remembered about one sensor between scrapes.
#[derive(Debug, Default)]
struct Snapshot {
    name: Option<String>,
    format: &'static str,
    rssi_dbm: Option<i16>,
    temperature_celsius: Option<f32>,
    humidity_percent: Option<f32>,
    battery_percent: Option<u8>,
    voltage_volts: Option<f32>,
    pressure_hpa: Option<f32>,
    illuminance_lux: Option<f32>,
    moisture_percent: Option<f32>,
    last_seen: u64,
    readings: u64,
}

/// The last reading from every sensor seen so far.
///
/// Keyed by MAC as text, in a BTreeMap so that a scrape always lists the sensors
/// in the same order.
#[derive(Debug, Default)]
pub struct Registry {
    sensors: Mutex<BTreeMap<String, Snapshot>>,
}

impl Registry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fold a reading into what will be served on the next scrape.
    ///
    /// Measurements are merged rather than replaced: a sensor that alternates
    /// between a temperature packet and a battery packet would otherwise have
    /// each series flapping in and out of existence.
    pub fn record(&self, reading: &Reading) {
        let mut sensors = self.lock();
        let snapshot = sensors.entry(reading.address.clone()).or_default();

        if reading.name.is_some() {
            snapshot.name = reading.name.clone();
        }
        snapshot.format = reading.format;
        snapshot.last_seen = reading.timestamp;
        snapshot.readings += 1;

        merge(&mut snapshot.rssi_dbm, reading.rssi_dbm);
        merge(
            &mut snapshot.temperature_celsius,
            reading.temperature_celsius,
        );
        merge(&mut snapshot.humidity_percent, reading.humidity_percent);
        merge(&mut snapshot.battery_percent, reading.battery_percent);
        merge(&mut snapshot.voltage_volts, reading.voltage_volts);
        merge(&mut snapshot.pressure_hpa, reading.pressure_hpa);
        merge(&mut snapshot.illuminance_lux, reading.illuminance_lux);
        merge(&mut snapshot.moisture_percent, reading.moisture_percent);
    }

    /// Render everything in the Prometheus text exposition format.
    pub fn render(&self) -> String {
        let sensors = self.lock();
        let mut out = String::new();

        for metric in GAUGES {
            let mut wrote_header = false;
            for (address, snapshot) in sensors.iter() {
                let Some(value) = (metric.value)(snapshot) else {
                    continue;
                };
                if !wrote_header {
                    let _ = writeln!(out, "# HELP {} {}", metric.name, metric.help);
                    let _ = writeln!(out, "# TYPE {} gauge", metric.name);
                    wrote_header = true;
                }
                let _ = writeln!(out, "{}{} {value}", metric.name, labels(address, snapshot));
            }
        }

        if !sensors.is_empty() {
            let _ = writeln!(
                out,
                "# HELP mitempr_readings_total Advertisements successfully decoded from the sensor."
            );
            let _ = writeln!(out, "# TYPE mitempr_readings_total counter");
            for (address, snapshot) in sensors.iter() {
                let _ = writeln!(
                    out,
                    "mitempr_readings_total{} {}",
                    labels(address, snapshot),
                    snapshot.readings
                );
            }
        }

        out
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Snapshot>> {
        self.sensors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Keep the previous value when this advertisement did not carry the
/// measurement.
fn merge<T>(current: &mut Option<T>, incoming: Option<T>) {
    if let Some(value) = incoming {
        *current = Some(value);
    }
}

fn labels(address: &str, snapshot: &Snapshot) -> String {
    format!(
        "{{mac=\"{}\",name=\"{}\",format=\"{}\"}}",
        escape(address),
        escape(snapshot.name.as_deref().unwrap_or_default()),
        escape(snapshot.format),
    )
}

/// Escape a label value as the exposition format requires. A sensor named from a
/// config file can contain anything, including a quote.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            other => out.push(other),
        }
    }
    out
}

/// Serve `/metrics` until the listener fails.
pub async fn serve(addr: SocketAddr, registry: std::sync::Arc<Registry>) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    log::info!("serving metrics on http://{addr}/metrics");

    loop {
        let (mut stream, peer) = match listener.accept().await {
            Ok(accepted) => accepted,
            Err(e) => {
                log::warn!("metrics listener: {e}");
                continue;
            }
        };

        let registry = std::sync::Arc::clone(&registry);
        tokio::spawn(async move {
            if let Err(e) = respond(&mut stream, &registry).await {
                log::debug!("metrics request from {peer} failed: {e}");
            }
        });
    }
}

async fn respond(stream: &mut TcpStream, registry: &Registry) -> std::io::Result<()> {
    // Only the request line matters, and it is the first thing on the wire, so
    // there is no need to read the whole request.
    let mut buffer = [0u8; REQUEST_PEEK];
    let read = stream.read(&mut buffer).await?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let mut fields = request.split_whitespace();
    let method = fields.next().unwrap_or_default();
    let path = fields.next().unwrap_or_default();

    let response = match (method, path) {
        ("GET", "/metrics") => {
            let body = registry.render();
            http_response("200 OK", CONTENT_TYPE, &body)
        }
        ("GET", "/") => http_response("200 OK", "text/plain; charset=utf-8", "See /metrics\n"),
        ("GET", _) => http_response("404 Not Found", "text/plain; charset=utf-8", "Not found\n"),
        _ => http_response(
            "405 Method Not Allowed",
            "text/plain; charset=utf-8",
            "Only GET is supported\n",
        ),
    };

    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    // Ignore a shutdown error: the client may well have hung up already.
    let _ = stream.shutdown().await;
    Ok(())
}

fn http_response(status: &str, content_type: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

/// Where to POST the exposition text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PushTarget {
    host: String,
    port: u16,
    path: String,
}

impl PushTarget {
    /// Parse `http://host[:port][/path]`.
    ///
    /// Plain HTTP only: this uses a hand-written client, so there is no TLS. A
    /// Pushgateway that needs TLS wants a reverse proxy in front of it, or use
    /// --metrics-addr and let Prometheus scrape instead.
    pub fn parse(url: &str) -> Result<Self, String> {
        let rest = url
            .strip_prefix("http://")
            .ok_or_else(|| format!("{url:?} must start with http:// (TLS is not supported)"))?;

        let (authority, path) = match rest.find('/') {
            Some(index) => (&rest[..index], &rest[index..]),
            None => (rest, ""),
        };

        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (
                host,
                port.parse::<u16>()
                    .map_err(|_| format!("{port:?} is not a port number"))?,
            ),
            None => (authority, 9091),
        };

        if host.is_empty() {
            return Err(format!("{url:?} has no host"));
        }

        // The Pushgateway API lives under /metrics/job/<name>. Accept a full path
        // so the job and any grouping labels can be chosen, and fall back to a
        // sensible default when only a base URL is given.
        let path = if path.is_empty() || path == "/" {
            format!("/metrics/job/{DEFAULT_JOB}")
        } else {
            path.to_owned()
        };

        Ok(Self {
            host: host.to_owned(),
            port,
            path,
        })
    }
}

/// POST the exposition text to the Pushgateway on a fixed interval.
pub async fn push_periodically(
    target: PushTarget,
    registry: std::sync::Arc<Registry>,
    interval: Duration,
) {
    log::info!(
        "pushing metrics to http://{}:{}{} every {interval:?}",
        target.host,
        target.port,
        target.path
    );

    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;

        let body = registry.render();
        if body.is_empty() {
            log::debug!("nothing to push yet");
            continue;
        }

        if let Err(e) = push_once(&target, &body).await {
            // A Pushgateway that is down should not take the scanner with it.
            log::warn!("could not push metrics: {e}");
        }
    }
}

async fn push_once(target: &PushTarget, body: &str) -> std::io::Result<()> {
    let mut stream = TcpStream::connect((target.host.as_str(), target.port)).await?;

    let request = format!(
        "POST {} HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: {CONTENT_TYPE}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        target.path,
        target.host,
        target.port,
        body.len()
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut response = String::new();
    let mut buffer = [0u8; REQUEST_PEEK];
    let read = stream.read(&mut buffer).await?;
    response.push_str(&String::from_utf8_lossy(&buffer[..read]));

    let status = response.lines().next().unwrap_or_default();
    if status.contains(" 200") || status.contains(" 202") {
        log::debug!("pushed {} bytes of metrics", body.len());
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "pushgateway answered {status:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::{SensorData, SensorFormat};
    use bluer::Address;

    fn reading(address: [u8; 6], name: Option<&str>, data: SensorData) -> Reading {
        let mut reading = Reading::new(
            Address::new(address),
            name.map(str::to_string),
            Some(-67),
            &data,
        );
        reading.timestamp = 1_770_000_000;
        reading
    }

    fn living_room() -> Reading {
        reading(
            [0xA4, 0xC1, 0x38, 0xA0, 0x7B, 0x03],
            Some("Living Room"),
            SensorData {
                format: SensorFormat::Pvvx,
                temperature: Some(22.5),
                humidity: Some(64.0),
                battery: Some(16),
                ..Default::default()
            },
        )
    }

    #[test]
    fn a_single_sensor_renders_one_series_per_measurement() {
        let registry = Registry::new();
        registry.record(&living_room());

        let rendered = registry.render();

        let labels = r#"{mac="A4:C1:38:A0:7B:03",name="Living Room",format="pvvx"}"#;
        assert!(rendered.contains(&format!("mitempr_temperature_celsius{labels} 22.5\n")));
        assert!(rendered.contains(&format!("mitempr_humidity_percent{labels} 64\n")));
        assert!(rendered.contains(&format!("mitempr_battery_percent{labels} 16\n")));
        assert!(rendered.contains(&format!("mitempr_rssi_dbm{labels} -67\n")));
        assert!(rendered.contains(&format!(
            "mitempr_last_seen_timestamp_seconds{labels} 1770000000\n"
        )));
        assert!(rendered.contains(&format!("mitempr_readings_total{labels} 1\n")));
    }

    /// A measurement no sensor reports must not produce an empty family, and its
    /// HELP/TYPE header must not be written either.
    #[test]
    fn absent_measurements_produce_no_series_at_all() {
        let registry = Registry::new();
        registry.record(&living_room());

        let rendered = registry.render();

        assert!(!rendered.contains("mitempr_pressure_hpa"), "{rendered}");
        assert!(!rendered.contains("mitempr_illuminance_lux"), "{rendered}");
        assert!(!rendered.contains("mitempr_moisture_percent"), "{rendered}");
    }

    #[test]
    fn each_metric_declares_help_and_type_exactly_once() {
        let registry = Registry::new();
        registry.record(&living_room());
        registry.record(&reading(
            [0xA4, 0xC1, 0x38, 0xAA, 0xBB, 0xCC],
            Some("Balcony"),
            SensorData {
                format: SensorFormat::BtHome,
                temperature: Some(11.25),
                ..Default::default()
            },
        ));

        let rendered = registry.render();

        assert_eq!(
            rendered
                .matches("# TYPE mitempr_temperature_celsius gauge")
                .count(),
            1
        );
        assert_eq!(
            rendered
                .matches("# HELP mitempr_temperature_celsius")
                .count(),
            1
        );
        assert_eq!(
            rendered.matches("mitempr_temperature_celsius{").count(),
            2,
            "one series per sensor"
        );
    }

    /// A sensor that alternates between packet types must not have its series
    /// flapping in and out of existence between scrapes.
    #[test]
    fn measurements_from_earlier_advertisements_are_kept() {
        let registry = Registry::new();
        registry.record(&living_room());
        registry.record(&reading(
            [0xA4, 0xC1, 0x38, 0xA0, 0x7B, 0x03],
            Some("Living Room"),
            SensorData {
                format: SensorFormat::Pvvx,
                battery: Some(15),
                ..Default::default()
            },
        ));

        let rendered = registry.render();

        assert!(
            rendered.contains("mitempr_temperature_celsius"),
            "the temperature from the first advertisement is still there"
        );
        assert!(rendered.contains(" 15\n"), "the battery was updated");
        assert!(
            rendered.contains("mitempr_readings_total{mac=\"A4:C1:38:A0:7B:03\",name=\"Living Room\",format=\"pvvx\"} 2"),
            "{rendered}"
        );
    }

    #[test]
    fn label_values_are_escaped() {
        let registry = Registry::new();
        registry.record(&reading(
            [0; 6],
            Some("Sam\"s \\ Room\nUpstairs"),
            SensorData {
                format: SensorFormat::BtHome,
                temperature: Some(20.0),
                ..Default::default()
            },
        ));

        let rendered = registry.render();

        assert!(
            rendered.contains(r#"name="Sam\"s \\ Room\nUpstairs""#),
            "{rendered}"
        );
        assert_eq!(
            rendered.lines().count(),
            // temperature, rssi, last_seen and readings_total, each with a
            // HELP/TYPE pair.
            4 * 3,
            "an unescaped newline would have added a line: {rendered}"
        );
    }

    #[test]
    fn an_empty_registry_renders_nothing() {
        assert_eq!(Registry::new().render(), "");
    }

    #[test]
    fn a_base_url_gets_the_default_pushgateway_path() {
        let target = PushTarget::parse("http://gateway:9091").expect("should parse");

        assert_eq!(
            target,
            PushTarget {
                host: "gateway".to_string(),
                port: 9091,
                path: "/metrics/job/mitempr".to_string(),
            }
        );
        assert_eq!(
            PushTarget::parse("http://gateway:9091/").expect("should parse"),
            target,
            "a bare slash is the same as no path"
        );
    }

    #[test]
    fn the_port_defaults_to_the_pushgateway_one() {
        let target = PushTarget::parse("http://gateway").expect("should parse");

        assert_eq!(target.port, 9091);
    }

    /// Spelling out the path is how a different job name or grouping labels get
    /// chosen, so it must be kept verbatim.
    #[test]
    fn an_explicit_path_is_kept() {
        let target =
            PushTarget::parse("http://gateway:9091/metrics/job/attic/room/loft").expect("parse");

        assert_eq!(target.path, "/metrics/job/attic/room/loft");
    }

    #[test]
    fn unusable_push_urls_are_rejected() {
        for url in [
            "https://gateway:9091",
            "gateway:9091",
            "http://gateway:notaport",
            "http://",
        ] {
            assert!(PushTarget::parse(url).is_err(), "{url} should be rejected");
        }
    }

    /// Ask the real handler over a real socket, so the status line, the
    /// content type and the framing are all exercised.
    async fn request(line: &str, registry: Registry) -> String {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("should bind a loopback port");
        let addr = listener.local_addr().expect("should have a local address");

        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("should accept");
            respond(&mut stream, &registry)
                .await
                .expect("should respond");
        });

        let mut client = TcpStream::connect(addr).await.expect("should connect");
        client
            .write_all(format!("{line}\r\nHost: localhost\r\n\r\n").as_bytes())
            .await
            .expect("should send the request");

        let mut response = Vec::new();
        client
            .read_to_end(&mut response)
            .await
            .expect("should read the response");
        server.await.expect("server task should finish");

        String::from_utf8(response).expect("response should be UTF-8")
    }

    #[tokio::test]
    async fn the_metrics_endpoint_serves_the_exposition_text() {
        let registry = Registry::new();
        registry.record(&living_room());
        let expected_body = registry.render();

        let response = request("GET /metrics HTTP/1.1", registry).await;

        assert!(response.starts_with("HTTP/1.1 200 OK\r\n"), "{response}");
        assert!(
            response.contains("Content-Type: text/plain; version=0.0.4; charset=utf-8\r\n"),
            "{response}"
        );
        assert!(
            response.contains(&format!("Content-Length: {}\r\n", expected_body.len())),
            "{response}"
        );
        let body = response
            .split_once("\r\n\r\n")
            .expect("headers should be terminated")
            .1;
        assert_eq!(body, expected_body);
    }

    #[tokio::test]
    async fn other_paths_are_not_found() {
        let response = request("GET /wat HTTP/1.1", Registry::new()).await;

        assert!(
            response.starts_with("HTTP/1.1 404 Not Found\r\n"),
            "{response}"
        );
    }

    #[tokio::test]
    async fn only_get_is_allowed() {
        let response = request("POST /metrics HTTP/1.1", Registry::new()).await;

        assert!(
            response.starts_with("HTTP/1.1 405 Method Not Allowed\r\n"),
            "{response}"
        );
    }
}
