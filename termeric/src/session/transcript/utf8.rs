//! Streaming lossy UTF-8 with a three-byte carry and bounded repair storage.
//! Valid chunks/prefixes are borrowed. Repair never creates multiple transcript
//! records from one call, preserving the existing per-entry redaction boundary.
use std::borrow::Cow;

#[derive(Default)]
pub(super) struct Decoder {
    pending: [u8; 4],
    len: usize,
}

pub(super) struct Decoded<'a> {
    pub text: Cow<'a, str>,
    pub replaced: bool,
}

impl Decoder {
    /// None means the entire call exceeded the repair/input budget. Forget any
    /// carry across this loss so future bytes cannot join a discarded prefix.
    pub fn decode<'a>(&mut self, bytes: &'a [u8], limit: usize) -> Option<Decoded<'a>> {
        if bytes.len() > limit {
            self.clear();
            return None;
        }
        if self.len == 0 {
            match std::str::from_utf8(bytes) {
                Ok(text) => {
                    return Some(Decoded {
                        text: Cow::Borrowed(text),
                        replaced: false,
                    });
                }
                Err(error) if error.error_len().is_none() => {
                    let split = error.valid_up_to();
                    self.save(&bytes[split..]);
                    return Some(Decoded {
                        text: Cow::Borrowed(std::str::from_utf8(&bytes[..split]).unwrap()),
                        replaced: false,
                    });
                }
                _ => {}
            }
        }
        let result = self.repair(bytes, limit);
        if result.is_none() {
            self.clear();
        }
        debug_assert!(self.len <= 3);
        result
    }

    fn repair<'a>(&mut self, mut bytes: &[u8], limit: usize) -> Option<Decoded<'a>> {
        let mut text = String::new();
        let mut replaced = false;
        while self.len > 0 {
            match std::str::from_utf8(&self.pending[..self.len]) {
                Ok(valid) => {
                    append(&mut text, valid, limit)?;
                    self.len = 0;
                }
                Err(error) => {
                    if let Some(invalid) = error.error_len() {
                        // A saved prefix is incomplete, never initially invalid.
                        // Invalidity is discovered by adding its next byte.
                        let valid = error.valid_up_to();
                        append(
                            &mut text,
                            std::str::from_utf8(&self.pending[..valid]).unwrap(),
                            limit,
                        )?;
                        append(&mut text, "\u{fffd}", limit)?;
                        replaced = true;
                        self.pending.copy_within(valid + invalid..self.len, 0);
                        self.len -= valid + invalid;
                    } else if let Some((&next, rest)) = bytes.split_first() {
                        self.pending[self.len] = next;
                        self.len += 1;
                        bytes = rest;
                    } else {
                        break;
                    }
                }
            }
        }
        while !bytes.is_empty() {
            match std::str::from_utf8(bytes) {
                Ok(valid) => {
                    append(&mut text, valid, limit)?;
                    break;
                }
                Err(error) => {
                    let valid = error.valid_up_to();
                    append(
                        &mut text,
                        std::str::from_utf8(&bytes[..valid]).unwrap(),
                        limit,
                    )?;
                    if let Some(invalid) = error.error_len() {
                        append(&mut text, "\u{fffd}", limit)?;
                        replaced = true;
                        bytes = &bytes[valid + invalid..];
                    } else {
                        self.save(&bytes[valid..]);
                        break;
                    }
                }
            }
        }
        Some(Decoded {
            text: Cow::Owned(text),
            replaced,
        })
    }

    fn save(&mut self, bytes: &[u8]) {
        debug_assert!(bytes.len() <= 3);
        self.pending[..bytes.len()].copy_from_slice(bytes);
        self.len = bytes.len();
    }

    pub fn clear(&mut self) {
        self.pending.fill(0);
        self.len = 0;
    }

    /// One replacement for a truncated final sequence; repeated finish is empty.
    pub fn finish(&mut self) -> bool {
        let partial = self.len != 0;
        self.clear();
        partial
    }
}

fn append(text: &mut String, part: &str, limit: usize) -> Option<()> {
    let required = text.len().checked_add(part.len())?;
    if required > limit {
        return None;
    }
    if required > text.capacity() {
        let target = required.max(text.capacity().saturating_mul(2)).min(limit);
        text.reserve_exact(target - text.len());
    }
    text.push_str(part);
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(bytes: &[u8], sizes: &[usize]) -> String {
        let mut decoder = Decoder::default();
        let mut text = String::new();
        let mut start = 0;
        for &size in sizes {
            let piece = decoder.decode(&bytes[start..start + size], 8192).unwrap();
            text.push_str(&piece.text);
            assert!(decoder.len <= 3);
            start += size;
        }
        assert_eq!(start, bytes.len());
        if decoder.finish() {
            text.push('\u{fffd}');
        }
        assert!(!decoder.finish());
        text
    }

    #[test]
    fn unicode_and_malformed_streams_match_lossy_decoding_at_every_split() {
        for bytes in [
            "aé根🌱e\u{301}\x1b[31m\r\n\0end".as_bytes(),
            b"before\xffafter\xc0\xaf\xed\xa0\x80\xf4\x90\x80\x80end",
            b"\xf0\x9fA\xe2\x82\xc2\xa2\xf0\x9f\x8c",
            b"\x80\xbf\xf5\xff\xfe\xe0\x80\x80\xe2",
        ] {
            let expected = String::from_utf8_lossy(bytes);
            for split in 0..=bytes.len() {
                assert_eq!(decoded(bytes, &[split, bytes.len() - split]), expected);
            }
            assert_eq!(decoded(bytes, &vec![1; bytes.len()]), expected);
        }
    }

    #[test]
    fn randomized_bytes_and_chunks_match_standard_replacement_policy() {
        let mut seed = 42_u64;
        for _ in 0..500 {
            let mut bytes = Vec::new();
            for _ in 0..97 {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
                bytes.push((seed >> 32) as u8);
            }
            let sizes: Vec<_> = bytes.chunks(3).map(<[u8]>::len).collect();
            assert_eq!(decoded(&bytes, &sizes), String::from_utf8_lossy(&bytes));
        }
    }

    #[test]
    fn repair_capacity_and_retained_carry_stay_bounded() {
        let mut decoder = Decoder::default();
        for _ in 0..1000 {
            assert!(matches!(
                decoder.decode(b"ASCII", 64).unwrap().text,
                Cow::Borrowed(_)
            ));
            let repair = decoder.decode(b"\xffhello", 64).unwrap();
            let Cow::Owned(text) = repair.text else {
                panic!("repair must own text")
            };
            assert!(text.capacity() <= 64);
            assert!(decoder.decode(&[0xff; 64], 64).is_none());
            assert_eq!(decoder.len, 0);
            decoder.decode(b"\xf0\x9f\x8c", 64).unwrap();
            assert_eq!(decoder.len, 3);
            assert!(decoder.decode(&[b'x'; 65], 64).is_none());
            assert_eq!(decoder.len, 0);
        }
        assert!(std::mem::size_of::<Decoder>() <= 16);
        assert_eq!(decoder.decode(b"\xff", 3).unwrap().text, "\u{fffd}");
        assert!(decoder.decode(b"\xff", 2).is_none());
    }
}
