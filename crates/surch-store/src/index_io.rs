use surch_codec::codec_util::crc32_zlib;
use thiserror::Error;

pub type IndexInputResult<T> = std::result::Result<T, IndexInputError>;
pub type IndexOutputResult<T> = std::result::Result<T, IndexOutputError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IndexInputError {
    #[error("unexpected end of input at byte position {position}")]
    UnexpectedEof { position: u64 },
    #[error("cannot seek to byte position {position}; input length is {length}")]
    SeekPastEof { position: u64, length: u64 },
    #[error("slice offset {offset} and length {length} exceed input length {input_length}")]
    SliceOutOfBounds {
        offset: u64,
        length: u64,
        input_length: u64,
    },
    #[error("index input length {length} exceeds u64::MAX")]
    LengthOverflow { length: usize },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum IndexOutputError {
    #[error("index output length {length} exceeds u64::MAX")]
    LengthOverflow { length: usize },
}

pub trait IndexOutput {
    fn write_byte(&mut self, byte: u8) -> IndexOutputResult<()>;

    fn write_bytes(&mut self, bytes: &[u8]) -> IndexOutputResult<()> {
        for byte in bytes {
            self.write_byte(*byte)?;
        }
        Ok(())
    }

    fn file_pointer(&self) -> u64;

    fn checksum(&self) -> u32;
}

pub trait IndexInput: Sized {
    fn read_byte(&mut self) -> IndexInputResult<u8>;

    fn read_bytes(&mut self, target: &mut [u8]) -> IndexInputResult<()> {
        for byte in target {
            *byte = self.read_byte()?;
        }
        Ok(())
    }

    fn seek(&mut self, position: u64) -> IndexInputResult<()>;

    fn file_pointer(&self) -> u64;

    fn length(&self) -> u64;

    fn slice(&self, description: &str, offset: u64, length: u64) -> IndexInputResult<Self>;
}

#[derive(Debug, Clone, Default)]
pub struct ByteArrayIndexOutput {
    bytes: Vec<u8>,
}

impl ByteArrayIndexOutput {
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

impl IndexOutput for ByteArrayIndexOutput {
    fn write_byte(&mut self, byte: u8) -> IndexOutputResult<()> {
        self.bytes.push(byte);
        Ok(())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> IndexOutputResult<()> {
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    fn file_pointer(&self) -> u64 {
        checked_output_len(self.bytes.len())
            .expect("usize length always fits in u64 on supported targets")
    }

    fn checksum(&self) -> u32 {
        crc32_zlib(&self.bytes)
    }
}

#[derive(Debug, Clone)]
pub struct ByteArrayIndexInput<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> ByteArrayIndexInput<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }
}

impl<'a> IndexInput for ByteArrayIndexInput<'a> {
    fn read_byte(&mut self) -> IndexInputResult<u8> {
        let Some(byte) = self.bytes.get(self.position).copied() else {
            return Err(IndexInputError::UnexpectedEof {
                position: checked_input_len(self.position)?,
            });
        };
        self.position += 1;
        Ok(byte)
    }

    fn read_bytes(&mut self, target: &mut [u8]) -> IndexInputResult<()> {
        let end = self.position.saturating_add(target.len());
        if end > self.bytes.len() {
            return Err(IndexInputError::UnexpectedEof {
                position: checked_input_len(self.bytes.len())?,
            });
        }
        target.copy_from_slice(&self.bytes[self.position..end]);
        self.position = end;
        Ok(())
    }

    fn seek(&mut self, position: u64) -> IndexInputResult<()> {
        let length = self.length();
        if position > length {
            return Err(IndexInputError::SeekPastEof { position, length });
        }
        self.position = usize::try_from(position)
            .map_err(|_| IndexInputError::SeekPastEof { position, length })?;
        Ok(())
    }

    fn file_pointer(&self) -> u64 {
        checked_input_len(self.position)
            .expect("usize position always fits in u64 on supported targets")
    }

    fn length(&self) -> u64 {
        checked_input_len(self.bytes.len())
            .expect("usize length always fits in u64 on supported targets")
    }

    fn slice(&self, _description: &str, offset: u64, length: u64) -> IndexInputResult<Self> {
        let input_length = self.length();
        let Some(end) = offset.checked_add(length) else {
            return Err(IndexInputError::SliceOutOfBounds {
                offset,
                length,
                input_length,
            });
        };
        if end > input_length {
            return Err(IndexInputError::SliceOutOfBounds {
                offset,
                length,
                input_length,
            });
        }

        let offset = usize::try_from(offset).map_err(|_| IndexInputError::SliceOutOfBounds {
            offset,
            length,
            input_length,
        })?;
        let end = usize::try_from(end).map_err(|_| IndexInputError::SliceOutOfBounds {
            offset: offset as u64,
            length,
            input_length,
        })?;

        Ok(Self::new(&self.bytes[offset..end]))
    }
}

fn checked_output_len(length: usize) -> std::result::Result<u64, IndexOutputError> {
    u64::try_from(length).map_err(|_| IndexOutputError::LengthOverflow { length })
}

fn checked_input_len(length: usize) -> IndexInputResult<u64> {
    u64::try_from(length).map_err(|_| IndexInputError::LengthOverflow { length })
}
