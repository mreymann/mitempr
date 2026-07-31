use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug)]
pub enum BlePacketType {
    Mijia,  // 0xFE95
    BTHome, // 0xFCD2
    Pvvx,   // 0x181A
    Other,
}

// --- SensorData Struct (from your working code) ---
#[derive(Debug, Default)]
pub struct SensorData {
    pub temperature: Option<f32>,
    pub humidity: Option<f32>,
    pub battery: Option<u8>,
    pub voltage: Option<f32>,
    /// Pressure in hPa.
    pub pressure: Option<f32>,
    /// Illuminance in lux.
    pub illuminance: Option<f32>,
    /// Moisture in percent.
    pub moisture: Option<f32>,
}

impl SensorData {
    /// True when nothing in the advertisement could be turned into a reading.
    fn is_empty(&self) -> bool {
        self.temperature.is_none()
            && self.humidity.is_none()
            && self.battery.is_none()
            && self.voltage.is_none()
            && self.pressure.is_none()
            && self.illuminance.is_none()
            && self.moisture.is_none()
    }
}

// --- Constants ---
// Define the custom UUIDs used by Xiaomi/BTHome/PVVX devices
const MIJIA_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FE95_0000_1000_8000_00805F9B34FB);
const BTHOME_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FCD2_0000_1000_8000_00805F9B34FB);
const PVVX_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181A_0000_1000_8000_00805F9B34FB);

// Function to check the Service Data keys and return the classification
fn get_packet_type(service_data: &HashMap<Uuid, Vec<u8>>) -> (BlePacketType, Option<&[u8]>) {
    if let Some(data) = service_data.get(&MIJIA_SERVICE_UUID) {
        return (BlePacketType::Mijia, Some(data.as_slice()));
    }
    if let Some(data) = service_data.get(&BTHOME_SERVICE_UUID) {
        return (BlePacketType::BTHome, Some(data.as_slice()));
    }
    if let Some(data) = service_data.get(&PVVX_SERVICE_UUID) {
        return (BlePacketType::Pvvx, Some(data.as_slice()));
    }
    (BlePacketType::Other, None)
}

/// Decode or print service data from BLE advertisements.
///
/// This function is intentionally crate-agnostic: it doesn't depend on `bluer`
/// or any Bluetooth stack, only on standard Rust types.
pub fn handle_service_data(data: &HashMap<Uuid, Vec<u8>>) -> Option<SensorData> {
    let (packet_type, payload) = get_packet_type(data);

    match packet_type {
        BlePacketType::Mijia => {
            if let Some(bytes) = payload {
                match decode_mijia(bytes) {
                    Ok(decoded) => {
                        //println!("  🔍 Decoded Mijia data: {:?}", decoded);
                        return Some(decoded);
                    }
                    Err(e) => {
                        println!("  ⚠️  Could not decode Mijia payload: {e}");
                    }
                }
            }
        }

        BlePacketType::BTHome => {
            if let Some(bytes) = payload {
                if let Some(decoded) = decode_bthome(bytes) {
                    //println!("  🔍 Decoded BTHome data: {:?}", decoded);
                    return Some(decoded);
                } else {
                    println!("  ⚠️  Could not decode BTHome payload");
                }
            }
        }

        BlePacketType::Pvvx => {
            if let Some(bytes) = payload {
                if let Some(decoded) = decode_pvvx(bytes) {
                    //println!("  🔍 Decoded PVVX data: {:?}", decoded);
                    return Some(decoded);
                } else {
                    println!("  ⚠️  Could not decode PVVX payload");
                }
            }
        }

        BlePacketType::Other => {
            // Every phone, watch and TV in range lands here. Now that each
            // advertisement is processed rather than only the first one per
            // device, saying so on every packet drowns out the readings.
        }
    }

    None
}

// --- BTHome Decoder ---

