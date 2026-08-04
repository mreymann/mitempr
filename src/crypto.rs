//! AES-CCM decryption for encrypted BTHome and MiBeacon advertisements.
//!
//! Both formats use AES-128-CCM with a 4-byte MIC and a per-device bind key, but
//! they build the nonce differently, and MiBeacon authenticates one byte of
//! associated data while BTHome v2 authenticates none. The constructions here
//! follow the two reference implementations:
//!
//!  - <https://github.com/Bluetooth-Devices/bthome-ble>
//!  - <https://github.com/Bluetooth-Devices/xiaomi-ble>
//!
//! A wrong key or a wrong nonce fails the MIC check, so a mistake here shows up
//! as "could not decrypt" rather than as plausible-looking wrong measurements.

use aes::Aes128;
use ccm::Ccm;
use ccm::aead::generic_array::GenericArray;
use ccm::aead::{AeadInPlace, KeyInit};
use ccm::consts::{U4, U12, U13};

/// A 16-byte AES-128 bind key, as printed by the app that paired the sensor.
pub type BindKey = [u8; 16];

/// BTHome v2: 13-byte nonce, 4-byte MIC.
type BthomeCcm = Ccm<Aes128, U4, U13>;
/// MiBeacon v4/v5: 12-byte nonce, 4-byte MIC.
type MibeaconCcm = Ccm<Aes128, U4, U12>;

/// Both formats authenticate with a 4-byte MIC.
const MIC_LEN: usize = 4;
/// BTHome v2 appends a 4-byte counter and then the MIC.
const BTHOME_COUNTER_LEN: usize = 4;
const BTHOME_TRAILER: usize = BTHOME_COUNTER_LEN + MIC_LEN;
/// MiBeacon appends a 3-byte counter and then the MIC.
const MIBEACON_COUNTER_LEN: usize = 3;
const MIBEACON_TRAILER: usize = MIBEACON_COUNTER_LEN + MIC_LEN;

/// The BTHome service UUID, little-endian, as it appears in the nonce.
const BTHOME_UUID_LE: [u8; 2] = [0xD2, 0xFC];

/// MiBeacon authenticates this single byte of associated data.
const MIBEACON_AAD: [u8; 1] = [0x11];

#[derive(Debug, PartialEq, Eq)]
pub enum CryptoError {
    /// The advertisement is too short to hold a ciphertext plus its trailer.
    TooShort,
    /// The MIC did not match: wrong bind key, or a corrupted advertisement.
    Authentication,
}

impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooShort => f.write_str("encrypted payload is too short"),
            Self::Authentication => {
                f.write_str("MIC does not match, the bind key is probably wrong")
            }
        }
    }
}

/// Decrypt a BTHome v2 payload.
///
/// `service_data` is the whole service data, starting with the device-information
/// byte. `mac` is the sender's address in the order it is printed.
///
/// Nonce: MAC, then the service UUID little-endian, then the device-information
/// byte, then the 4-byte counter that precedes the MIC. No associated data.
pub fn decrypt_bthome(
    service_data: &[u8],
    mac: [u8; 6],
    key: &BindKey,
) -> Result<Vec<u8>, CryptoError> {
    // device info byte + at least one byte of ciphertext + counter + MIC
    if service_data.len() < 1 + 1 + BTHOME_TRAILER {
        return Err(CryptoError::TooShort);
    }

    let device_info = service_data[0];
    let split = service_data.len() - BTHOME_TRAILER;
    let ciphertext = &service_data[1..split];
    let counter = &service_data[split..split + BTHOME_COUNTER_LEN];
    let mic = &service_data[split + BTHOME_COUNTER_LEN..];

    let mut nonce = Vec::with_capacity(13);
    nonce.extend_from_slice(&mac);
    nonce.extend_from_slice(&BTHOME_UUID_LE);
    nonce.push(device_info);
    nonce.extend_from_slice(counter);

    let cipher = BthomeCcm::new(GenericArray::from_slice(key));
    let mut plaintext = ciphertext.to_vec();
    cipher
        .decrypt_in_place_detached(
            GenericArray::from_slice(&nonce),
            &[],
            &mut plaintext,
            GenericArray::from_slice(mic),
        )
        .map_err(|_| CryptoError::Authentication)?;

    Ok(plaintext)
}

