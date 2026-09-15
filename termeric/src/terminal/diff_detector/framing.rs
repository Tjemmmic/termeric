//! Bounded, chunk-independent framing for the passive diff observer.
//! This is deliberately not a terminal screen reconstruction: SGR and OSC are
//! ignored; editing controls, invalid UTF-8 and overlong lines reject capture.

pub(super) const MAX_LINE_BYTES: usize = 16 * 1024;

#[derive(Debug, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    Ground,
    Start,
    Csi,
    String {
        osc: bool,
    },
    StringEnd {
        osc: bool,
    },
}

#[derive(Debug, Default)]
pub(super) struct Framer {
    text: Vec<u8>,
    escape: Escape,
    wire_bytes: usize,
    rejected: bool,
    carriage_return: bool,
}

impl Framer {
    pub fn feed(&mut self, bytes: &[u8], mut line: impl FnMut(Result<&str, ()>)) {
        for &byte in bytes {
            if self.escape == Escape::Ground && byte == b'\n' {
                self.emit(&mut line);
                continue;
            }
            self.wire_bytes = self.wire_bytes.saturating_add(1).min(MAX_LINE_BYTES + 1);
            self.rejected |= self.wire_bytes > MAX_LINE_BYTES;
            // Only CRLF is losslessly recognizable in an output stream. A lone
            // CR edits the current screen line, so do not copy a guessed patch.
            if self.carriage_return {
                // ONLCR can turn a child's CRLF into CRCRLF. Repeated CRs
                // before LF are idempotent; a following text byte is editing.
                self.rejected |= byte != b'\r';
                self.carriage_return = false;
            }
            match self.escape {
                Escape::Ground => match byte {
                    b'\x1b' => self.escape = Escape::Start,
                    b'\r' => self.carriage_return = true,
                    b'\t' | 0x20..=0x7e | 0x80..=0xff => {
                        if !self.rejected {
                            if self.text.len() == self.text.capacity() {
                                let target = (self.text.capacity().max(64) * 2).min(MAX_LINE_BYTES);
                                self.text.reserve_exact(target - self.text.len());
                            }
                            self.text.push(byte);
                        }
                    }
                    _ => self.rejected = true,
                },
                Escape::Start => match byte {
                    b'[' => self.escape = Escape::Csi,
                    b']' => self.escape = Escape::String { osc: true },
                    b'P' | b'_' | b'^' | b'X' => {
                        self.rejected = true;
                        self.escape = Escape::String { osc: false };
                    }
                    _ => {
                        self.rejected = true;
                        self.escape = Escape::Ground;
                    }
                },
                Escape::Csi => match byte {
                    0x40..=0x7e => {
                        self.rejected |= byte != b'm';
                        self.escape = Escape::Ground;
                    }
                    0x20..=0x3f => {}
                    _ => {
                        self.rejected = true;
                        self.escape = Escape::Ground;
                    }
                },
                Escape::String { osc } => match byte {
                    b'\x1b' => self.escape = Escape::StringEnd { osc },
                    b'\x07' if osc => self.escape = Escape::Ground,
                    _ => {}
                },
                Escape::StringEnd { osc } => match byte {
                    b'\\' => self.escape = Escape::Ground,
                    b'\x1b' => {}
                    b'\x07' if osc => self.escape = Escape::Ground,
                    _ => self.escape = Escape::String { osc },
                },
            }
        }
    }

    pub fn finish(&mut self, mut line: impl FnMut(Result<&str, ()>)) {
        self.rejected |= self.escape != Escape::Ground || self.carriage_return;
        if self.wire_bytes != 0 || self.rejected {
            self.emit(&mut line);
        }
        self.escape = Escape::Ground;
    }

    fn emit(&mut self, line: &mut impl FnMut(Result<&str, ()>)) {
        line(if self.rejected {
            Err(())
        } else {
            std::str::from_utf8(&self.text).map_err(|_| ())
        });
        self.text.clear();
        self.wire_bytes = 0;
        self.rejected = false;
        self.carriage_return = false;
    }

    #[cfg(test)]
    pub fn allocated_bytes(&self) -> usize {
        self.text.capacity()
    }
}