/// The BTHome v2 device-information byte, i.e. the first byte of the service
/// data. See <https://bthome.io/format/>.
struct BthomeDeviceInfo {
    /// Bits 5-7. This decoder only understands version 2.
    version: u8,
    /// Bit 0. When set, everything after this byte is ciphertext.
    encrypted: bool,
}

impl BthomeDeviceInfo {
    fn parse(byte: u8) -> Self {
        Self {
            version: (byte >> 5) & 0x07,
            encrypted: byte & 0x01 != 0,
        }
    }
}

/// Number of value bytes that follow a BTHome object id, or `None` if the
/// length cannot be determined.
///
/// This is what keeps the parser in sync: an object we do not interpret still
/// has to be skipped by exactly the right number of bytes, otherwise the next
/// value byte gets read as an object id. Lengths are from
/// <https://bthome.io/format/>. `tail` is the payload after the object id and
/// is only needed for the few variable-length objects.
fn bthome_value_len(object_id: u8, tail: &[u8]) -> Option<usize> {
    let len = match object_id {
        // 1-byte sensor values: packet id, battery, count, humidity, moisture,
        // UV index, temperature, channel, light level, settings revision.
        0x00 | 0x01 | 0x09 | 0x2E | 0x2F | 0x46 | 0x57 | 0x58 | 0x59 | 0x60 | 0x64 | 0x65 => 1,
        // 2-byte sensor values.
        0x02 | 0x03 | 0x06 | 0x07 | 0x08 | 0x0C | 0x0D | 0x0E | 0x12 | 0x13 | 0x14 | 0x40
        | 0x41 | 0x43 | 0x44 | 0x45 | 0x47 | 0x48 | 0x49 | 0x4A | 0x51 | 0x52 | 0x56 | 0x5A
        | 0x5D | 0x5E | 0x5F | 0x61 => 2,
        // 3-byte sensor values: pressure, illuminance, energy, power, duration, gas.
        0x04 | 0x05 | 0x0A | 0x0B | 0x42 | 0x4B => 3,
        // 4-byte sensor values.
        0x4C | 0x4D | 0x4E | 0x4F | 0x50 | 0x55 | 0x5B | 0x5C | 0x62 | 0x63 => 4,
        // Binary sensors and the button event are one byte each.
        0x0F..=0x11 | 0x15..=0x2D | 0x3A => 1,
        // Command and dimmer events carry a step count for some event types.
        0x3B => match *tail.first()? {
            0x03 | 0x04 => 2,
            _ => 1,
        },
        0x3C => match *tail.first()? {
            0x01 | 0x02 => 2,
            _ => 1,
        },
        // Text and raw are length-prefixed.
        0x53 | 0x54 => 1 + usize::from(*tail.first()?),
        _ => return None,
    };
    Some(len)
}

/// Read a 24-bit little-endian unsigned value.
fn u24_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0])
}

