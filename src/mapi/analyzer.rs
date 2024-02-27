
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Analyzer {
    Head {
        boundary: bool,
        was_body: bool,
    },
    PartialHead {
        byte1: u8,
    },
    Body {
        still_needed: u16,
        len: u16,
        last: bool,
    },
    Unix0,
    Error,
}

impl Analyzer {
    pub fn new(unix_client: bool) -> Self {
        if unix_client {
            Analyzer::Unix0
        } else {
            Analyzer::Head {
                boundary: true,
                was_body: false,
            }
        }
    }

    pub fn split_chunk<'a>(&mut self, data: &mut &'a [u8]) -> Option<&'a [u8]> {
        // self.analyze(data).map(|n| data.split_at(n))
        match self.analyze(data) {
            Some(n) => {
                let (head, tail) = data.split_at(n);
                *data = tail;
                Some(head)
            }
            None => None,
        }
    }

    fn analyze(&mut self, data: &[u8]) -> Option<usize> {
        use Analyzer::*;

        let (taken, new_state) = match (&self, data) {
            (Head { .. }, [byte1, byte2, ..]) => (2, Self::parse_header(byte1, byte2)),

            (Head { .. }, [byte1]) => (1, Self::PartialHead { byte1: *byte1 }),

            (PartialHead { byte1 }, [byte2, ..]) => (1, Self::parse_header(byte1, byte2)),

            (
                Body {
                    still_needed, last, ..
                },
                _,
            ) if *still_needed as usize <= data.len() => (
                *still_needed,
                Head {
                    was_body: true,
                    boundary: *last,
                },
            ),

            (
                Body {
                    still_needed,
                    len,
                    last,
                },
                [_byte1, ..],
            ) => {
                let n =
                    u16::try_from(data.len()).expect("large data slices handled in previous case");
                (
                    n,
                    Body {
                        still_needed: still_needed - n,
                        len: *len,
                        last: *last,
                    },
                )
            }

            (_, []) => return None,

            (Error, _) => (u16::try_from(data.len()).unwrap_or(u16::MAX), Error),

            (Unix0, [0x30, ..]) => (
                1,
                Self::Head {
                    was_body: false,
                    boundary: true,
                },
            ),

            (Unix0, [_, ..]) => (1, Self::Error),
        };
        *self = new_state;
        Some(taken as usize)
    }

    fn parse_header(byte1: &u8, byte2: &u8) -> Analyzer {
        // little endian
        let n = *byte1 as u16 + 256 * *byte2 as u16;
        let len = n / 2;
        if len <= 8190 {
            let last = n & 1 > 0;
            Self::Body {
                still_needed: len,
                len,
                last,
            }
        } else {
            Self::Error
        }
    }

    pub fn was_error(&self) -> bool {
        matches!(self, Self::Error)
    }

    pub fn was_head(&self) -> bool {
        match self {
            Self::PartialHead { .. } => true,
            Self::Body {
                still_needed, len, ..
            } => still_needed == len,
            _ => false,
        }
    }

    pub fn was_body(&self) -> bool {
        match self {
            Self::Body {
                still_needed, len, ..
            } => still_needed < len,
            Self::Head { was_body, .. } => *was_body,
            _ => false,
        }
    }

    pub fn was_block_boundary(&self) -> bool {
        matches!(self, Self::Head { .. })
    }

    pub fn was_message_boundary(&self) -> bool {
        matches!(self, Self::Head { boundary: true, .. })
    }

    pub fn check_incomplete(&self) -> Result<(), &'static str> {
        let msg = match self {
            Analyzer::Head { boundary: true, .. } => return Ok(()),
            Analyzer::Head {
                boundary: false, ..
            } => "on a block boundary but not on a message boundary",
            Analyzer::PartialHead { .. } => "in the middle of the header block",
            Analyzer::Body { last: false, .. } => "in the middle of a block",
            Analyzer::Body { last: true, .. } => "in the middle of the last block of the message",
            Analyzer::Error | Analyzer::Unix0 => return Ok(()),
        };
        Err(msg)
    }
}
