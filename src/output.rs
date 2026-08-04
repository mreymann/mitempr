//! Turning a decoded advertisement into something outside this process can use.

use crate::decoder::SensorData;
use bluer::Address;
use serde::Serialize;
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// How readings are written.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    /// One human-readable line per reading, through the logger, so it picks up
    /// the same timestamp and level filtering as everything else.
    #[default]
    Text,
    /// One JSON object per line, written straight to stdout so it can be piped
    /// into `jq` and friends without log decoration getting in the way.
    Json,
}

/// A decoded reading plus where and when it came from.
///
/// Field names carry their unit so that a consumer of the JSON does not have to
/// guess. Absent measurements are omitted rather than serialised as `null`.
#[derive(Debug, Serialize)]
pub struct Reading {
    /// Seconds since the Unix epoch.
    pub timestamp: u64,
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub format: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rssi_dbm: Option<i16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature_celsius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub humidity_percent: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery_percent: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voltage_volts: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressure_hpa: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub illuminance_lux: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub moisture_percent: Option<f32>,
}

impl Reading {
    pub fn new(
        address: Address,
        name: Option<String>,
        rssi: Option<i16>,
        data: &SensorData,
    ) -> Self {
        Self {
            timestamp: unix_timestamp(),
            address: address.to_string(),
            name,
            format: data.format.as_str(),
            rssi_dbm: rssi,
            temperature_celsius: data.temperature,
            humidity_percent: data.humidity,
            battery_percent: data.battery,
            voltage_volts: data.voltage,
            pressure_hpa: data.pressure,
            illuminance_lux: data.illuminance,
            moisture_percent: data.moisture,
        }
    }

    /// Write this reading in the requested format.
    pub fn emit(&self, format: Format) {
        match format {
            Format::Text => log::info!("{self}"),
            Format::Json => match serde_json::to_string(self) {
                Ok(line) => println!("{line}"),
                Err(e) => log::error!("could not serialise reading: {e}"),
            },
        }
    }
}

/// Seconds since the Unix epoch, or 0 if the clock is set before 1970.
fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl fmt::Display for Reading {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{}]", self.address, self.format)?;

        if let Some(name) = &self.name {
            write!(f, " {name}")?;
        }
        if let Some(rssi) = self.rssi_dbm {
            write!(f, " {rssi} dBm")?;
        }

        write!(f, " |")?;
        if let Some(value) = self.temperature_celsius {
            write!(f, " {value:.2} C")?;
        }
        if let Some(value) = self.humidity_percent {
            write!(f, " {value:.2} %RH")?;
        }
        if let Some(value) = self.pressure_hpa {
            write!(f, " {value:.2} hPa")?;
        }
        if let Some(value) = self.illuminance_lux {
            write!(f, " {value:.0} lx")?;
        }
        if let Some(value) = self.moisture_percent {
            write!(f, " {value:.0} % moisture")?;
        }
        if let Some(value) = self.voltage_volts {
            write!(f, " {value:.3} V")?;
        }
        if let Some(value) = self.battery_percent {
            write!(f, " {value} % battery")?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::SensorFormat;

    fn sample() -> SensorData {
        SensorData {
            format: SensorFormat::Pvvx,
            temperature: Some(22.90),
            humidity: Some(64.25),
            battery: Some(16),
            voltage: Some(2.333),
            ..Default::default()
        }
    }

    fn reading() -> Reading {
        let mut reading = Reading::new(
            Address::new([0xA4, 0xC1, 0x38, 0xA0, 0x7B, 0x03]),
            Some("LYWSD03MMC".to_string()),
            Some(-67),
            &sample(),
        );
        // Pin the clock so the expectations below are stable.
        reading.timestamp = 1_770_000_000;
        reading
    }

    #[test]
    fn text_output_lists_every_present_measurement() {
        assert_eq!(
            reading().to_string(),
            "A4:C1:38:A0:7B:03 [pvvx] LYWSD03MMC -67 dBm | 22.90 C 64.25 %RH 2.333 V 16 % battery"
        );
    }

    #[test]
    fn json_output_is_one_object_with_units_in_the_keys() {
        let json = serde_json::to_string(&reading()).expect("reading should serialise");

        assert_eq!(
            json,
            concat!(
                r#"{"timestamp":1770000000,"address":"A4:C1:38:A0:7B:03","#,
                r#""name":"LYWSD03MMC","format":"pvvx","rssi_dbm":-67,"#,
                r#""temperature_celsius":22.9,"humidity_percent":64.25,"#,
                r#""battery_percent":16,"voltage_volts":2.333}"#
            )
        );
    }

    /// Measurements a sensor does not report must be left out entirely rather
    /// than serialised as null, so consumers can tell "not measured" from
    /// "measured as zero".
    #[test]
    fn json_output_omits_absent_measurements() {
        let data = SensorData {
            format: SensorFormat::BtHome,
            temperature: Some(24.29),
            ..Default::default()
        };
        let mut reading = Reading::new(Address::new([0; 6]), None, None, &data);
        reading.timestamp = 0;

        let json = serde_json::to_string(&reading).expect("reading should serialise");

        assert!(json.contains(r#""temperature_celsius":24.29"#));
        assert!(!json.contains("humidity"), "{json}");
        assert!(!json.contains("name"), "{json}");
        assert!(!json.contains("rssi"), "{json}");
        assert!(!json.contains("null"), "{json}");
    }
}
