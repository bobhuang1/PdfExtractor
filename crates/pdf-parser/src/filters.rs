//! Stream filters that are simple enough to own outright.
//!
//! A PDF stream names its encoding in `/Filter`, and the encodings here are the
//! ones a producer can choose before or instead of `FlateDecode`: hex, base-85,
//! LZW and run-length. They are small, fully specified algorithms, so they are
//! implemented rather than delegated.
//!
//! Every decoder returns `None` on malformed input, which the caller treats as
//! "leave the bytes alone": a partially decoded stream is worse than raw bytes,
//! because it silently corrupts whatever it was carrying.
//!
//! Not handled here: `/DecodeParms` predictors, which apply *after* these
//! decoders and are undone where image samples are assembled.

/// Decode `/ASCIIHexDecode`: pairs of hex digits, whitespace ignored, `>`
/// terminates. An odd final digit is padded with `0`, as the specification says.
pub fn ascii_hex(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut high: Option<u8> = None;
    for &byte in data {
        match byte {
            b'>' => break,
            b if b.is_ascii_whitespace() => continue,
            b => {
                let digit = (b as char).to_digit(16)? as u8;
                match high.take() {
                    Some(h) => out.push(h << 4 | digit),
                    None => high = Some(digit),
                }
            }
        }
    }
    if let Some(h) = high {
        out.push(h << 4);
    }
    Some(out)
}

/// Decode `/ASCII85Decode`: five base-85 digits per four bytes, with `z` as
/// shorthand for four zero bytes and a final partial group producing fewer.
pub fn ascii85(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 4 / 5);
    // Digits of the group being accumulated, and whether `z` may still appear.
    let mut group: Vec<u8> = Vec::with_capacity(5);
    let mut at_group_start = true;
    let mut started = false;

    let flush = |group: &mut Vec<u8>, out: &mut Vec<u8>| -> Option<()> {
        if group.is_empty() {
            return Some(());
        }
        let count = group.len();
        if count == 1 {
            // A single leftover digit cannot encode anything; the spec allows it
            // to be ignored, but it also means the stream is malformed.
            return None;
        }
        let mut value: u32 = 0;
        for i in 0..5 {
            // A short group is padded with the highest digit, `u`.
            let digit = group.get(i).copied().unwrap_or(b'u');
            value = value.checked_mul(85)?.checked_add((digit - b'!') as u32)?;
        }
        let bytes = value.to_be_bytes();
        out.extend_from_slice(&bytes[..count - 1]);
        group.clear();
        Some(())
    };

    let mut i = 0usize;
    // Skip the optional `<~` introducer.
    if data.starts_with(b"<~") {
        i = 2;
    }
    while i < data.len() {
        let byte = data[i];
        i += 1;
        match byte {
            b'~' => break,
            b if b.is_ascii_whitespace() => continue,
            b'z' if at_group_start => {
                out.extend_from_slice(&[0, 0, 0, 0]);
                started = true;
            }
            b'!'..=b'u' => {
                group.push(byte);
                started = true;
                at_group_start = false;
                if group.len() == 5 {
                    flush(&mut group, &mut out)?;
                    at_group_start = true;
                }
            }
            _ => return None,
        }
    }
    flush(&mut group, &mut out)?;
    if !started {
        return None;
    }
    Some(out)
}

/// Decode `/RunLengthDecode`: a length byte, then either literal bytes or one
/// repeated byte. `128` ends the stream.
pub fn run_length(data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut i = 0usize;
    while i < data.len() {
        let length = data[i];
        i += 1;
        match length {
            128 => return Some(out),
            // 0..=127: the next `length + 1` bytes are literal.
            0..=127 => {
                let count = length as usize + 1;
                if i + count > data.len() {
                    return None;
                }
                out.extend_from_slice(&data[i..i + count]);
                i += count;
            }
            // 129..=255: the next byte is repeated `257 - length` times.
            _ => {
                let count = 257 - length as usize;
                let byte = *data.get(i)?;
                out.extend(std::iter::repeat_n(byte, count));
                i += 1;
            }
        }
    }
    // A stream without an explicit end marker is truncated, but everything
    // decoded up to that point is still usable.
    Some(out)
}