/// Decrypt a MiBeacon v4/v5 object payload.
///
/// `service_data` is the whole service data, starting with the frame control.
/// `objects_at` is where the object payload begins, as worked out from the
/// frame-control bits. `mac` is the sender's address in the order it is printed.
///
/// Nonce: MAC in advertisement order (i.e. reversed), then the product id and
/// frame counter, then the 3-byte counter that precedes the MIC. One byte of
/// associated data, 0x11.
pub fn decrypt_mibeacon(
    service_data: &[u8],
    objects_at: usize,
    mac: [u8; 6],
    key: &BindKey,
) -> Result<Vec<u8>, CryptoError> {
    // At least one byte of ciphertext has to fit between the header and the
    // trailer.
    if service_data.len() < objects_at + 1 + MIBEACON_TRAILER || objects_at < 5 {
        return Err(CryptoError::TooShort);
    }

    let split = service_data.len() - MIBEACON_TRAILER;
    let ciphertext = &service_data[objects_at..split];
    let counter = &service_data[split..split + MIBEACON_COUNTER_LEN];
    let mic = &service_data[split + MIBEACON_COUNTER_LEN..];

    let mut reversed_mac = mac;
    reversed_mac.reverse();

    let mut nonce = Vec::with_capacity(12);
    nonce.extend_from_slice(&reversed_mac);
    // Product id (2 bytes) and frame counter (1 byte).
    nonce.extend_from_slice(&service_data[2..5]);
    nonce.extend_from_slice(counter);

    let cipher = MibeaconCcm::new(GenericArray::from_slice(key));
    let mut plaintext = ciphertext.to_vec();
    cipher
        .decrypt_in_place_detached(
            GenericArray::from_slice(&nonce),
            &MIBEACON_AAD,
            &mut plaintext,
            GenericArray::from_slice(mic),
        )
        .map_err(|_| CryptoError::Authentication)?;

    Ok(plaintext)
}

/// Parse a bind key written as 32 hex characters.
pub fn parse_bindkey(value: &str) -> Result<BindKey, String> {
    let trimmed = value.trim();
    if trimmed.len() != 32 {
        return Err(format!(
            "a bind key is 32 hex characters, this one has {}",
            trimmed.len()
        ));
    }

    let mut key = [0u8; 16];
    for (index, byte) in key.iter_mut().enumerate() {
        let pair = &trimmed[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16)
            .map_err(|_| format!("{pair:?} in the bind key is not hexadecimal"))?;
    }

    Ok(key)
}

/// Encrypt, for tests only: it lets a round trip check the nonce construction
/// without needing a captured encrypted advertisement. Also used by the decoder
/// tests to build a whole encrypted advertisement.
#[cfg(test)]
pub(crate) fn seal(
    cipher_kind: Kind,
    plaintext: &[u8],
    nonce: &[u8],
    key: &BindKey,
) -> (Vec<u8>, [u8; MIC_LEN]) {
    let mut buffer = plaintext.to_vec();
    let tag = match cipher_kind {
        Kind::Bthome => BthomeCcm::new(GenericArray::from_slice(key)).encrypt_in_place_detached(
            GenericArray::from_slice(nonce),
            &[],
            &mut buffer,
        ),
        Kind::Mibeacon => MibeaconCcm::new(GenericArray::from_slice(key))
            .encrypt_in_place_detached(GenericArray::from_slice(nonce), &MIBEACON_AAD, &mut buffer),
    }
    .expect("encryption should succeed");

    let mut mic = [0u8; MIC_LEN];
    mic.copy_from_slice(&tag);
    (buffer, mic)
}

#[cfg(test)]
#[derive(Copy, Clone)]
pub(crate) enum Kind {
    Bthome,
    Mibeacon,
}

/// Assemble an encrypted BTHome v2 service-data payload, for tests.
#[cfg(test)]
pub(crate) fn seal_bthome(
    objects: &[u8],
    device_info: u8,
    counter: [u8; BTHOME_COUNTER_LEN],
    mac: [u8; 6],
    key: &BindKey,
) -> Vec<u8> {
    let mut nonce = Vec::new();
    nonce.extend_from_slice(&mac);
    nonce.extend_from_slice(&BTHOME_UUID_LE);
    nonce.push(device_info);
    nonce.extend_from_slice(&counter);

    let (ciphertext, mic) = seal(Kind::Bthome, objects, &nonce, key);

    let mut service_data = vec![device_info];
    service_data.extend_from_slice(&ciphertext);
    service_data.extend_from_slice(&counter);
    service_data.extend_from_slice(&mic);
    service_data
}

