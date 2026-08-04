//! Optional TOML configuration: naming sensors, calibrating them and ignoring
//! everything else.

use crate::decoder::SensorData;
use bluer::Address;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::{fmt, fs, io};

/// The file as it is written on disk.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    #[serde(default)]
    general: General,
    /// `[[sensor]]` blocks.
    #[serde(default, rename = "sensor")]
    sensors: Vec<SensorEntry>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct General {
    /// Ignore any sensor that has no `[[sensor]]` block.
    #[serde(default)]
    only_known: bool,
    /// Ignore advertisements weaker than this, in dBm.
    min_rssi: Option<i16>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SensorEntry {
    mac: String,
    name: Option<String>,
    /// Added to every temperature reading from this sensor, in degrees Celsius.
    #[serde(default)]
    temperature_offset: f32,
    /// Added to every humidity reading from this sensor, in percent.
    #[serde(default)]
    humidity_offset: f32,
}

/// What is known about one configured sensor.
#[derive(Debug, Default, PartialEq)]
pub struct SensorSettings {
    pub name: Option<String>,
    pub temperature_offset: f32,
    pub humidity_offset: f32,
}

impl SensorSettings {
    /// Apply this sensor's calibration offsets in place.
    pub fn calibrate(&self, data: &mut SensorData) {
        if let Some(temperature) = data.temperature.as_mut() {
            *temperature += self.temperature_offset;
        }
        if let Some(humidity) = data.humidity.as_mut() {
            *humidity += self.humidity_offset;
        }
    }
}

/// The resolved configuration used while scanning.
#[derive(Debug, Default)]
pub struct Config {
    sensors: HashMap<Address, SensorSettings>,
    only_known: bool,
    min_rssi: Option<i16>,
}

impl Config {
    /// Read and validate a configuration file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;

        let file: File = toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })?;

        let mut sensors = HashMap::with_capacity(file.sensors.len());
        for entry in file.sensors {
            let address = parse_address(&entry.mac)?;
            let settings = SensorSettings {
                name: entry.name,
                temperature_offset: entry.temperature_offset,
                humidity_offset: entry.humidity_offset,
            };
            if sensors.insert(address, settings).is_some() {
                return Err(ConfigError::DuplicateSensor(entry.mac));
            }
        }

        Ok(Self {
            sensors,
            only_known: file.general.only_known,
            min_rssi: file.general.min_rssi,
        })
    }

    /// Turn on `only_known` regardless of what the file said.
    pub fn set_only_known(&mut self) {
        self.only_known = true;
    }

    /// Override the file's `min_rssi`.
    pub fn set_min_rssi(&mut self, min_rssi: i16) {
        self.min_rssi = Some(min_rssi);
    }

    pub fn configured_sensors(&self) -> usize {
        self.sensors.len()
    }

    pub fn only_known(&self) -> bool {
        self.only_known
    }

    /// Whether this device is worth the D-Bus round-trip needed to read its
    /// properties. Checked before fetching them, so filtered-out devices cost
    /// nothing but the event.
    pub fn accepts_address(&self, address: Address) -> bool {
        !self.only_known || self.sensors.contains_key(&address)
    }

    /// Whether an advertisement is strong enough to report. An unknown RSSI is
    /// accepted: absent is not the same as weak.
    pub fn accepts_rssi(&self, rssi: Option<i16>) -> bool {
        match (self.min_rssi, rssi) {
            (Some(minimum), Some(actual)) => actual >= minimum,
            _ => true,
        }
    }

    pub fn settings(&self, address: Address) -> Option<&SensorSettings> {
        self.sensors.get(&address)
    }
}