/// Decode `/LZWDecode`, the variable-width LZW variant the specification
/// defines: big-endian bit packing, codes starting at nine bits.
///
/// `early_change` is `/DecodeParms /EarlyChange`, which producers set to `1`
/// (the default) a code early.
pub fn lzw(data: &[u8], early_change: bool) -> Option<Vec<u8>> {
    const CLEAR: u16 = 256;
    const EOD: u16 = 257;
    const FIRST_CODE: u16 = 258;
    const MAX_CODE: u16 = 4096;

    let mut out = Vec::with_capacity(data.len() * 2);
    // Dictionary entries as (previous code, appended byte): a prefix tree is
    // enough to expand any code without storing the strings themselves.
    let mut prefix: Vec<u16> = vec![0; MAX_CODE as usize];
    let mut suffix: Vec<u8> = vec![0; MAX_CODE as usize];
    let mut next_code = FIRST_CODE;
    let mut width: u32 = 9;
    let mut previous: Option<u16> = None;
    let mut bits = BitReader::new(data);
    let mut stack: Vec<u8> = Vec::new();

    while let Some(code) = bits.read(width) {
        if code == EOD {
            break;
        }
        if code == CLEAR {
            next_code = FIRST_CODE;
            width = 9;
            previous = None;
            continue;
        }

        // Expand the code: walk the prefix chain to a literal, then output it
        // in reverse order.
        if code >= next_code {
            // The one case that needs a guess: the code being defined right now,
            // which is the previous string followed by its own first byte.
            let known = previous?;
            stack.clear();
            expand(known, &prefix, &suffix, &mut stack);
            stack.reverse();
            let first = *stack.first()?;
            out.extend_from_slice(&stack);
            out.push(first);
        } else {
            stack.clear();
            expand(code, &prefix, &suffix, &mut stack);
            stack.reverse();
            out.extend_from_slice(&stack);
        }

        if let Some(prev) = previous {
            if next_code < MAX_CODE {
                // The new entry is the previous string plus the first byte of the
                // one just emitted. When the code was the one being defined
                // (`code >= next_code`) that first byte is the previous string's
                // own first byte, which is exactly what the decoder can know.
                let source = if code >= next_code { prev } else { code };
                let first = first_byte(source, next_code, &prefix, &suffix, &mut stack)?;
                prefix[next_code as usize] = prev;
                suffix[next_code as usize] = first;
                next_code += 1;
                // Widen when the next code no longer fits, one code early by
                // default so a decoder stays in step with the encoder.
                let limit = if early_change {
                    (1 << width) - 1
                } else {
                    1 << width
                };
                if next_code == limit && width < 12 {
                    width += 1;
                }
            }
        }
        previous = Some(code);
    }
    Some(out)
}

/// Follow a code's prefix chain to its first byte, which is what a new
/// dictionary entry appends to its prefix.
fn first_byte(
    code: u16,
    next_code: u16,
    prefix: &[u16],
    suffix: &[u8],
    stack: &mut Vec<u8>,
) -> Option<u8> {
    stack.clear();
    // A code that is not in the dictionary yet is the code being defined; it has
    // no first byte to give. Callers pass the previous code in that case.
    if code >= next_code {
        return None;
    }
    expand(code, prefix, suffix, stack);
    stack.last().copied()
}

/// Push the bytes of `code` onto `stack`, last byte first.
fn expand(code: u16, prefix: &[u16], suffix: &[u8], stack: &mut Vec<u8>) {
    let mut current = code;
    // A literal is its own byte; anything from 258 up has a prefix.
    loop {
        if current < 256 {
            stack.push(current as u8);
            return;
        }
        stack.push(suffix[current as usize]);
        current = prefix[current as usize];
    }
}

