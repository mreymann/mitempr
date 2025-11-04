#[derive(Debug)]
pub enum SensorReading {
    Temperature(f32),
    Humidity(f32),
    Battery(u8),
    Voltage(f32)
    // Extendable for more sensor types
}

/// Decode ATC format (Xiaomi ATC sensors)
fn decode_atc(data: &[u8]) -> Vec<SensorReading> {
    let mut readings = Vec::new();
    if data.len() < 5 {
        return readings;
    }

    // Temperature: 2 bytes little-endian at data[2..4], scaled by 100
    let temp_raw = u16::from_le_bytes([data[2], data[3]]);
    let temp = temp_raw as f32 / 100.0;
    readings.push(SensorReading::Temperature(temp));

    // Humidity: byte at data[4], scale 0..100
    let humidity = data[4];
    readings.push(SensorReading::Humidity(humidity as f32));

    // Battery: optional, last byte
    if let Some(&batt) = data.last() {
        readings.push(SensorReading::Battery(batt));
    }

    readings
}

/// Decode BTHome v2 format (TLV-like)
fn decode_bthome(data: &[u8]) -> Vec<SensorReading> {
    let mut readings = Vec::new();
    let mut i = 0;
    let len = data.len();

    // BTHome v2 commonly encodes fields as: [field_id][value...]
    // Implement known common field ids and lengths:
    // 0x01: Temperature  -> int16 LE, scale 0.01 => /100
    // 0x02: Humidity     -> uint8 => %
    // 0x03: Battery      -> uint8 => %
    // 0x04: Voltage      -> uint16 LE => mV -> /1000 to V
    //
    // Stop parsing on unknown/insufficient bytes to avoid misalignment.
    while i < len {
        let key = data[i];
        i += 1;

        match key {
            0x01 => {
                if i + 2 <= len {
                    let raw = i16::from_le_bytes([data[i], data[i + 1]]);
                    let temp = raw as f32 / 100.0;
                    readings.push(SensorReading::Temperature(temp));
                    i += 2;
                } else {
                    break;
                }
            }
            0x02 => {
                if i + 1 <= len {
                    let hum = data[i];
                    readings.push(SensorReading::Humidity(hum as f32));
                    i += 1;
                } else {
                    break;
                }
            }
            0x03 => {
                if i + 1 <= len {
                    let batt = data[i];
                    readings.push(SensorReading::Battery(batt));
                    i += 1;
                } else {
                    break;
                }
            }
            0x04 => {
                if i + 2 <= len {
                    let raw = u16::from_le_bytes([data[i], data[i + 1]]);
                    readings.push(SensorReading::Voltage(raw as f32 / 1000.0));
                    i += 2;
                } else {
                    break;
                }
            }
            // Unknown key: cannot determine length reliably -> stop to avoid misparse
            _ => break,
        }
    }

    readings
}

/// Master decode function
pub fn decode_sensor(data: &[u8]) -> Vec<SensorReading> {
    // Try BTHome first (TLV-like). If it yields no readings, fall back to ATC.
    let bthome = decode_bthome(data);
    if !bthome.is_empty() {
        bthome
    } else {
        decode_atc(data)
    }
}
