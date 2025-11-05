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
// Define the custom UUIDs used by Xiaomi/BTHome/PVVX devices
const MIJIA_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FE95_0000_1000_8000_00805F9B34FB);
const BTHOME_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000FCD2_0000_1000_8000_00805F9B34FB);
const PVVX_SERVICE_UUID: Uuid = Uuid::from_u128(0x0000181A_0000_1000_8000_00805F9B34FB);
const BTHOME_V2_PREAMBLE: [u8; 4] = [0x16, 0xd2, 0xfc, 0x40];

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
    //for (id, data) in props.manufacturer_data.clone() {
    //    println!("  🏭 Manufacturer ID: {}, data: {:02X?}", id, data);
    //}

    let rssi = props.rssi.unwrap_or(0);
    println!("📡 Found device: rssi={}", rssi);

    let (packet_type, data_option) = get_packet_type(props);

    match packet_type {
        BlePacketType::Mijia => {
            println!("  ✅ Detected: MIJIA/LYWSDCGQ Packet");
            // We know `data_option` is `Some` here.
            //handle_lywsdcgq_packet(data_option.unwrap());
            output_sensor_data(&handle_lywsdcgq_packet(data_option.unwrap()).unwrap());
        }
        BlePacketType::BTHome => {
            println!("  ✅ Detected: BTHome");
            // We know `data_option` is `Some` here.
            //handle_bthome_packet(data_option.unwrap());
            output_sensor_data(&handle_bthome_packet(data_option.unwrap()).unwrap());
        }
        BlePacketType::Pvvx => {
            println!("  ✅ Detected: PVVX Packet");
            // We know `data_option` is `Some` here.
            //handle_pvvx_packet(data_option.unwrap());
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

// --- PVVX Decoder (Adapted from your working code) ---
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
fn handle_bthome_packet(payload: &Vec<u8>) -> Option<SensorData> {
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

fn handle_lywsdcgq_packet(data: &Vec<u8>) -> Result<SensorData> {
    // The Xiaomi Manufacturer ID (0x04C0) is already stripped by btleplug.
    // The data slice starts with the payload length, version, etc.
    // The byte at index 11 (in the raw slice) is the Type Identifier byte (0x0D, 0x06, 0x0A, etc.)
    const TYPE_IDENTIFIER_OFFSET: usize = 11;
    if data.len() < TYPE_IDENTIFIER_OFFSET + 1 { 
        return Err(anyhow!("LYWSDCGQ V3 packet too short: {} bytes", data.len()));
    }
    let type_identifier = data[TYPE_IDENTIFIER_OFFSET];
 
    // Initialize all fields as None
    let mut temperature: Option<f32> = None;
    let mut humidity: Option<f32> = None;
    let mut battery_percent: Option<u8> = None;
    let voltage: Option<f32> = None; // V3 typically doesn't send voltage by default

    match type_identifier {
        // 0x0D: Combined Temperature and Humidity
        0x0D if data.len() >= 18 => {
            
            const TEMP_START: usize = 14; 
            const HUMI_START: usize = 16; 

            // TEMP (14-15)
            let raw_temp_bytes: [u8; 2] = data[TEMP_START..TEMP_START+2].try_into().unwrap_or_default();
            let raw_temp = i16::from_le_bytes(raw_temp_bytes);
            
            // *** FIX 1: Change / 100.0 to / 10.0 ***
            temperature = Some(raw_temp as f32 / 10.0);
            
            // HUMI (16-17)
            let raw_humi_bytes: [u8; 2] = data[HUMI_START..HUMI_START+2].try_into().unwrap_or_default();
            let raw_humi = u16::from_le_bytes(raw_humi_bytes);
            
            // *** FIX 2: Change / 100.0 to / 10.0 ***
            humidity = Some(raw_humi as f32 / 10.0);
        }
        
        // 0x04: Temperature Only
        // The packet length is 16 bytes total (indices 0 to 15)
        0x04 if data.len() >= 16 => {
            const TEMP_START: usize = 14; 

            // TEMP (14-15)
            let raw_temp_bytes: [u8; 2] = data[TEMP_START..TEMP_START+2].try_into().unwrap_or_default();
            let raw_temp = i16::from_le_bytes(raw_temp_bytes);
            
            // Apply 0.1 precision
            temperature = Some(raw_temp as f32 / 10.0);
        }

        // ... (You should also verify the offsets for 0x04, 0x06, and 0x0A based on this new understanding)
        // For 0x0A (Battery, 1 byte): The data starts at index 14, not 10 or 14.
        // The packet length for 0x0A is usually shorter (e.g., 15 bytes total)

        // 0x0A: Battery Percentage Only
        0x0A if data.len() >= 13 => { 
            const BATT_START: usize = 14; // Must also be checked and verified
            battery_percent = Some(data[BATT_START]);
        }

        // 0x06: Humidity Only
        // The packet length is 16 bytes total (indices 0 to 15)
        0x06 if data.len() >= 16 => {
            const HUMI_START: usize = 14; 
            
            // HUMI (14-15)
            let raw_humi_bytes: [u8; 2] = data[HUMI_START..HUMI_START+2].try_into().unwrap_or_default();
            let raw_humi = u16::from_le_bytes(raw_humi_bytes); // Humidity is typically unsigned (u16)
            
            // Apply 0.1 precision (divide by 10.0)
            humidity = Some(raw_humi as f32 / 10.0);
        }
        
        _ => {
            return Err(anyhow!("Unrecognized or incomplete LYWSDCGQ V3 payload (Type 0x{:02X}, Length {})", type_identifier, data.len()));
        }
    }

    // Return the result with whichever fields were populated
    Ok(SensorData {
        //mac_address: mac.to_string(),
        temperature,
        humidity,
        battery: battery_percent, // Assuming your struct field is named 'battery'
        voltage,
    })
}

// --- Output Function ---
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