use btleplug::api::PeripheralProperties;
use uuid::Uuid;
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

fn handle_pvvx_packet(data: &Vec<u8>) {
    // TODO: Implement your Pvvx decoding logic here
    println!("    Payload ({} bytes): {:02X?}", data.len(), data);
    // e.g., print_lywsdcgq_data(&data);
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