fn decode_bthome(payload: &[u8]) -> Option<SensorData> {
    let (&info_byte, mut rest) = payload.split_first()?;
    let info = BthomeDeviceInfo::parse(info_byte);

    if info.version != 2 {
        println!(
            "  [!] BTHome v{} is not supported (device info 0x{info_byte:02X})",
            info.version
        );
        return None;
    }

    if info.encrypted {
        // Reading the ciphertext as if it were objects yields plausible-looking
        // nonsense, so refuse instead of inventing measurements.
        println!("  [!] BTHome payload is encrypted and no bind key is configured");
        return None;
    }

    let mut result = SensorData::default();

    while let Some((&object_id, tail)) = rest.split_first() {
        let Some(len) = bthome_value_len(object_id, tail) else {
            println!(
                "  [!] Unknown BTHome object id 0x{object_id:02X}; stopping here rather than \
                 guessing its length"
            );
            break;
        };

        if tail.len() < len {
            println!("  [!] Truncated BTHome object 0x{object_id:02X}");
            break;
        }

        let (value, remainder) = tail.split_at(len);
        rest = remainder;

        match object_id {
            0x01 => result.battery = Some(value[0]),
            0x02 => {
                result.temperature =
                    Some(f32::from(i16::from_le_bytes([value[0], value[1]])) / 100.0);
            }
            0x45 => {
                result.temperature =
                    Some(f32::from(i16::from_le_bytes([value[0], value[1]])) / 10.0);
            }
            0x03 => {
                result.humidity = Some(f32::from(u16::from_le_bytes([value[0], value[1]])) / 100.0);
            }
            0x2E => result.humidity = Some(f32::from(value[0])),
            0x0C => {
                result.voltage = Some(f32::from(u16::from_le_bytes([value[0], value[1]])) / 1000.0);
            }
            0x04 => result.pressure = Some(u24_le(value) as f32 / 100.0),
            0x05 => result.illuminance = Some(u24_le(value) as f32 / 100.0),
            0x14 => {
                result.moisture = Some(f32::from(u16::from_le_bytes([value[0], value[1]])) / 100.0);
            }
            0x2F => result.moisture = Some(f32::from(value[0])),
            // Skipped by exactly the right length, just not reported (yet):
            // packet id, binary sensors, events, power meters, air quality, ...
            _ => {}
        }
    }

    if result.is_empty() {
        return None;
    }

    Some(result)
}

// --- PVVX Decoder ---
fn decode_pvvx(payload: &[u8]) -> Option<SensorData> {
    const MIN_LENGTH: usize = 15;
    const MAC_LENGTH: usize = 6;

    if payload.len() < MIN_LENGTH {
        // Packet too short → return None
        return None;
    }

    // Slice out the data after the MAC address
    let data_slice = &payload[MAC_LENGTH..];

    // Temperature: Bytes 0 & 1 (Little-Endian, signed, factor 0.01)
    let temperature = if data_slice.len() >= 2 {
        let temp_raw = i16::from_le_bytes([data_slice[0], data_slice[1]]);
        Some(temp_raw as f32 / 100.0)
    } else {
        None
    };

    // Humidity: Bytes 2 & 3 (Little-Endian, unsigned, factor 0.01)
    let humidity = if data_slice.len() >= 4 {
        let hum_raw = u16::from_le_bytes([data_slice[2], data_slice[3]]);
        Some(hum_raw as f32 / 100.0)
    } else {
        None
    };

    // Voltage: Bytes 4 & 5 (Little-Endian, unsigned, factor 0.001)
    let voltage = if data_slice.len() >= 6 {
        let volt_raw = u16::from_le_bytes([data_slice[4], data_slice[5]]);
        Some(volt_raw as f32 / 1000.0)
    } else {
        None
    };

    // Battery: Byte 6
    let battery = if data_slice.len() >= 7 {
        Some(data_slice[6])
    } else {
        None
    };

    Some(SensorData {
        temperature,
        humidity,
        battery,
        voltage,
        ..Default::default()
    })
}

// --- Xiaomi MiBeacon Decoder (LYWSDCGQ and friends) ---

/// The MiBeacon frame-control field: the first two bytes of the service data,
/// little-endian.
///
/// Only the bits that affect parsing are kept. Bit numbering follows the
/// MiBeacon specification, cross-checked against
/// <https://github.com/Bluetooth-Devices/xiaomi-ble>.
struct MibeaconFrameControl {
    /// Bit 3: the data objects are AES-CCM encrypted.
    encrypted: bool,
    /// Bit 4: the sender's MAC address is part of the frame.
    mac_included: bool,
    /// Bit 5: a capability byte follows the MAC.
    capability_included: bool,
    /// Bit 6: the frame carries at least one data object.
    object_included: bool,
}

