//! Newline-delimited input from other processes, read with bounded memory.
//! Callers decide what an oversized or unterminated line means.

use std::io::{self, BufRead};

/// What one bounded line read found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LineRead {
    /// The line ran past the limit: only its first `limit` bytes were kept,
    /// and the rest of it was consumed.
    pub oversized: bool,
    /// The line ended with a newline, not at the end of the input.
    pub terminated: bool,
}

/// Read one line into `line`, which is cleared first, keeping at most
/// `limit` bytes and never the newline. `None` at the end of the input.
pub fn read_bounded_line(
    reader: &mut impl BufRead,
    limit: usize,
    line: &mut Vec<u8>,
) -> io::Result<Option<LineRead>> {
    line.clear();
    let mut oversized = false;
    let mut read_any = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(read_any.then_some(LineRead {
                oversized,
                terminated: false,
            }));
        }
        read_any = true;
        let newline = available.iter().position(|byte| *byte == b'\n');
        let content = newline.map_or(available, |index| &available[..index]);
        let kept = content.len().min(limit - line.len());
        line.extend_from_slice(&content[..kept]);
        oversized |= kept < content.len();
        let consumed = newline.map_or(available.len(), |index| index + 1);
        reader.consume(consumed);
        if newline.is_some() {
            return Ok(Some(LineRead {
                oversized,
                terminated: true,
            }));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_cut_at_the_limit_and_the_rest_is_consumed() {
        let data = format!("short\n{}\n\nafter\ntail", "x".repeat(40));
        let mut reader = io::BufReader::with_capacity(8, data.as_bytes());
        let mut line = Vec::new();
        let mut reads = Vec::new();
        while let Some(read) = read_bounded_line(&mut reader, 16, &mut line).unwrap() {
            reads.push((String::from_utf8(line.clone()).unwrap(), read));
        }
        let read = |oversized, terminated| LineRead {
            oversized,
            terminated,
        };
        assert_eq!(
            reads,
            [
                ("short".to_owned(), read(false, true)),
                ("x".repeat(16), read(true, true)),
                (String::new(), read(false, true)),
                ("after".to_owned(), read(false, true)),
                ("tail".to_owned(), read(false, false)),
            ]
        );
    }

    #[test]
    fn a_line_of_exactly_the_limit_is_not_oversized() {
        let mut line = Vec::new();
        let read = read_bounded_line(&mut "abcd\n".as_bytes(), 4, &mut line)
            .unwrap()
            .unwrap();
        assert_eq!(line, b"abcd");
        assert!(!read.oversized && read.terminated);
    }
}