/// Assemble an encrypted MiBeacon service-data payload, for tests. `header` runs
/// from the frame control up to where the objects would start.
#[cfg(test)]
pub(crate) fn seal_mibeacon(
    objects: &[u8],
    header: &[u8],
    counter: [u8; MIBEACON_COUNTER_LEN],
    mac: [u8; 6],
    key: &BindKey,
) -> Vec<u8> {
    let mut reversed = mac;
    reversed.reverse();

    let mut nonce = Vec::new();
    nonce.extend_from_slice(&reversed);
    nonce.extend_from_slice(&header[2..5]);
    nonce.extend_from_slice(&counter);

    let (ciphertext, mic) = seal(Kind::Mibeacon, objects, &nonce, key);

    let mut service_data = header.to_vec();
    service_data.extend_from_slice(&ciphertext);
    service_data.extend_from_slice(&counter);
    service_data.extend_from_slice(&mic);
    service_data
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: BindKey = [
        0x23, 0x1D, 0x39, 0xC1, 0xD7, 0xCC, 0x1A, 0xB1, 0xAE, 0xE2, 0x24, 0xCD, 0x09, 0x6D, 0xB9,
        0x32,
    ];
    const MAC: [u8; 6] = [0xA4, 0xC1, 0x38, 0xA0, 0x7B, 0x03];

    /// Build an encrypted BTHome v2 advertisement around a plaintext object list,
    /// then decrypt it. A round trip is what pins the nonce layout: encrypt and
    /// decrypt derive it independently from the finished packet.
    #[test]
    fn a_bthome_round_trip_recovers_the_objects() {
        let objects = [0x02, 0x7D, 0x09, 0x03, 0x8D, 0x18];
        let device_info = 0x41; // v2, encrypted
        let counter = [0x08, 0x00, 0x00, 0x00];

        let mut nonce = Vec::new();
        nonce.extend_from_slice(&MAC);
        nonce.extend_from_slice(&BTHOME_UUID_LE);
        nonce.push(device_info);
        nonce.extend_from_slice(&counter);
        let (ciphertext, mic) = seal(Kind::Bthome, &objects, &nonce, &KEY);

        let mut service_data = vec![device_info];
        service_data.extend_from_slice(&ciphertext);
        service_data.extend_from_slice(&counter);
        service_data.extend_from_slice(&mic);

        let plaintext = decrypt_bthome(&service_data, MAC, &KEY).expect("should decrypt");
        assert_eq!(plaintext, objects);
    }

    #[test]
    fn a_bthome_payload_with_the_wrong_key_is_rejected() {
        let objects = [0x02, 0x7D, 0x09];
        let device_info = 0x41;
        let counter = [0x01, 0x00, 0x00, 0x00];

        let mut nonce = Vec::new();
        nonce.extend_from_slice(&MAC);
        nonce.extend_from_slice(&BTHOME_UUID_LE);
        nonce.push(device_info);
        nonce.extend_from_slice(&counter);
        let (ciphertext, mic) = seal(Kind::Bthome, &objects, &nonce, &KEY);

        let mut service_data = vec![device_info];
        service_data.extend_from_slice(&ciphertext);
        service_data.extend_from_slice(&counter);
        service_data.extend_from_slice(&mic);

        let mut wrong = KEY;
        wrong[0] ^= 0xFF;
        assert_eq!(
            decrypt_bthome(&service_data, MAC, &wrong),
            Err(CryptoError::Authentication)
        );
    }

    /// The nonce includes the MAC, so a packet replayed by a different device
    /// must not authenticate.
    #[test]
    fn a_bthome_payload_from_another_mac_is_rejected() {
        let objects = [0x02, 0x7D, 0x09];
        let device_info = 0x41;
        let counter = [0x01, 0x00, 0x00, 0x00];

        let mut nonce = Vec::new();
        nonce.extend_from_slice(&MAC);
        nonce.extend_from_slice(&BTHOME_UUID_LE);
        nonce.push(device_info);
        nonce.extend_from_slice(&counter);
        let (ciphertext, mic) = seal(Kind::Bthome, &objects, &nonce, &KEY);

        let mut service_data = vec![device_info];
        service_data.extend_from_slice(&ciphertext);
        service_data.extend_from_slice(&counter);
        service_data.extend_from_slice(&mic);

        let other_mac = [0xA4, 0xC1, 0x38, 0xAA, 0xBB, 0xCC];
        assert_eq!(
            decrypt_bthome(&service_data, other_mac, &KEY),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn a_mibeacon_round_trip_recovers_the_objects() {
        let objects = [0x0D, 0x10, 0x04, 0xEA, 0x00, 0x61, 0x02];
        // Frame control 0x2058: MAC and object included, encrypted.
        let header: [u8; 11] = [
            0x58, 0x20, 0xAA, 0x01, 0xF5, 0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4,
        ];
        let counter = [0x0A, 0x00, 0x00];

        let mut reversed = MAC;
        reversed.reverse();
        let mut nonce = Vec::new();
        nonce.extend_from_slice(&reversed);
        nonce.extend_from_slice(&header[2..5]);
        nonce.extend_from_slice(&counter);
        let (ciphertext, mic) = seal(Kind::Mibeacon, &objects, &nonce, &KEY);

        let mut service_data = header.to_vec();
        service_data.extend_from_slice(&ciphertext);
        service_data.extend_from_slice(&counter);
        service_data.extend_from_slice(&mic);

        let plaintext = decrypt_mibeacon(&service_data, 11, MAC, &KEY).expect("should decrypt");
        assert_eq!(plaintext, objects);
    }

    #[test]
    fn a_mibeacon_payload_with_the_wrong_key_is_rejected() {
        let objects = [0x0D, 0x10, 0x04, 0xEA, 0x00, 0x61, 0x02];
        let header: [u8; 11] = [
            0x58, 0x20, 0xAA, 0x01, 0xF5, 0x03, 0x7B, 0xA0, 0x38, 0xC1, 0xA4,
        ];
        let counter = [0x0A, 0x00, 0x00];

        let mut reversed = MAC;
        reversed.reverse();
        let mut nonce = Vec::new();
        nonce.extend_from_slice(&reversed);
        nonce.extend_from_slice(&header[2..5]);
        nonce.extend_from_slice(&counter);
        let (ciphertext, mic) = seal(Kind::Mibeacon, &objects, &nonce, &KEY);

        let mut service_data = header.to_vec();
        service_data.extend_from_slice(&ciphertext);
        service_data.extend_from_slice(&counter);
        service_data.extend_from_slice(&mic);

        let mut wrong = KEY;
        wrong[15] ^= 0x01;
        assert_eq!(
            decrypt_mibeacon(&service_data, 11, MAC, &wrong),
            Err(CryptoError::Authentication)
        );
    }

    #[test]
    fn payloads_without_room_for_a_trailer_are_rejected() {
        assert_eq!(
            decrypt_bthome(&[0x41, 0x00, 0x00], MAC, &KEY),
            Err(CryptoError::TooShort)
        );
        assert_eq!(
            decrypt_mibeacon(&[0x58, 0x20, 0xAA, 0x01, 0xF5], 5, MAC, &KEY),
            Err(CryptoError::TooShort)
        );
    }

    #[test]
    fn a_bind_key_is_32_hex_characters() {
        assert_eq!(
            parse_bindkey("231d39c1d7cc1ab1aee224cd096db932").expect("should parse"),
            KEY
        );
        assert_eq!(
            parse_bindkey("231D39C1D7CC1AB1AEE224CD096DB932").expect("uppercase should parse"),
            KEY
        );
        assert_eq!(
            parse_bindkey("  231d39c1d7cc1ab1aee224cd096db932  ").expect("should be trimmed"),
            KEY
        );
    }

    #[test]
    fn unusable_bind_keys_are_rejected() {
        for value in [
            "231d39c1d7cc1ab1aee224cd096db9",
            "231d39c1d7cc1ab1aee224cd096db93200",
            "231d39c1d7cc1ab1aee224cd096db9zz",
            "",
        ] {
            assert!(
                parse_bindkey(value).is_err(),
                "{value:?} should be rejected"
            );
        }
    }
}