/// Parse `A4:C1:38:00:11:22`.
fn parse_address(value: &str) -> Result<Address, ConfigError> {
    let invalid = || ConfigError::InvalidAddress(value.to_owned());

    let mut bytes = [0u8; 6];
    let mut groups = value.split(':');
    for byte in &mut bytes {
        let group = groups.next().ok_or_else(invalid)?;
        if group.len() != 2 {
            return Err(invalid());
        }
        *byte = u8::from_str_radix(group, 16).map_err(|_| invalid())?;
    }
    if groups.next().is_some() {
        return Err(invalid());
    }

    Ok(Address::new(bytes))
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: io::Error,
    },
    // Boxed because toml::de::Error is large and this variant is the rare path.
    Parse {
        path: PathBuf,
        source: Box<toml::de::Error>,
    },
    InvalidAddress(String),
    DuplicateSensor(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse { path, source } => write!(f, "cannot parse {}: {source}", path.display()),
            Self::InvalidAddress(value) => {
                write!(f, "{value:?} is not a MAC address like A4:C1:38:00:11:22")
            }
            Self::DuplicateSensor(value) => write!(f, "{value} is configured more than once"),
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } => Some(source),
            Self::Parse { source, .. } => Some(source),
            Self::InvalidAddress(_) | Self::DuplicateSensor(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn living_room() -> Address {
        Address::new([0xA4, 0xC1, 0x38, 0x00, 0x11, 0x22])
    }

    fn unknown() -> Address {
        Address::new([0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01])
    }

    /// `Config::load` takes a path, so tests need a real file.
    fn load(toml: &str) -> Result<Config, ConfigError> {
        let path = std::env::temp_dir().join(format!(
            "mitempr-config-test-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ));
        let mut file = fs::File::create(&path).expect("temp file should be creatable");
        file.write_all(toml.as_bytes())
            .expect("temp file should be writable");
        drop(file);

        let result = Config::load(&path);
        let _ = fs::remove_file(&path);
        result
    }

    #[test]
    fn an_empty_file_accepts_everything() {
        let config = load("").expect("an empty config is valid");

        assert_eq!(config.configured_sensors(), 0);
        assert!(config.accepts_address(unknown()));
        assert!(config.accepts_rssi(Some(-120)));
    }

    #[test]
    fn sensors_are_looked_up_by_address() {
        let config = load(
            r#"
            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            name = "Living Room"
            temperature_offset = -0.3
            humidity_offset = 1.5
            "#,
        )
        .expect("config should parse");

        let settings = config
            .settings(living_room())
            .expect("sensor should be found");
        assert_eq!(settings.name.as_deref(), Some("Living Room"));
        assert!((settings.temperature_offset - -0.3).abs() < 0.0001);
        assert!((settings.humidity_offset - 1.5).abs() < 0.0001);
        assert!(config.settings(unknown()).is_none());
    }

    #[test]
    fn a_sensor_needs_nothing_but_a_mac() {
        let config = load(
            r#"
            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            "#,
        )
        .expect("config should parse");

        let settings = config
            .settings(living_room())
            .expect("sensor should be found");
        assert_eq!(settings, &SensorSettings::default());
    }

    #[test]
    fn only_known_rejects_unlisted_sensors() {
        let config = load(
            r#"
            [general]
            only_known = true

            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            "#,
        )
        .expect("config should parse");

        assert!(config.accepts_address(living_room()));
        assert!(!config.accepts_address(unknown()));
    }

    #[test]
    fn min_rssi_rejects_weak_advertisements_but_not_missing_ones() {
        let config = load(
            r#"
            [general]
            min_rssi = -90
            "#,
        )
        .expect("config should parse");

        assert!(config.accepts_rssi(Some(-60)));
        assert!(config.accepts_rssi(Some(-90)), "the boundary is inclusive");
        assert!(!config.accepts_rssi(Some(-91)));
        assert!(
            config.accepts_rssi(None),
            "an unknown RSSI is not a weak one"
        );
    }

    #[test]
    fn command_line_overrides_beat_the_file() {
        let mut config = load(
            r#"
            [general]
            only_known = false
            min_rssi = -100
            "#,
        )
        .expect("config should parse");

        config.set_only_known();
        config.set_min_rssi(-70);

        assert!(!config.accepts_address(unknown()));
        assert!(!config.accepts_rssi(Some(-80)));
    }

    #[test]
    fn calibration_offsets_are_added_to_the_reading() {
        let settings = SensorSettings {
            name: None,
            temperature_offset: -0.3,
            humidity_offset: 1.5,
        };
        let mut data = SensorData {
            temperature: Some(22.90),
            humidity: Some(64.25),
            battery: Some(16),
            ..Default::default()
        };

        settings.calibrate(&mut data);

        assert!((data.temperature.unwrap() - 22.60).abs() < 0.0005);
        assert!((data.humidity.unwrap() - 65.75).abs() < 0.0005);
        assert_eq!(
            data.battery,
            Some(16),
            "only offsets that exist are applied"
        );
    }

    #[test]
    fn calibration_leaves_absent_measurements_absent() {
        let settings = SensorSettings {
            name: None,
            temperature_offset: -0.3,
            humidity_offset: 1.5,
        };
        let mut data = SensorData::default();

        settings.calibrate(&mut data);

        assert_eq!(data.temperature, None);
        assert_eq!(data.humidity, None);
    }

    #[test]
    fn a_malformed_mac_is_rejected() {
        for mac in [
            "A4:C1:38:00:11",
            "A4:C1:38:00:11:22:33",
            "A4C13800 1122",
            "A4:C1:38:00:11:2",
            "A4:C1:38:00:11:ZZ",
            "",
        ] {
            let toml = format!("[[sensor]]\nmac = \"{mac}\"\n");
            assert!(
                matches!(load(&toml), Err(ConfigError::InvalidAddress(_))),
                "{mac:?} should have been rejected"
            );
        }
    }

    #[test]
    fn the_same_sensor_twice_is_rejected() {
        let result = load(
            r#"
            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            name = "First"

            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            name = "Second"
            "#,
        );

        assert!(matches!(result, Err(ConfigError::DuplicateSensor(_))));
    }

    /// A typo in a key should be reported rather than silently ignored.
    #[test]
    fn an_unknown_key_is_rejected() {
        let result = load(
            r#"
            [[sensor]]
            mac = "A4:C1:38:00:11:22"
            temp_offset = -0.3
            "#,
        );

        assert!(matches!(result, Err(ConfigError::Parse { .. })));
    }

    #[test]
    fn a_missing_file_is_reported_with_its_path() {
        let result = Config::load(Path::new("/nonexistent/mitempr.toml"));

        let error = result.expect_err("a missing file is an error");
        assert!(
            error.to_string().contains("/nonexistent/mitempr.toml"),
            "{error}"
        );
    }
}