impl MibeaconFrameControl {
    fn parse(bytes: [u8; 2]) -> Self {
        let bits = u16::from_le_bytes(bytes);
        Self {
            encrypted: bits & (1 << 3) != 0,
            mac_included: bits & (1 << 4) != 0,
            capability_included: bits & (1 << 5) != 0,
            object_included: bits & (1 << 6) != 0,
        }
    }
}

/// Frame control (2) + product id (2) + frame counter (1).
const MIBEACON_HEADER_LEN: usize = 5;
const MIBEACON_MAC_LEN: usize = 6;
/// Bit 5 of the capability byte announces an extra IO-capability byte.
const MIBEACON_CAPABILITY_IO: u8 = 0x20;

fn decode_mijia(payload: &[u8]) -> Result<SensorData, String> {
    if payload.len() < MIBEACON_HEADER_LEN {
        return Err(format!("MiBeacon frame too short: {} bytes", payload.len()));
    }

    let frame_control = MibeaconFrameControl::parse([payload[0], payload[1]]);

    if frame_control.encrypted {
        // The objects are ciphertext. Parsing them as plaintext produces
        // believable-looking rubbish, so refuse rather than invent readings.
        return Err("frame is encrypted and no bind key is configured".to_string());
    }

    if !frame_control.object_included {
        return Err("frame carries no data object".to_string());
    }

    // The header is followed by an optional MAC address and an optional
    // capability byte, so the offset of the first data object depends on the
    // frame-control bits rather than being a fixed constant.
    let mut offset = MIBEACON_HEADER_LEN;

    if frame_control.mac_included {
        offset += MIBEACON_MAC_LEN;
    }

    if frame_control.capability_included {
        let capability = *payload
            .get(offset)
            .ok_or("frame ends before the capability byte")?;
        offset += 1;
        if capability & MIBEACON_CAPABILITY_IO != 0 {
            offset += 1;
        }
    }

    let mut objects = payload
        .get(offset..)
        .ok_or("frame ends before the first data object")?;

    let mut result = SensorData::default();

    // Each object is a little-endian u16 id, a length byte, then that many value
    // bytes. Because the length is explicit, an object this decoder does not
    // know about can be skipped without losing the ones after it.
    while objects.len() >= 3 {
        let event_id = u16::from_le_bytes([objects[0], objects[1]]);
        let value_len = usize::from(objects[2]);

        let Some(value) = objects.get(3..3 + value_len) else {
            return Err(format!(
                "truncated MiBeacon object 0x{event_id:04X}: {value_len} value bytes announced, \
                 {} available",
                objects.len() - 3
            ));
        };
        objects = &objects[3 + value_len..];

        match (event_id, value) {
            // Temperature, signed, factor 0.1
            (0x1004, &[lo, hi]) => {
                result.temperature = Some(f32::from(i16::from_le_bytes([lo, hi])) / 10.0);
            }
            // Humidity, factor 0.1
            (0x1006, &[lo, hi]) => {
                result.humidity = Some(f32::from(u16::from_le_bytes([lo, hi])) / 10.0);
            }
            // Illuminance in lux
            (0x1007, &[a, b, c]) => {
                result.illuminance = Some(u24_le(&[a, b, c]) as f32);
            }
            // Moisture in percent
            (0x1008, &[percent]) => {
                result.moisture = Some(f32::from(percent));
            }
            // Battery in percent
            (0x100A, &[percent]) => {
                result.battery = Some(percent);
            }
            // Temperature and humidity in one object
            (0x100D, &[t_lo, t_hi, h_lo, h_hi]) => {
                result.temperature = Some(f32::from(i16::from_le_bytes([t_lo, t_hi])) / 10.0);
                result.humidity = Some(f32::from(u16::from_le_bytes([h_lo, h_hi])) / 10.0);
            }
            // Skipped by its announced length: door/lock/button events, remaining
            // battery of consumables, formaldehyde, ...
            _ => {}
        }
    }

    if result.is_empty() {
        return Err("no known MiBeacon object in frame".to_string());
    }

    Ok(result)
}

