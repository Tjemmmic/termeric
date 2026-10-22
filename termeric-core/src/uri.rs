//! Reading the `file://` URIs shells and the desktop hand over.

/// The bytes `encoded` spells, with each `%XX` turned back into the byte it
/// stands for, or `None` when a `%` isn't followed by two hex digits.
pub fn percent_decode(encoded: &[u8]) -> Option<Vec<u8>> {
    let mut decoded = Vec::with_capacity(encoded.len());
    let mut index = 0;
    while index < encoded.len() {
        if encoded[index] == b'%' {
            let high = hex_digit(*encoded.get(index + 1)?)?;
            let low = hex_digit(*encoded.get(index + 2)?)?;
            decoded.push((high << 4) | low);
            index += 3;
        } else {
            decoded.push(encoded[index]);
            index += 1;
        }
    }
    Some(decoded)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_turn_back_into_their_bytes_and_bad_ones_are_refused() {
        assert_eq!(
            percent_decode(b"/a%20b/%E7%95%8C%2f").as_deref(),
            Some("/a b/界/".as_bytes())
        );
        assert_eq!(percent_decode(b"plain").as_deref(), Some(&b"plain"[..]));
        for bad in [&b"%"[..], b"%2", b"%zz", b"a%g0"] {
            assert_eq!(percent_decode(bad), None, "{bad:?}");
        }
    }
}
