use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

pub type Result<T> = std::result::Result<T, DataIoError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DataIoError {
    #[error("unexpected end of input at byte position {position}")]
    UnexpectedEof { position: usize },
    #[error("invalid varint encoding for {kind}")]
    InvalidVarintEncoding { kind: &'static str },
    #[error("invalid UTF-8 string data")]
    InvalidUtf8,
    #[error("invalid negative length {length}")]
    NegativeLength { length: i32 },
    #[error("length {length} exceeds i32::MAX")]
    LengthOverflow { length: usize },
    #[error("cannot write negative vLong value {value}")]
    NegativeVLong { value: i64 },
}

pub trait DataInput {
    fn read_byte(&mut self) -> Result<u8>;

    fn read_bytes(&mut self, target: &mut [u8]) -> Result<()> {
        for byte in target {
            *byte = self.read_byte()?;
        }
        Ok(())
    }

    fn read_vint(&mut self) -> Result<i32> {
        let mut value = 0_u32;
        for shift in (0..32).step_by(7) {
            let byte = self.read_byte()?;
            value |= u32::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value as i32);
            }

            if shift >= 28 {
                return Err(DataIoError::InvalidVarintEncoding { kind: "vint" });
            }
        }

        Err(DataIoError::InvalidVarintEncoding { kind: "vint" })
    }

    fn read_vlong(&mut self) -> Result<i64> {
        let mut value = 0_u64;
        for shift in (0..64).step_by(7) {
            let byte = self.read_byte()?;
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return Ok(value as i64);
            }

            if shift >= 63 {
                return Err(DataIoError::InvalidVarintEncoding { kind: "vlong" });
            }
        }

        Err(DataIoError::InvalidVarintEncoding { kind: "vlong" })
    }

    fn read_zlong(&mut self) -> Result<i64> {
        Ok(zig_zag_decode_i64(self.read_vlong()? as u64))
    }

    fn read_string(&mut self) -> Result<String> {
        let length = checked_vint_length(self.read_vint()?)?;
        let mut bytes = vec![0; length];
        self.read_bytes(&mut bytes)?;
        String::from_utf8(bytes).map_err(|_| DataIoError::InvalidUtf8)
    }

    fn read_map_of_strings(&mut self) -> Result<BTreeMap<String, String>> {
        let count = checked_vint_length(self.read_vint()?)?;
        let mut values = BTreeMap::new();
        for _ in 0..count {
            values.insert(self.read_string()?, self.read_string()?);
        }
        Ok(values)
    }

    fn read_set_of_strings(&mut self) -> Result<BTreeSet<String>> {
        let count = checked_vint_length(self.read_vint()?)?;
        let mut values = BTreeSet::new();
        for _ in 0..count {
            values.insert(self.read_string()?);
        }
        Ok(values)
    }
}

pub trait DataOutput {
    fn write_byte(&mut self, byte: u8) -> Result<()>;

    fn write_bytes(&mut self, bytes: &[u8]) -> Result<()> {
        for byte in bytes {
            self.write_byte(*byte)?;
        }
        Ok(())
    }

    fn write_vint(&mut self, value: i32) -> Result<()> {
        let mut bits = value as u32;
        while bits & !0x7f != 0 {
            self.write_byte(((bits & 0x7f) | 0x80) as u8)?;
            bits >>= 7;
        }
        self.write_byte(bits as u8)
    }

    fn write_vlong(&mut self, value: i64) -> Result<()> {
        if value < 0 {
            return Err(DataIoError::NegativeVLong { value });
        }
        self.write_signed_vlong(value as u64)
    }

    fn write_zlong(&mut self, value: i64) -> Result<()> {
        self.write_signed_vlong(zig_zag_encode_i64(value))
    }

    fn write_signed_vlong(&mut self, mut value: u64) -> Result<()> {
        while value & !0x7f != 0 {
            self.write_byte(((value & 0x7f) | 0x80) as u8)?;
            value >>= 7;
        }
        self.write_byte(value as u8)
    }

    fn write_string(&mut self, value: &str) -> Result<()> {
        let bytes = value.as_bytes();
        self.write_vint(checked_usize_length(bytes.len())?)?;
        self.write_bytes(bytes)
    }

    fn write_map_of_strings(&mut self, values: &BTreeMap<String, String>) -> Result<()> {
        self.write_vint(checked_usize_length(values.len())?)?;
        for (key, value) in values {
            self.write_string(key)?;
            self.write_string(value)?;
        }
        Ok(())
    }

    fn write_set_of_strings(&mut self, values: &BTreeSet<String>) -> Result<()> {
        self.write_vint(checked_usize_length(values.len())?)?;
        for value in values {
            self.write_string(value)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct ByteArrayDataInput<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteArrayDataInput<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    pub fn position(&self) -> usize {
        self.position
    }
}

impl DataInput for ByteArrayDataInput<'_> {
    fn read_byte(&mut self) -> Result<u8> {
        let Some(byte) = self.bytes.get(self.position).copied() else {
            return Err(DataIoError::UnexpectedEof {
                position: self.position,
            });
        };
        self.position += 1;
        Ok(byte)
    }
}

#[derive(Debug, Clone, Default)]
pub struct ByteArrayDataOutput {
    bytes: Vec<u8>,
}

impl ByteArrayDataOutput {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.bytes
    }
}

impl DataOutput for ByteArrayDataOutput {
    fn write_byte(&mut self, byte: u8) -> Result<()> {
        self.bytes.push(byte);
        Ok(())
    }
}

fn zig_zag_encode_i64(value: i64) -> u64 {
    ((value as u64) << 1) ^ ((value >> 63) as u64)
}

fn zig_zag_decode_i64(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

fn checked_vint_length(length: i32) -> Result<usize> {
    usize::try_from(length).map_err(|_| DataIoError::NegativeLength { length })
}

fn checked_usize_length(length: usize) -> Result<i32> {
    i32::try_from(length).map_err(|_| DataIoError::LengthOverflow { length })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_vint_rejects_non_canonical_encoding() {
        let mut input = ByteArrayDataInput::new(&[0x80, 0x80, 0x80, 0x80, 0x80, 0x01]);
        assert!(matches!(
            input.read_vint(),
            Err(DataIoError::InvalidVarintEncoding { kind: "vint" })
        ));
    }

    #[test]
    fn read_vlong_rejects_non_canonical_encoding() {
        let mut input = ByteArrayDataInput::new(&[
            0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80,
        ]);
        assert!(matches!(
            input.read_vlong(),
            Err(DataIoError::InvalidVarintEncoding { kind: "vlong" })
        ));
    }
}