// Unit tests for the decoder module
#[cfg(test)]
mod tests {
    use super::*;
    use uuid::uuid;

    const MIJIA_UUID: Uuid = uuid!("0000fe95-0000-1000-8000-00805f9b34fb");
    const BTHOME_UUID: Uuid = uuid!("0000fcd2-0000-1000-8000-00805f9b34fb");
    const PVVX_UUID: Uuid = uuid!("0000181a-0000-1000-8000-00805f9b34fb");

    /// Wrap a single service-data payload the way BlueZ hands it to us.
    fn advertisement(uuid: Uuid, payload: &[u8]) -> HashMap<Uuid, Vec<u8>> {
        let mut map = HashMap::new();
        map.insert(uuid, payload.to_vec());
        map
    }

    /// Compare a decoded measurement against its expected value. The decoders
    /// divide integers by 10/100/1000, so an exact `==` would be fragile.
    #[track_caller]
    fn assert_measurement(actual: Option<f32>, expected: f32) {
        let value = actual.expect("expected a measurement, got None");
        assert!(
            (value - expected).abs() < 0.0005,
            "expected {expected}, got {value}"
        );
    }

    // --- BTHome v2 ---

    /// Device info 0x40 (v2, unencrypted), packet id 0x12, battery 100 %,
    /// temperature 0x097D = 2429 -> 24.29 degC, humidity 0x188D = 6285 -> 62.85 %.
    #[test]
    fn bthome_decodes_battery_temperature_and_humidity() {
        let data = advertisement(
            BTHOME_UUID,
            &[
                0x40, 0x00, 0x12, 0x01, 0x64, 0x02, 0x7D, 0x09, 0x03, 0x8D, 0x18,
            ],
        );

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_eq!(reading.battery, Some(100));
        assert_measurement(reading.temperature, 24.29);
        assert_measurement(reading.humidity, 62.85);
        assert_eq!(reading.voltage, None);
    }

