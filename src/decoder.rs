use btleplug::api::PeripheralProperties;
use uuid::Uuid;
use anyhow::{anyhow, Result};
use std::convert::TryInto;
//use std::collections::HashMap;

#[derive(Debug)]
pub enum BlePacketType {
    Mijia,   // 0xFE95
    BTHome,  // 0xFCD2
    Pvvx,    // 0x181A
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
// Define the custom UUIDs used by Xiaomi/BTHome
const MIJIA_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FE95_0000_1000_8000_00805F9B34FB);
const BTHOME_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FCD2_0000_1000_8000_00805F9B34FB);
const PVVX_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181A_0000_1000_8000_00805F9B34FB);
const BTHOME_V2_PREAMBLE: [u8; 4] = [0x16, 0xd2, 0xfc, 0x40];

// --- Helper function (from your working code) ---
//fn print_hex(data: &[u8]) -> String {
//    data.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join("")
//}

// Function to check the Service Data keys and return the classification
fn get_packet_type<'a>(props: &'a PeripheralProperties) -> (BlePacketType, Option<&'a Vec<u8>>) {
    // 1. Check MIJIA/LYWSDCGQ (0xFE95)
    if let Some(data) = props.service_data.get(&MIJIA_SERVICE_UUID) {
        return (BlePacketType::Mijia, Some(data));
    } 
    // 2. Check BTHome (0xFCD2)
    if let Some(data) = props.service_data.get(&BTHOME_SERVICE_UUID) {
        return (BlePacketType::BTHome, Some(data));
    }
    // 3. Check PVVX (0x181A)
    if let Some(data) = props.service_data.get(&PVVX_SERVICE_UUID) {
        return (BlePacketType::Pvvx, Some(data));
    }
    // Default to Other
    (BlePacketType::Other, None)
}

// --- Public Decoding Function ---
pub fn classify_and_decode(props: &PeripheralProperties) {
    println!("  🏭 Service Data: {:02X?}", props.service_data);

    let (packet_type, data_option) = get_packet_type(props);

    match packet_type {
        BlePacketType::Mijia => {
            println!("  ✅ Detected: MIJIA/LYWSDCGQ Packet");
            // We know `data_option` is `Some` here.
            handle_lywsdcgq_packet(data_option.unwrap());
        }
        BlePacketType::BTHome => {
            println!("  ✅ Detected: BTHome");
            // We know `data_option` is `Some` here.
            handle_bthome_packet(data_option.unwrap());
        }
        BlePacketType::Pvvx => {
            println!("  ✅ Detected: PVVX Packet");
            // We know `data_option` is `Some` here.
            handle_pvvx_packet(data_option.unwrap());
            output_sensor_data(&handle_pvvx_packet(data_option.unwrap()).unwrap());
        }
        BlePacketType::Other => {
            println!("  ⚠️ Detected: Other/Unclassified BLE Packet (Ignoring)");
            // Optional: print manufacturer data for debugging other devices
            for (id, data) in &props.manufacturer_data {
                println!("    🏭 Manufacturer ID: 0x{:04X}, data: {:02X?}", id, data);
            }
        }
    }
}

// --- Private Handler Functions (stubs for implementation) ---
fn handle_lywsdcgq_packet(data: &Vec<u8>) {
    // TODO: Implement your Lywsdcgq decoding logic here
    println!("    Payload ({} bytes): {:02X?}", data.len(), data);
    // e.g., print_lywsdcgq_data(&data);
}



