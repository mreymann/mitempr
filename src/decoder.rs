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
#[derive(Debug)]
pub struct SensorData {
    pub temperature: Option<f32>,
    pub humidity: Option<f32>,
    pub battery: Option<u8>,
    pub voltage: Option<f32>,
}
// --- Constants ---
// Define the custom UUIDs used by Xiaomi/BTHome/PVVX devices
const MIJIA_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FE95_0000_1000_8000_00805F9B34FB);
const BTHOME_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FCD2_0000_1000_8000_00805F9B34FB);
const PVVX_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181A_0000_1000_8000_00805F9B34FB);
const BTHOME_V2_PREAMBLE: [u8; 4] = [0x16, 0xd2, 0xfc, 0x40];

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
            println!("  -> Unknown BLE packet");
        }
    }

    None
}

// --- BTHome Decoder ---
fn decode_bthome(payload: &[u8]) -> Option<SensorData> {
    // 1. Create the full data array by prepending the preamble
    let mut all_data = Vec::new();
    all_data.extend_from_slice(&BTHOME_V2_PREAMBLE);
    all_data.extend_from_slice(payload); // payload is the [40, 00, 73, 0C, ...]

    // 2. The working decoder expects the full array but is sliced to skip the first 4 bytes
    let data = &all_data[4..];

    let mut result = SensorData {
        temperature: None,
        humidity: None,
        battery: None,
        voltage: None,
    };

    let mut i = 1; // Skip first byte (00) - This is the Packet ID in the [40, 00] header
    while i < data.len() {
        if i + 1 >= data.len() {
            break;
        }

        match data[i] {
            0x01 => {
                // Battery (%) (1 byte)
                if i + 1 >= data.len() {
                    break;
                }
                result.battery = Some(data[i + 1]);
                i += 2;
            }
            0x02 => {
                // Temperature (2 bytes, factor 0.01)
                if i + 2 >= data.len() {
                    break;
                }
                let temp_raw = i16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.temperature = Some(temp_raw as f32 / 100.0);
                i += 3;
            }
            0x03 => {
                // Humidity (2 bytes, factor 0.01)
                if i + 2 >= data.len() {
                    break;
                }
                let hum_raw = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.humidity = Some(hum_raw as f32 / 100.0);
                i += 3;
            }
            0x0C => {
                // Voltage (2 bytes, factor 0.001)
                if i + 2 >= data.len() {
                    break;
                }
                let voltage_raw = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.voltage = Some(voltage_raw as f32 / 1000.0);
                i += 3;
            }
            _ => {
                //println!("  ⚠️  Unknown type 0x{:02x} at position {}", data[i], i);
                i += 2; // Try to skip an assumed Type + 1 byte value to continue
            }
        }
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
    })
}

// --- LYWSDCGQ V3 Decoder ---
fn decode_mijia(payload: &[u8]) -> Result<SensorData, String> {
    // The Xiaomi Manufacturer ID (0x04C0) is already stripped by bluer.
    // The byte at index 11 is the Type Identifier byte (0x0D, 0x06, 0x0A, etc.)
    const TYPE_IDENTIFIER_OFFSET: usize = 11;

    if payload.len() <= TYPE_IDENTIFIER_OFFSET {
        return Err(format!(
            "LYWSDCGQ V3 packet too short: {} bytes",
            payload.len()
        ));
    }

    let type_identifier = payload[TYPE_IDENTIFIER_OFFSET];

    // Initialize all fields as None
    let mut temperature: Option<f32> = None;
    let mut humidity: Option<f32> = None;
    let mut battery_percent: Option<u8> = None;
    let voltage: Option<f32> = None; // V3 typically doesn't send voltage

    match type_identifier {
        // 0x0D: Combined Temperature and Humidity
        0x0D if payload.len() >= 18 => {
            let raw_temp_bytes: [u8; 2] = payload[14..16].try_into().unwrap_or([0, 0]);
            temperature = Some(i16::from_le_bytes(raw_temp_bytes) as f32 / 10.0);

            let raw_humi_bytes: [u8; 2] = payload[16..18].try_into().unwrap_or([0, 0]);
            humidity = Some(u16::from_le_bytes(raw_humi_bytes) as f32 / 10.0);
        }

        // 0x04: Temperature Only
        0x04 if payload.len() >= 16 => {
            let raw_temp_bytes: [u8; 2] = payload[14..16].try_into().unwrap_or([0, 0]);
            temperature = Some(i16::from_le_bytes(raw_temp_bytes) as f32 / 10.0);
        }

        // 0x06: Humidity Only
        0x06 if payload.len() >= 16 => {
            let raw_humi_bytes: [u8; 2] = payload[14..16].try_into().unwrap_or([0, 0]);
            humidity = Some(u16::from_le_bytes(raw_humi_bytes) as f32 / 10.0);
        }

        // 0x0A: Battery Percentage Only
        0x0A if payload.len() >= 15 => {
            battery_percent = Some(payload[14]);
        }

        _ => {
            return Err(format!(
                "Unrecognized or incomplete LYWSDCGQ V3 payload (Type 0x{type_identifier:02X}, Length {})",
                payload.len()
            ));
        }
    }

    Ok(SensorData {
        temperature,
        humidity,
        battery: battery_percent,
        voltage,
    })
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
    /// Currently the unknown-object fallback advances the cursor by a guessed two
    /// bytes, so it lands mid-value: byte 0x03 is mistaken for a humidity object
    /// and the parser reports humidity 5.12 % while dropping the temperature
    /// entirely. Ignored until the object-length table lands.
    #[test]
    #[ignore = "known bug: unknown object IDs advance the cursor by a guessed 2 bytes, which desyncs the parser on 3-byte objects"]
    fn bthome_three_byte_object_does_not_desync_the_parser() {
        let data = advertisement(
            BTHOME_UUID,
            &[0x40, 0x05, 0xE8, 0x03, 0x00, 0x02, 0x7D, 0x09],
        );

        let reading = handle_service_data(&data).expect("BTHome payload should decode");
        assert_measurement(reading.temperature, 24.29);
        assert_eq!(
            reading.humidity, None,
            "this packet carries no humidity object"
        );
    }

    /// Device info 0x41 sets bit 0, marking the payload as encrypted. Without a
    /// bind key nothing here can be decoded, and the ciphertext must not be
    /// parsed as if it were plaintext objects.
    ///
    /// Currently the device-info byte is skipped unread, so the first ciphertext
    /// bytes are happily interpreted as a temperature object.
    #[test]
    #[ignore = "known bug: the device-info byte is never parsed, so the encryption flag is ignored"]
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
    /// Currently the event offset is the hardcoded constant 11, so the decoder
    /// reads the capability byte as the event id and gives up.
    #[test]
    #[ignore = "known bug: the event offset is hardcoded to 11 instead of derived from the frame-control bits"]
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
    #[ignore = "known bug: the event offset assumes the MAC-included frame-control bit is always set"]
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
    #[ignore = "known bug: the frame-control encryption bit is never checked"]
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