    /// Voltage object 0x0C: 0x0B9E = 2974 -> 2.974 V.
    #[test]
    fn bthome_decodes_voltage() {
        let data = advertisement(BTHOME_UUID, &[0x40, 0x00, 0x01, 0x0C, 0x9E, 0x0B]);

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.voltage, 2.974);
        assert_eq!(reading.temperature, None);
        assert_eq!(reading.humidity, None);
    }

    /// A three-byte object (illuminance, 0x05) followed by a temperature.
    ///
    /// Illuminance 0x0003E8 = 1000 -> 10.00 lux, then 0x097D = 2429 -> 24.29 degC.
    ///
    /// Before the object-length table, the unknown-object fallback advanced the
    /// cursor by a guessed two bytes and landed mid-value: byte 0x03 was mistaken
    /// for a humidity object, so the parser reported humidity 5.12 % and dropped
    /// the temperature entirely.
    #[test]
    fn bthome_three_byte_object_does_not_desync_the_parser() {
        let data = advertisement(
            BTHOME_UUID,
            &[0x40, 0x05, 0xE8, 0x03, 0x00, 0x02, 0x7D, 0x09],
        );

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.illuminance, 10.00);
        assert_measurement(reading.temperature, 24.29);
        assert_eq!(
            reading.humidity, None,
            "this packet carries no humidity object"
        );
    }

    /// Pressure 0x018BCD = 101325 -> 1013.25 hPa, the other three-byte object.
    #[test]
    fn bthome_decodes_pressure() {
        let data = advertisement(BTHOME_UUID, &[0x40, 0x04, 0xCD, 0x8B, 0x01]);

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.pressure, 1013.25);
    }

    /// The single-byte humidity (0x2E) and moisture (0x2F) objects.
    #[test]
    fn bthome_decodes_single_byte_humidity_and_moisture() {
        let data = advertisement(BTHOME_UUID, &[0x40, 0x2E, 0x33, 0x2F, 0x22]);

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.humidity, 51.0);
        assert_measurement(reading.moisture, 34.0);
    }

    /// Temperature object 0x45 uses factor 0.1: 0x00EA = 234 -> 23.4 degC.
    #[test]
    fn bthome_decodes_low_resolution_temperature() {
        let data = advertisement(BTHOME_UUID, &[0x40, 0x45, 0xEA, 0x00]);

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.temperature, 23.4);
    }

    /// Objects this tool does not report must still be skipped by the right
    /// length. Here a 2-byte CO2 object (0x12) sits between two objects we do
    /// report; getting its length wrong would lose the humidity.
    #[test]
    fn bthome_skips_unreported_objects_without_losing_the_rest() {
        let data = advertisement(
            BTHOME_UUID,
            &[0x40, 0x02, 0x7D, 0x09, 0x12, 0xE8, 0x03, 0x03, 0x8D, 0x18],
        );

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.temperature, 24.29);
        assert_measurement(reading.humidity, 62.85);
    }

    /// A genuinely unknown object id has no known length, so parsing stops there
    /// instead of guessing. Everything decoded before it is still reported.
    #[test]
    fn bthome_stops_at_an_unknown_object_id() {
        let data = advertisement(
            BTHOME_UUID,
            &[0x40, 0x02, 0x7D, 0x09, 0xF0, 0xAA, 0xBB, 0x03, 0x8D, 0x18],
        );

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.temperature, 24.29);
        assert_eq!(
            reading.humidity, None,
            "objects after an unknown id cannot be located reliably"
        );
    }

    /// An object whose value is cut off by the end of the advertisement is
    /// dropped rather than read out of bounds.
    #[test]
    fn bthome_ignores_a_truncated_object() {
        let data = advertisement(BTHOME_UUID, &[0x40, 0x02, 0x7D, 0x09, 0x03, 0x8D]);

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.temperature, 24.29);
        assert_eq!(reading.humidity, None);
    }

    /// Bits 5-7 of the device-info byte carry the version. 0x20 is v1, which
    /// this decoder does not implement, so it must not be parsed as v2.
    #[test]
    fn bthome_rejects_a_non_v2_payload() {
        let data = advertisement(BTHOME_UUID, &[0x20, 0x02, 0x7D, 0x09]);

        assert!(handle_service_data(&data).is_none());
    }

    /// A payload with nothing but a device-info byte carries no measurement.
    #[test]
    fn bthome_rejects_a_payload_without_objects() {
        let data = advertisement(BTHOME_UUID, &[0x40]);

        assert!(handle_service_data(&data).is_none());
    }

    /// Device info 0x41 sets bit 0, marking the payload as encrypted. Without a
    /// bind key nothing here can be decoded, and the ciphertext must not be
    /// parsed as if it were plaintext objects.
    ///
    /// Before the device-info byte was parsed it was skipped unread, so the
    /// first ciphertext bytes were happily interpreted as a temperature object.
    #[test]
    fn bthome_encrypted_payload_is_not_decoded_as_plaintext() {
        let data = advertisement(
            BTHOME_UUID,
            &[
                0x41, 0x02, 0x7D, 0x09, 0x03, 0x8D, 0x18, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77,
                0x88,
            ],
        );

        assert!(
            handle_service_data(&data).is_none(),
            "an encrypted payload must not yield a plaintext reading"
        );
    }

    // --- PVVX custom format ---

    /// MAC A4:C1:38:A0:7B:03, temperature 0x08F2 = 2290 -> 22.90 degC,
    /// humidity 0x1919 = 6425 -> 64.25 %, battery 0x091D = 2333 -> 2.333 V and
    /// 0x10 = 16 %.
    #[test]
    fn pvvx_decodes_temperature_humidity_voltage_and_battery() {
        let data = advertisement(
            PVVX_UUID,
            &[
                0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4, 0xF2, 0x08, 0x19, 0x19, 0x1D, 0x09, 0x10, 0x4A,
                0x05,
            ],
        );

        let reading = handle_service_data(&data).expect("PVVX payload should decode");
        assert_measurement(reading.temperature, 22.90);
        assert_measurement(reading.humidity, 64.25);
        assert_measurement(reading.voltage, 2.333);
        assert_eq!(reading.battery, Some(16));
    }

    /// Sub-zero temperatures are signed: 0xFF9C = -100 -> -1.00 degC.
    #[test]
    fn pvvx_decodes_negative_temperature() {
        let data = advertisement(
            PVVX_UUID,
            &[
                0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4, 0x9C, 0xFF, 0x19, 0x19, 0x1D, 0x09, 0x10, 0x4A,
                0x05,
            ],
        );

        let reading = handle_service_data(&data).expect("PVVX payload should decode");
        assert_measurement(reading.temperature, -1.00);
    }

    /// Anything below the 15-byte custom format is rejected rather than read
    /// out of bounds.
    #[test]
    fn pvvx_rejects_payload_shorter_than_the_custom_format() {
        let data = advertisement(
            PVVX_UUID,
            &[
                0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4, 0xF2, 0x08, 0x19, 0x19, 0x1D, 0x09, 0x10, 0x4A,
            ],
        );

        assert!(handle_service_data(&data).is_none());
    }

    // --- Xiaomi MiBeacon / LYWSDCGQ ---

    /// Frame control 0x2050 (MAC included, object included, unencrypted),
    /// product id 0x01AA (LYWSDCGQ), event 0x100D (temperature + humidity):
    /// 0x00EA = 234 -> 23.4 degC, 0x0261 = 609 -> 60.9 %.
    #[test]
    fn mijia_decodes_combined_temperature_and_humidity() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0D, 0x10, 0x04,
                0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
        assert_eq!(reading.battery, None);
        assert_eq!(reading.voltage, None);
    }

    /// Event 0x1004 carries only a temperature.
    #[test]
    fn mijia_decodes_temperature_only_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF7, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x04, 0x10, 0x02,
                0xEA, 0x00,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_eq!(reading.humidity, None);
    }

    /// Event 0x1006 carries only a humidity.
    #[test]
    fn mijia_decodes_humidity_only_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF8, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x06, 0x10, 0x02,
                0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.humidity, 60.9);
        assert_eq!(reading.temperature, None);
    }

    /// Event 0x100A carries the battery percentage: 0x5B = 91 %.
    #[test]
    fn mijia_decodes_battery_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF6, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0A, 0x10, 0x01,
                0x5B,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_eq!(reading.battery, Some(91));
        assert_eq!(reading.temperature, None);
        assert_eq!(reading.humidity, None);
    }

    /// A frame that stops right after the MAC has no event to decode.
    #[test]
    fn mijia_rejects_payload_without_an_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C,
            ],
        );

        assert!(handle_service_data(&data).is_none());
    }

    /// An event id this decoder does not know is reported as undecodable rather
    /// than guessed at.
    #[test]
    fn mijia_rejects_unknown_event_id() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x99, 0x10, 0x02,
                0x00, 0x00,
            ],
        );

        assert!(handle_service_data(&data).is_none());
    }

    /// Frame control 0x2070 additionally sets bit 5, so a capability byte (0x08,
    /// no IO capability) sits between the MAC and the event. The same
    /// 23.4 degC / 60.9 % reading as
    /// `mijia_decodes_combined_temperature_and_humidity`, shifted one byte.
    ///
    /// Before the offset was derived from the frame-control bits it was the
    /// constant 11, so the decoder read the capability byte as the event id and
    /// gave up.
    #[test]
    fn mijia_decodes_frame_with_capability_byte() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x70, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x08, 0x0D, 0x10,
                0x04, 0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
    }

    /// Frame control 0x2040 clears bit 4, so no MAC is included and the event
    /// starts six bytes earlier.
    #[test]
    fn mijia_decodes_frame_without_mac_address() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x40, 0x20, 0xAA, 0x01, 0xF5, 0x0D, 0x10, 0x04, 0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
    }

    /// Frame control 0x2058 sets bit 3, marking the event payload as encrypted.
    /// Without a bind key it must not be read as plaintext.
    #[test]
    fn mijia_encrypted_payload_is_not_decoded_as_plaintext() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x58, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0D, 0x10, 0x04,
                0xEA, 0x00, 0x61, 0x02,
            ],
        );

        assert!(
            handle_service_data(&data).is_none(),
            "an encrypted payload must not yield a plaintext reading"
        );
    }

    /// Capability byte 0x28 sets bit 5, which announces one further
    /// IO-capability byte before the first event.
    #[test]
    fn mijia_decodes_frame_with_io_capability() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x70, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x28, 0x00, 0x0D,
                0x10, 0x04, 0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
    }

    /// Frame control 0x2010 clears bit 6, so the frame carries no data object at
    /// all -- it is a bare advertisement, not a reading.
    #[test]
    fn mijia_rejects_frame_without_the_object_bit() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x10, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0D, 0x10, 0x04,
                0xEA, 0x00, 0x61, 0x02,
            ],
        );

        assert!(handle_service_data(&data).is_none());
    }

    /// A frame may carry several objects in a row. Here a battery event is
    /// followed by a combined temperature/humidity event.
    #[test]
    fn mijia_decodes_multiple_objects_in_one_frame() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0A, 0x10, 0x01,
                0x5B, 0x0D, 0x10, 0x04, 0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_eq!(reading.battery, Some(91));
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
    }

    /// An event this decoder does not report is skipped by its announced length,
    /// so the event after it is still found. Here an unknown 2-byte object sits
    /// in front of the temperature/humidity event.
    #[test]
    fn mijia_skips_unknown_objects_without_losing_the_rest() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x99, 0x10, 0x02,
                0xAA, 0xBB, 0x0D, 0x10, 0x04, 0xEA, 0x00, 0x61, 0x02,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.temperature, 23.4);
        assert_measurement(reading.humidity, 60.9);
    }

    /// An object that announces more value bytes than the frame contains is
    /// rejected rather than read out of bounds.
    #[test]
    fn mijia_rejects_a_truncated_object() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0D, 0x10, 0x04,
                0xEA, 0x00,
            ],
        );

        assert!(handle_service_data(&data).is_none());
    }

    /// Illuminance event 0x1007: 0x0001F4 = 500 lux.
    #[test]
    fn mijia_decodes_illuminance_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0x98, 0x00, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x07, 0x10, 0x03,
                0xF4, 0x01, 0x00,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.illuminance, 500.0);
    }

    /// Moisture event 0x1008 carries a percentage in a single byte.
    #[test]
    fn mijia_decodes_moisture_event() {
        let data = advertisement(
            MIJIA_UUID,
            &[
                0x50, 0x20, 0x98, 0x00, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x08, 0x10, 0x01,
                0x1C,
            ],
        );

        let reading = handle_service_data(&data).expect("MiBeacon payload should decode");
        assert_measurement(reading.moisture, 28.0);
    }

    // --- Dispatch ---

    /// Service data for something that isn't a supported sensor is ignored.
    #[test]
    fn unrelated_service_uuid_is_ignored() {
        let data = advertisement(
            uuid!("0000180f-0000-1000-8000-00805f9b34fb"),
            &[0x64, 0x00, 0x00],
        );

        assert!(handle_service_data(&data).is_none());
    }

    /// An advertisement with no service data at all is ignored.
    #[test]
    fn empty_service_data_is_ignored() {
        assert!(handle_service_data(&HashMap::new()).is_none());
    }
}
