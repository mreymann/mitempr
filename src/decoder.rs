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
fn get_packet_type(service_data: &HashMap<Uuid, Vec<u8>>) -> (BlePacketType, Option<&Vec<u8>>) {
    if let Some(data) = service_data.get(&MIJIA_SERVICE_UUID) {
        return (BlePacketType::Mijia, Some(data));
    }
    if let Some(data) = service_data.get(&BTHOME_SERVICE_UUID) {
        return (BlePacketType::BTHome, Some(data));
    }
    if let Some(data) = service_data.get(&PVVX_SERVICE_UUID) {
        return (BlePacketType::Pvvx, Some(data));
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
                        println!("  ⚠️  Could not decode Mijia payload: {}", e);
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
fn decode_bthome(payload: &Vec<u8>) -> Option<SensorData> {
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
fn decode_pvvx(payload: &Vec<u8>) -> Option<SensorData> {
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
fn decode_mijia(payload: &Vec<u8>) -> Result<SensorData, String> {
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
                "Unrecognized or incomplete LYWSDCGQ V3 payload (Type 0x{:02X}, Length {})",
                type_identifier,
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

    #[test]
    fn test_mijia_service_data() {
        let mut data = HashMap::new();
        data.insert(
            uuid!("0000fe95-0000-1000-8000-00805f9b34fb"),
            vec![
                0x50, 0x20, 0xAA, 0x01, 0xF5, 0x40, 0x71, 0xD5, 0xA8, 0x65, 0x4C, 0x0D, 0x10, 0x04,
                0xEA, 0x00, 0x61, 0x02,
            ],
        );

        handle_service_data(&data);
    }

    #[test]
    fn test_pvvx_service_data() {
        let mut data = HashMap::new();
        data.insert(
            uuid!("0000181A-0000-1000-8000-00805F9B34FB"),
            vec![
                0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4, 0xF2, 0x08, 0x19, 0x19, 0x1D, 0x09, 0x10, 0x4A,
                0x05,
            ],
        );

        handle_service_data(&data);
    }

    #[test]
    fn test_bthome_service_data() {
        let mut data = HashMap::new();
        data.insert(
            uuid!("0000fcd2-0000-1000-8000-00805f9b34fb"),
            vec![
                0x40, 0x00, 0x12, 0x01, 0x64, 0x02, 0x7D, 0x09, 0x03, 0x8D, 0x18,
            ],
        );

        handle_service_data(&data);
    }
}