fn handle_pvvx_packet(data: &Vec<u8>) -> Result<SensorData> {
    // Expected minimal length: 6 bytes MAC + 2 bytes Temp + 2 bytes Humi + 2 bytes Volt + 1 byte Batt% + 2 bytes Counter = 15 bytes
    const MIN_LENGTH: usize = 15;
    const MAC_LENGTH: usize = 6;
    
    if data.len() < MIN_LENGTH {
        return Err(anyhow!("PVVX packet too short. Expected at least {} bytes, got {}", MIN_LENGTH, data.len()));
    }
    
    // 1. MAC Check (Optional but good for validation)
    // The first 6 bytes of the payload should be the device's MAC address, reversed.
    // We can skip explicit MAC check for simplicity, as btleplug/bluez handles the matching.

    // 2. Extract Data (Starts after the 6-byte MAC address)
    let data_slice = &data[MAC_LENGTH..];
    
    // Temperature: Bytes 0 & 1 of the data_slice (data[6] and data[7])
    // Format: Little-Endian, Signed, 0.01°C
    let raw_temp_bytes: [u8; 2] = data_slice[0..2].try_into().unwrap_or_default();
    let raw_temp = i16::from_le_bytes(raw_temp_bytes);
    let temperature = raw_temp as f32 / 100.0;
    
    // Humidity: Bytes 2 & 3 of the data_slice (data[8] and data[9])
    // Format: Little-Endian, Unsigned, 0.01%
    let raw_humi_bytes: [u8; 2] = data_slice[2..4].try_into().unwrap_or_default();
    let raw_humi = u16::from_le_bytes(raw_humi_bytes);
    let humidity = raw_humi as f32 / 100.0;

    // Battery Voltage: Bytes 4 & 5 of the data_slice (data[10] and data[11])
    // Format: Little-Endian, Unsigned, mV
    let raw_volt_bytes: [u8; 2] = data_slice[4..6].try_into().unwrap_or_default();
    let raw_volt = u16::from_le_bytes(raw_volt_bytes);
    let voltage = raw_volt as f32 / 1000.0;

    // Battery Percent: Byte 6 of the data_slice (data[12])
    let battery = data_slice[6];

    // The counter (bytes 7 & 8) is often used to suppress repeated packets,
    // but we let the calling code handle deduplication based on the full packet content
    // or simply process all unique advertisements.

    Ok(SensorData {
        //mac_address: mac.to_string(),
        temperature: Some(temperature),
        humidity: Some(humidity),
        battery: Some(battery),
        voltage: Some(voltage),
    })
}

// --- BTHome Decoder (Moved from your working code) ---
// Note: This logic uses the custom Type IDs (0x01=Battery, 0x02=Temp, 0x03=Hum, 0x0C=Volt)
fn decode_bthome_v2(data: &[u8]) -> Option<SensorData> {
    // ... (copy the entire decode_bthome_v2 function body here)
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
            0x01 => { // Battery (%) (1 byte)
                if i + 1 >= data.len() { break; }
                result.battery = Some(data[i + 1]);
                i += 2;
            },
            0x02 => { // Temperature (2 bytes, factor 0.01)
                if i + 2 >= data.len() { break; }
                let temp_raw = i16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.temperature = Some(temp_raw as f32 / 100.0);
                i += 3;
            },
            0x03 => { // Humidity (2 bytes, factor 0.01)
                if i + 2 >= data.len() { break; }
                let hum_raw = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.humidity = Some(hum_raw as f32 / 100.0);
                i += 3;
            },
            0x0C => { // Voltage (2 bytes, factor 0.001)
                if i + 2 >= data.len() { break; }
                let voltage_raw = u16::from_le_bytes([data[i + 1], data[i + 2]]);
                result.voltage = Some(voltage_raw as f32 / 1000.0);
                i += 3;
            },
            _ => {
                //println!("  ⚠️  Unknown type 0x{:02x} at position {}", data[i], i);
                i += 2; // Try to skip an assumed Type + 1 byte value to continue
            }
        }
    }

    Some(result)
}

// --- The Dispatch Handler ---
fn handle_bthome_packet(payload: &Vec<u8>) {
    // 1. Create the full data array by prepending the preamble
    let mut all_data = Vec::new();
    all_data.extend_from_slice(&BTHOME_V2_PREAMBLE);
    all_data.extend_from_slice(payload); // payload is the [40, 00, 73, 0C, ...]

    // 2. The working decoder expects the full array but is sliced to skip the first 4 bytes
    let sliced_payload = &all_data[4..];
    
    // We don't have access to the address or RSSI here, so we print the essential data:
    //println!("    >>> full data: {}", print_hex(&all_data));
    //println!("    >>> stripped payload: {}", print_hex(sliced_payload));

    if let Some(sensor_data) = decode_bthome_v2(sliced_payload) {
        println!("    Decoded BTHome Data:");
        
        if let Some(temp) = sensor_data.temperature {
            println!("    Temperature:  {:.2} C", temp);
        }
        if let Some(hum) = sensor_data.humidity {
            println!("    Humidity:  {:.2} %", hum);
        }
        if let Some(volt) = sensor_data.voltage {
            println!("    Battery voltage: {:.3} V", volt); // Use 3 decimal places for voltage
        }
        if let Some(batt) = sensor_data.battery {
            println!("    Battery: {}%", batt);
        }
    }
}

fn output_sensor_data(sensor_data: &SensorData) {
    if let Some(temp) = sensor_data.temperature {
        println!("    Temperature:  {:.2} C", temp);
    }
    if let Some(hum) = sensor_data.humidity {
        println!("    Humidity:  {:.2} %", hum);
    }
    if let Some(volt) = sensor_data.voltage {
        println!("    Battery voltage: {:.3} V", volt); // Use 3 decimal places for voltage
    }
    if let Some(batt) = sensor_data.battery {
        println!("    Battery: {}%", batt);
    }
}