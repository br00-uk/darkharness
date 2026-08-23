//! Variable-length integer encoding.
//!
//! Task unit `G4`, Do 4: "Encode postings with delta encoding and
//! variable-length integers." [`write_varint`] and [`read_varint`] hold the
//! variable-length half of that: a LEB128-style encoding, seven value bits
//! per byte, the top bit set on every byte but the last. [`bm25::Postings`]
//! (`crate::index::bm25`) layers delta encoding of document identifiers on
//! top: after the first posting, each one stores the increase over the
//! previous document identifier rather than the identifier itself, which
//! keeps the value — and so the encoded byte count — small, since a term's
//! postings are always built and stored in ascending document-identifier
//! order.

use dark_contract::{ErrCode, Error, Result};

/// Appends `value` to `out` as a LEB128 variable-length unsigned integer.
///
/// A value under 128 costs one byte; the cost grows by one byte for every
/// seven more bits the value needs.
pub fn write_varint(out: &mut Vec<u8>, mut value: u64) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            out.push(byte);
            break;
        }
        out.push(byte | 0x80);
    }
}

/// Reads one LEB128 variable-length unsigned integer from `bytes`,
/// starting at `*pos`, and advances `*pos` past it.
///
/// # Errors
///
/// Returns `E_TOOL_FAILED` when `bytes` ends before a terminating byte (one
/// whose top bit is clear), or when the encoded value does not fit in a
/// `u64`.
pub fn read_varint(bytes: &[u8], pos: &mut usize) -> Result<u64> {
    let mut result: u64 = 0;
    let mut shift: u32 = 0;
    loop {
        let byte = *bytes.get(*pos).ok_or_else(|| {
            Error::new(
                ErrCode::ToolFailed,
                "varint decoding ran past the end of the buffer with no terminating byte",
            )
        })?;
        *pos += 1;
        if shift >= 64 {
            return Err(Error::new(
                ErrCode::ToolFailed,
                "varint encodes a value wider than 64 bits",
            ));
        }
        result |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_values_round_trip_in_one_byte() {
        for value in [0u64, 1, 63, 127] {
            let mut buf = Vec::new();
            write_varint(&mut buf, value);
            assert_eq!(buf.len(), 1, "value {value} should fit one byte");
            let mut pos = 0;
            assert_eq!(read_varint(&buf, &mut pos).unwrap(), value);
            assert_eq!(pos, buf.len());
        }
    }

    #[test]
    fn a_value_needing_a_second_byte_encodes_at_the_boundary() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 128);
        assert_eq!(buf.len(), 2);
        let mut pos = 0;
        assert_eq!(read_varint(&buf, &mut pos).unwrap(), 128);
    }

    #[test]
    fn large_values_round_trip() {
        for value in [u64::MAX, u64::MAX - 1, 1u64 << 40, 3_104_000_017] {
            let mut buf = Vec::new();
            write_varint(&mut buf, value);
            let mut pos = 0;
            assert_eq!(read_varint(&buf, &mut pos).unwrap(), value);
        }
    }

    #[test]
    fn several_values_pack_and_unpack_in_sequence() {
        let values = [0u64, 300, 2, 1_000_000, 1];
        let mut buf = Vec::new();
        for &v in &values {
            write_varint(&mut buf, v);
        }
        let mut pos = 0;
        let mut decoded = Vec::new();
        for _ in &values {
            decoded.push(read_varint(&buf, &mut pos).unwrap());
        }
        assert_eq!(decoded, values);
        assert_eq!(pos, buf.len());
    }

    #[test]
    fn truncated_bytes_report_tool_failed() {
        let mut buf = Vec::new();
        write_varint(&mut buf, 128); // needs 2 bytes
        buf.truncate(1); // cut off the terminating byte
        let mut pos = 0;
        let err = read_varint(&buf, &mut pos).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }

    #[test]
    fn an_empty_buffer_reports_tool_failed() {
        let mut pos = 0;
        let err = read_varint(&[], &mut pos).unwrap_err();
        assert_eq!(err.code, ErrCode::ToolFailed);
    }
}
