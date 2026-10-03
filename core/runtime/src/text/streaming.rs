//! Incremental Unicode decoding. Pending input is bounded independently of the
//! guest heap; successful calls retain at most one incomplete scalar value.
use super::Encoding;
use boa_engine::{JsResult, JsString, js_error};

const MAX_INPUT: usize = 16 * 1024 * 1024;

#[derive(Debug, Default, Clone)]
pub(super) struct DecoderState {
    pending: Vec<u8>,
    continuing: bool,
    bom_seen: bool,
}

impl DecoderState {
    pub(super) fn decode(
        &mut self,
        input: &[u8],
        encoding: Encoding,
        fatal: bool,
        ignore_bom: bool,
        stream: bool,
    ) -> JsResult<JsString> {
        if !self.continuing {
            self.pending.clear();
            self.bom_seen = false;
        }
        if input.len() > MAX_INPUT.saturating_sub(self.pending.len()) {
            return Err(js_error!(RangeError: "TextDecoder input limit exceeded"));
        }
        self.continuing = stream;
        self.pending.extend_from_slice(input);
        let mut output = Vec::new();
        let mut consumed = 0;
        let mut failed = false;
        while consumed < self.pending.len() {
            let remaining = &self.pending[consumed..];
            let step = match encoding {
                Encoding::Utf8 => utf8(remaining, stream),
                Encoding::Utf16Le => utf16(remaining, stream, false),
                Encoding::Utf16Be => utf16(remaining, stream, true),
            };
            match step {
                Step::Pending => break,
                Step::Scalar(value, count) => {
                    let mut units = [0; 2];
                    output.extend_from_slice(value.encode_utf16(&mut units));
                    consumed += count;
                }
                Step::Invalid(count) => {
                    consumed += count;
                    if fatal {
                        failed = true;
                        break;
                    }
                    output.push(0xfffd);
                }
            }
        }
        self.pending.drain(..consumed);
        if failed {
            return Err(js_error!(TypeError: "The encoded data was not valid"));
        }
        // Serialize only after successful decoding. A fatal error must not
        // consume the BOM-seen flag for an output that was never returned.
        let skip_bom = !self.bom_seen && !ignore_bom && output.first() == Some(&0xfeff);
        self.bom_seen |= !output.is_empty();
        Ok(JsString::from(&output[usize::from(skip_bom)..]))
    }
}

enum Step {
    Scalar(char, usize),
    Invalid(usize),
    Pending,
}

fn utf8(input: &[u8], stream: bool) -> Step {
    let width = match input[0] {
        0..=0x7f => return Step::Scalar(char::from(input[0]), 1),
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return Step::Invalid(1),
    };
    let prefix = &input[..width.min(input.len())];
    match std::str::from_utf8(prefix) {
        Ok(text) => Step::Scalar(text.chars().next().expect("nonempty UTF-8 prefix"), width),
        Err(error) => match error.error_len() {
            Some(count) => Step::Invalid(count),
            None if stream => Step::Pending,
            None => Step::Invalid(prefix.len()),
        },
    }
}

fn utf16(input: &[u8], stream: bool, big_endian: bool) -> Step {
    if input.len() < 2 {
        return if stream {
            Step::Pending
        } else {
            Step::Invalid(1)
        };
    }
    let unit = |bytes: &[u8]| {
        let pair = [bytes[0], bytes[1]];
        if big_endian {
            u16::from_be_bytes(pair)
        } else {
            u16::from_le_bytes(pair)
        }
    };
    let first = unit(input);
    match first {
        0xdc00..=0xdfff => Step::Invalid(2),
        0xd800..=0xdbff => {
            if input.len() < 4 {
                return if stream {
                    Step::Pending
                } else {
                    Step::Invalid(input.len())
                };
            }
            let second = unit(&input[2..]);
            if !(0xdc00..=0xdfff).contains(&second) {
                return Step::Invalid(2);
            }
            let scalar = 0x10000 + ((u32::from(first) - 0xd800) << 10) + u32::from(second) - 0xdc00;
            Step::Scalar(char::from_u32(scalar).expect("validated surrogate pair"), 4)
        }
        _ => Step::Scalar(
            char::from_u32(u32::from(first)).expect("non-surrogate unit"),
            2,
        ),
    }
}
