//! Lowercase hexadecimal identifiers: random request, upload, and terminal
//! IDs, and the checks that keep them to one spelling.

use std::io::{self, Read};

/// `bytes` as lowercase hexadecimal, two digits per byte.
pub fn encode(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(char::from(DIGITS[usize::from(byte >> 4)]));
        encoded.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// `byte_count` bytes from the operating system's random source, encoded.
pub fn random(byte_count: usize) -> io::Result<String> {
    let mut bytes = vec![0_u8; byte_count];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(encode(&bytes))
}

/// Exactly `len` lowercase hexadecimal digits.
pub fn is_lower(text: &str, len: usize) -> bool {
    text.len() == len
        && text
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod tests {
    #[test]
    fn identifiers_have_one_lowercase_spelling() {
        assert_eq!(super::encode(&[0x00, 0x9f, 0xff]), "009fff");
        let id = super::random(16).unwrap();
        assert!(super::is_lower(&id, 32));
        assert_ne!(id, super::random(16).unwrap());
        assert!(!super::is_lower(
            &id.to_uppercase().replace(char::is_numeric, "A"),
            32
        ));
        assert!(!super::is_lower("abc", 4));
    }
}
