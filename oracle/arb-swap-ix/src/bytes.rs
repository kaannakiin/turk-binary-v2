use crate::error::PoolsError;
use solana_pubkey::Pubkey;

macro_rules! le_reader {
    ($name:ident, $ty:ty, $width:expr) => {
        #[inline]
        pub(crate) fn $name(data: &[u8], off: usize) -> Result<$ty, PoolsError> {
            let end = off + $width;
            let slice = data.get(off..end).ok_or(PoolsError::BufferTooSmall {
                expected: end,
                got: data.len(),
            })?;
            let mut buf = [0u8; $width];
            buf.copy_from_slice(slice);
            Ok(<$ty>::from_le_bytes(buf))
        }
    };
}

le_reader!(read_u16, u16, 2);
le_reader!(read_u32, u32, 4);
le_reader!(read_i32, i32, 4);
le_reader!(read_i64, i64, 8);
le_reader!(read_u64, u64, 8);
le_reader!(read_u128, u128, 16);

#[inline]
pub(crate) fn read_pubkey(data: &[u8], off: usize) -> Result<Pubkey, PoolsError> {
    let end = off + 32;
    let slice = data.get(off..end).ok_or(PoolsError::BufferTooSmall {
        expected: end,
        got: data.len(),
    })?;
    let mut buf = [0u8; 32];
    buf.copy_from_slice(slice);
    Ok(Pubkey::new_from_array(buf))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_pubkey_rejects_short_buffer() {
        let data = [7u8; 40];
        match read_pubkey(&data, 9) {
            Err(PoolsError::BufferTooSmall { expected, got }) => {
                assert_eq!(expected, 41);
                assert_eq!(got, 40);
            }
            other => panic!("expected BufferTooSmall, got {other:?}"),
        }
        assert_eq!(
            read_pubkey(&data, 8).unwrap(),
            Pubkey::new_from_array([7u8; 32])
        );
    }

    #[test]
    fn le_readers_reject_short_buffers_and_decode_at_offset_zero() {
        assert_eq!(read_u16(&0xBEEFu16.to_le_bytes(), 0).unwrap(), 0xBEEF);
        assert!(read_u16(&0xBEEFu16.to_le_bytes(), 1).is_err());
        assert_eq!(
            read_u32(&0xDEAD_BEEFu32.to_le_bytes(), 0).unwrap(),
            0xDEAD_BEEF
        );
        assert!(read_u32(&0xDEAD_BEEFu32.to_le_bytes(), 1).is_err());
        assert_eq!(read_i32(&(-7i32).to_le_bytes(), 0).unwrap(), -7);
        assert!(read_i32(&(-7i32).to_le_bytes(), 3).is_err());
        assert_eq!(read_i64(&(-9i64).to_le_bytes(), 0).unwrap(), -9);
        assert!(read_i64(&(-9i64).to_le_bytes(), 1).is_err());
        assert_eq!(read_u64(&u64::MAX.to_le_bytes(), 0).unwrap(), u64::MAX);
        assert!(read_u64(&u64::MAX.to_le_bytes(), 1).is_err());
        assert_eq!(
            read_u128(&123_456_789_012_345_678_901u128.to_le_bytes(), 0).unwrap(),
            123_456_789_012_345_678_901
        );
        assert!(read_u128(&1u128.to_le_bytes(), 1).is_err());
    }
}