/// Big-endian bit reader, the order LZW codes are packed in.
struct BitReader<'a> {
    data: &'a [u8],
    bit: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, bit: 0 }
    }

    fn read(&mut self, width: u32) -> Option<u16> {
        let mut value: u16 = 0;
        for _ in 0..width {
            let byte = self.bit / 8;
            if byte >= self.data.len() {
                return None;
            }
            let shift = 7 - (self.bit % 8);
            let bit = (self.data[byte] >> shift) & 1;
            value = (value << 1) | bit as u16;
            self.bit += 1;
        }
        Some(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_decoding_ignores_whitespace_and_terminator() {
        assert_eq!(ascii_hex(b"48 65 6C 6C 6F>").unwrap(), b"Hello");
        // A trailing nibble is padded, not dropped.
        assert_eq!(ascii_hex(b"48656C6C6F2").unwrap(), b"Hello ");
        assert_eq!(ascii_hex(b"4865\n6c6c6f").unwrap(), b"Hello");
        assert!(ascii_hex(b"4g").is_none(), "a non-hex digit is malformed");
    }

    #[test]
    fn base85_decodes_the_documented_example() {
        // "Man " is the classic first group: five digits, four bytes.
        assert_eq!(ascii85(b"9jqo^").unwrap(), b"Man ");
        assert_eq!(ascii85(b"<~9jqo^~>").unwrap(), b"Man ");
        // `z` is four zero bytes.
        assert_eq!(ascii85(b"z").unwrap(), vec![0, 0, 0, 0]);
        assert_eq!(ascii85(b"zz").unwrap(), vec![0; 8]);
        // A partial final group yields one byte fewer than its digit count.
        assert_eq!(ascii85(b"9jqo").unwrap(), b"Man");
        assert!(
            ascii85(b"9jqo}").is_none(),
            "a digit above `u` is malformed"
        );
    }

    #[test]
    fn run_length_expands_literals_and_repeats() {
        // 2 literal bytes, then 4 copies of 0x41, then the end marker.
        let data = [1u8, b'h', b'i', 253, b'A', 128];
        assert_eq!(run_length(&data).unwrap(), b"hiAAAA");
        // Without an end marker the data still decodes.
        assert_eq!(run_length(&[0, b'x']).unwrap(), b"x");
        assert!(
            run_length(&[5, b'x']).is_none(),
            "a truncated run is malformed"
        );
    }

    #[test]
    fn lzw_decodes_the_documented_example() {
        // The specification's own example: ten input bytes of `-----A---B`
        // (45 45 45 45 45 65 45 45 45 66) compress to these nine bytes.
        let encoded = [0x80u8, 0x0b, 0x60, 0x50, 0x22, 0x0c, 0x0c, 0x85, 0x01];
        let decoded = lzw(&encoded, true).expect("the example should decode");
        assert_eq!(decoded, vec![45, 45, 45, 45, 45, 65, 45, 45, 45, 66]);
    }

    /// The reference encoder, so round-trips can be checked without trusting a
    /// table copied by hand.
    fn encode(input: &[u8], early_change: bool) -> Vec<u8> {
        let mut dict: std::collections::HashMap<Vec<u8>, u16> = std::collections::HashMap::new();
        let reset = |dict: &mut std::collections::HashMap<Vec<u8>, u16>| {
            dict.clear();
            for byte in 0..=255u16 {
                dict.insert(vec![byte as u8], byte);
            }
        };
        reset(&mut dict);
        let mut next = 258u16;
        let mut width = 9u32;
        let mut out: Vec<u8> = Vec::new();
        let mut bits: Vec<bool> = Vec::new();
        let push = |code: u16, width: u32, bits: &mut Vec<bool>| {
            for i in (0..width).rev() {
                bits.push((code >> i) & 1 == 1);
            }
        };

        push(256, width, &mut bits); // clear to start
        let mut current: Vec<u8> = Vec::new();
        for &byte in input {
            let mut candidate = current.clone();
            candidate.push(byte);
            if dict.contains_key(&candidate) {
                current = candidate;
                continue;
            }
            let code = dict[&current];
            push(code, width, &mut bits);
            if next < 4096 {
                dict.insert(candidate, next);
                next += 1;
                let limit = if early_change {
                    (1 << width) - 1
                } else {
                    1 << width
                };
                if next == limit && width < 12 {
                    width += 1;
                }
            }
            current = vec![byte];
        }
        if !current.is_empty() {
            push(dict[&current], width, &mut bits);
        }
        push(257, width, &mut bits); // end of data

        for chunk in bits.chunks(8) {
            let mut byte = 0u8;
            for (i, bit) in chunk.iter().enumerate() {
                if *bit {
                    byte |= 1 << (7 - i);
                }
            }
            out.push(byte);
        }
        out
    }

    #[test]
    fn lzw_round_trips_through_the_reference_encoder() {
        for early_change in [true, false] {
            for input in [
                "".as_bytes(),
                b"a",
                b"aaaaa",
                b"the quick brown fox jumps over the lazy dog",
                // Long and repetitive, so the code width has to grow to 12 bits.
                &b"abcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabcabc".repeat(40)[..],
            ] {
                let encoded = encode(input, early_change);
                let decoded = lzw(&encoded, early_change)
                    .unwrap_or_else(|| panic!("failed to decode {} bytes", input.len()));
                assert_eq!(
                    decoded,
                    input,
                    "round trip failed for {} bytes (early_change {early_change})",
                    input.len()
                );
            }
        }
    }

    #[test]
    fn lzw_honours_a_clear_code_mid_stream() {
        // clear, 'A', 'A', clear, 'B', EOD: the clear discards the dictionary,
        // so the second 'B' must come from a literal rather than a stale entry.
        let mut bits: Vec<bool> = Vec::new();
        let push = |code: u16, width: u32, bits: &mut Vec<bool>| {
            for i in (0..width).rev() {
                bits.push((code >> i) & 1 == 1);
            }
        };
        for code in [256u16, 65, 65, 256, 66, 257] {
            push(code, 9, &mut bits);
        }
        let mut encoded = Vec::new();
        for chunk in bits.chunks(8) {
            let mut byte = 0u8;
            for (i, bit) in chunk.iter().enumerate() {
                if *bit {
                    byte |= 1 << (7 - i);
                }
            }
            encoded.push(byte);
        }
        assert_eq!(lzw(&encoded, true).unwrap(), b"AAB");
    }

    #[test]
    fn malformed_lzw_does_not_panic() {
        // An empty stream decodes to nothing.
        assert_eq!(lzw(&[], true).unwrap(), Vec::<u8>::new());
        // Code 0 is the literal NUL byte. One byte holds only eight of the nine
        // bits a code needs, so it is the second byte that completes it, and the
        // seven bits left over are correctly discarded.
        assert_eq!(lzw(&[0x00], true).unwrap(), Vec::<u8>::new());
        assert_eq!(lzw(&[0x00, 0x00], true).unwrap(), vec![0]);
        // A stream whose first code is already above the dictionary has nothing
        // to expand from; it must fail rather than loop or panic.
        assert_eq!(lzw(&[0xff, 0xff, 0xff], true), None);
    }
}
