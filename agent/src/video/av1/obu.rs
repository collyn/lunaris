//! AV1 Open Bitstream Unit (OBU) parser and Annex-B to low-overhead converter.
//!
//! FFmpeg's AV1 encoder may emit data in Annex-B format (temporal unit → frame unit → OBU
//! with LEB128 size prefixes) or raw low-overhead OBUs. The WebRTC AV1 payloader expects
//! low-overhead OBUs, so we need a converter for the Annex-B case.

use bytes::Bytes;

/// Try to convert Annex-B formatted AV1 data to low-overhead OBU format.
///
/// Returns `None` if the data is already in low-overhead format or cannot be parsed.
/// Tries two strategies: full temporal-unit hierarchy, then flat size-prefixed OBUs.
pub fn av1_annexb_to_low_overhead(data: &Bytes) -> Option<Bytes> {
    let input = data.as_ref();
    parse_av1_annexb_temporal_units(input)
        .or_else(|| parse_av1_size_prefixed_obus(input))
        .map(Bytes::from)
}

/// Parse Annex-B temporal unit hierarchy: temporal_unit → frame_unit → OBU.
///
/// Each level is LEB128-size-prefixed. Extracts raw OBU bytes (without size prefixes).
fn parse_av1_annexb_temporal_units(input: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0usize;
    let mut out = Vec::with_capacity(input.len());
    while pos < input.len() {
        let (temporal_unit_size, next) = read_av1_leb128(input, pos)?;
        pos = next;
        if temporal_unit_size == 0 || pos + temporal_unit_size > input.len() {
            return None;
        }
        let temporal_unit_end = pos + temporal_unit_size;
        while pos < temporal_unit_end {
            let (frame_unit_size, next) = read_av1_leb128(input, pos)?;
            pos = next;
            if frame_unit_size == 0 || pos + frame_unit_size > temporal_unit_end {
                return None;
            }
            let frame_unit_end = pos + frame_unit_size;
            while pos < frame_unit_end {
                let (obu_size, next) = read_av1_leb128(input, pos)?;
                pos = next;
                if obu_size == 0 || pos + obu_size > frame_unit_end {
                    return None;
                }
                out.extend_from_slice(&input[pos..pos + obu_size]);
                pos += obu_size;
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Parse flat list of LEB128-size-prefixed OBUs (no temporal/frame unit hierarchy).
fn parse_av1_size_prefixed_obus(input: &[u8]) -> Option<Vec<u8>> {
    let mut pos = 0usize;
    let mut out = Vec::with_capacity(input.len());
    while pos < input.len() {
        let (obu_size, next) = read_av1_leb128(input, pos)?;
        pos = next;
        if obu_size == 0 || pos + obu_size > input.len() {
            return None;
        }
        out.extend_from_slice(&input[pos..pos + obu_size]);
        pos += obu_size;
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Read a LEB128-encoded unsigned integer from `input` starting at `pos`.
///
/// Returns `Some((value, next_pos))` on success, `None` on malformed input.
fn read_av1_leb128(input: &[u8], mut pos: usize) -> Option<(usize, usize)> {
    let mut value: usize = 0;
    for shift in (0..8).map(|i| i * 7) {
        let byte = *input.get(pos)?;
        pos += 1;
        value |= ((byte & 0x7f) as usize) << shift;
        if byte & 0x80 == 0 {
            return Some((value, pos));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_read_leb128_single_byte() {
        // 42 in LEB128: 0x2A (no continuation bit)
        let input = [0x2A];
        let (value, next) = read_av1_leb128(&input, 0).unwrap();
        assert_eq!(value, 42);
        assert_eq!(next, 1);
    }

    #[test]
    fn test_read_leb128_two_bytes() {
        // 300 in LEB128: 0xAC 0x02
        let input = [0xAC, 0x02];
        let (value, next) = read_av1_leb128(&input, 0).unwrap();
        assert_eq!(value, 300);
        assert_eq!(next, 2);
    }

    #[test]
    fn test_read_leb128_empty() {
        let input: Vec<u8> = vec![];
        assert!(read_av1_leb128(&input, 0).is_none());
    }

    #[test]
    fn test_parse_size_prefixed_obus() {
        // Two OBUs: size=3 data=[0xAA, 0xBB, 0xCC], size=2 data=[0xDD, 0xEE]
        // LEB128(3) = [0x03], LEB128(2) = [0x02]
        let input = vec![0x03, 0xAA, 0xBB, 0xCC, 0x02, 0xDD, 0xEE];
        let result = parse_av1_size_prefixed_obus(&input).unwrap();
        assert_eq!(result, vec![0xAA, 0xBB, 0xCC, 0xDD, 0xEE]);
    }

    #[test]
    fn test_parse_size_prefixed_obus_zero_size() {
        let input = vec![0x00];
        assert!(parse_av1_size_prefixed_obus(&input).is_none());
    }
}
