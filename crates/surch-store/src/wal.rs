//! Lucene-compatible write-ahead log (WAL) persistence.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use surch_codec::codec_util::{
    check_footer, check_header, footer_length, write_footer, write_header, CodecUtilError,
};

const WAL_FILE_NAME: &str = "wal.log";
const WAL_CODEC: &str = "wal";
const WAL_VERSION: i32 = 1;
const MAX_WAL_BINARY_FIELD_BYTES: usize = 4 * 1024 * 1024;

pub type Result<T> = std::result::Result<T, WalError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WalError {
    #[error("WAL I/O error: {0}")]
    Io(String),

    #[error("WAL data is corrupt: {message}")]
    Corrupt { message: String },

    #[error("WAL field `{field}` too large: {length} bytes exceeds limit {limit}")]
    FieldTooLarge {
        field: &'static str,
        length: usize,
        limit: usize,
    },

    #[error("invalid WAL identifier `{value}` for {field}")]
    InvalidIdentifier { field: &'static str, value: String },

    #[error(transparent)]
    CodecUtil(#[from] CodecUtilError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WalOperation {
    Index { source: Vec<u8> },
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WalRecord {
    pub sequence: u64,
    pub timestamp_millis: u64,
    pub index: String,
    pub doc_id: String,
    pub operation: WalOperation,
}

impl WalRecord {
    fn encode(&self, bytes: &mut Vec<u8>) -> Result<()> {
        write_u64(self.sequence, bytes);
        write_u64(self.timestamp_millis, bytes);

        write_prefixed_bytes("index", self.index.as_bytes(), bytes)?;
        write_prefixed_bytes("doc_id", self.doc_id.as_bytes(), bytes)?;

        match &self.operation {
            WalOperation::Index { source } => {
                bytes.push(1);
                write_prefixed_bytes("source", source, bytes)?;
            }
            WalOperation::Delete => {
                bytes.push(2);
            }
        }

        Ok(())
    }

    fn decode(bytes: &[u8], cursor: &mut usize) -> Result<Self> {
        let sequence = read_u64(bytes, cursor)?;
        let timestamp_millis = read_u64(bytes, cursor)?;

        let index = read_prefixed_string(bytes, cursor)?;
        let doc_id = read_prefixed_string(bytes, cursor)?;

        let op = match read_u8(bytes, cursor)? {
            1 => WalOperation::Index {
                source: read_prefixed_bytes(bytes, cursor)?,
            },
            2 => WalOperation::Delete,
            value => {
                return Err(WalError::Corrupt {
                    message: format!("unknown WAL op marker: {value}"),
                });
            }
        };

        Ok(Self {
            sequence,
            timestamp_millis,
            index,
            doc_id,
            operation: op,
        })
    }
}

#[derive(Debug, Clone)]
pub struct WriteAheadLog {
    path: PathBuf,
    next_sequence: u64,
    entries: Vec<WalRecord>,
}

impl WriteAheadLog {
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        fs::create_dir_all(&path).map_err(io_error("create WAL directory"))?;

        let file_path = path.join(WAL_FILE_NAME);
        let entries = if file_path.exists() {
            Self::read_file(&file_path)?
        } else {
            Vec::new()
        };

        let next_sequence = entries.last().map(|entry| entry.sequence).unwrap_or(0);

        Ok(Self {
            path: file_path,
            next_sequence,
            entries,
        })
    }

    pub fn append_index(
        &mut self,
        index: impl Into<String>,
        doc_id: impl Into<String>,
        source: Vec<u8>,
    ) -> Result<u64> {
        let index = index.into();
        let doc_id = doc_id.into();

        validate_identifier(&index, "index")?;
        validate_identifier(&doc_id, "document id")?;

        let record = WalRecord {
            sequence: self.next_sequence + 1,
            timestamp_millis: current_millis()?,
            index,
            doc_id,
            operation: WalOperation::Index { source },
        };

        self.entries.push(record);
        self.next_sequence += 1;

        Ok(self.next_sequence)
    }

    pub fn append_delete(
        &mut self,
        index: impl Into<String>,
        doc_id: impl Into<String>,
    ) -> Result<u64> {
        let index = index.into();
        let doc_id = doc_id.into();

        validate_identifier(&index, "index")?;
        validate_identifier(&doc_id, "document id")?;

        let record = WalRecord {
            sequence: self.next_sequence + 1,
            timestamp_millis: current_millis()?,
            index,
            doc_id,
            operation: WalOperation::Delete,
        };

        self.entries.push(record);
        self.next_sequence += 1;

        Ok(self.next_sequence)
    }

    pub fn entries(&self) -> &[WalRecord] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn drain_entries(&mut self) -> Vec<WalRecord> {
        std::mem::take(&mut self.entries)
    }

    pub fn retain_entries_greater_than(&mut self, min_sequence_inclusive: u64) -> Result<()> {
        self.entries = self
            .entries
            .iter()
            .filter(|entry| entry.sequence > min_sequence_inclusive)
            .cloned()
            .collect();

        self.next_sequence = self.entries.last().map(|entry| entry.sequence).unwrap_or(0);

        self.flush()
    }

    pub fn clear(&mut self) -> Result<()> {
        self.entries.clear();
        self.next_sequence = 0;
        self.flush()
    }

    pub fn flush(&self) -> Result<()> {
        let mut bytes = Vec::new();
        write_header(&mut bytes, WAL_CODEC, WAL_VERSION)?;

        for record in &self.entries {
            record.encode(&mut bytes)?;
        }

        write_footer(&mut bytes);

        write_atomic(&self.path, &bytes)
    }

    fn read_file(path: &Path) -> Result<Vec<WalRecord>> {
        let mut file = File::open(path).map_err(io_error("open WAL file"))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(io_error("read WAL file"))?;

        if bytes.is_empty() {
            return Ok(Vec::new());
        }

        let _ = check_footer(&bytes)?;
        let header = check_header(&bytes, WAL_CODEC, WAL_VERSION, WAL_VERSION)?;

        let body_end =
            bytes
                .len()
                .checked_sub(footer_length())
                .ok_or_else(|| WalError::Corrupt {
                    message: "WAL file too small for footer".to_owned(),
                })?;

        let mut cursor = header.length;
        let mut entries = Vec::new();
        let mut last_sequence = 0_u64;

        while cursor < body_end {
            let record = WalRecord::decode(&bytes, &mut cursor)?;

            if record.sequence == 0 {
                return Err(WalError::Corrupt {
                    message: "WAL sequence cannot be zero".to_owned(),
                });
            }

            if record.sequence <= last_sequence {
                return Err(WalError::Corrupt {
                    message: "WAL sequence must be strictly increasing".to_owned(),
                });
            }

            validate_identifier(&record.index, "index")?;
            validate_identifier(&record.doc_id, "document id")?;

            entries.push(record);
            last_sequence = entries.last().expect("at least one WAL entry").sequence;
        }

        if cursor != body_end {
            return Err(WalError::Corrupt {
                message: "WAL has trailing bytes before footer".to_owned(),
            });
        }

        Ok(entries)
    }
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> WalError {
    move |error| WalError::Io(format!("{context}: {error}"))
}

fn current_millis() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| WalError::Io(format!("system clock before epoch: {error}")))?
        .as_millis()
        .try_into()
        .expect("system timestamp does not fit in u64"))
}

fn validate_identifier(value: &str, field: &'static str) -> Result<()> {
    if value.trim().is_empty() {
        return Err(WalError::InvalidIdentifier {
            field,
            value: value.to_owned(),
        });
    }

    if value
        .as_bytes()
        .iter()
        .any(|byte| *byte == 0 || *byte == b'/' || *byte == b'\\')
    {
        return Err(WalError::InvalidIdentifier {
            field,
            value: value.to_owned(),
        });
    }

    Ok(())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.to_path_buf();
    tmp.set_extension(format!(
        "{}-tmp-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("wal"),
        std::process::id()
    ));

    let mut file = File::create(&tmp).map_err(io_error("create temp WAL file"))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(io_error("write WAL file"))?;
    fs::rename(&tmp, path).map_err(io_error("rename WAL temp file"))?;

    Ok(())
}

fn write_u64(value: u64, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn _write_u8(bytes: &mut Vec<u8>, value: u8) {
    bytes.push(value);
}

fn write_prefixed_bytes(field: &'static str, value: &[u8], bytes: &mut Vec<u8>) -> Result<()> {
    if value.len() > MAX_WAL_BINARY_FIELD_BYTES {
        return Err(WalError::FieldTooLarge {
            field,
            length: value.len(),
            limit: MAX_WAL_BINARY_FIELD_BYTES,
        });
    }

    write_u32(value.len() as u32, bytes);
    bytes.extend_from_slice(value);
    Ok(())
}

fn write_u32(value: u32, bytes: &mut Vec<u8>) {
    bytes.extend_from_slice(&value.to_be_bytes());
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8> {
    let byte = bytes
        .get(*cursor)
        .copied()
        .ok_or_else(|| WalError::Corrupt {
            message: "unexpected end of WAL input".to_owned(),
        })?;
    *cursor += 1;
    Ok(byte)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32> {
    let end = cursor.checked_add(4).ok_or_else(|| WalError::Corrupt {
        message: "WAL input cursor overflow".to_owned(),
    })?;
    let chunk = bytes.get(*cursor..end).ok_or_else(|| WalError::Corrupt {
        message: "unexpected end of WAL input".to_owned(),
    })?;
    let mut array = [0_u8; 4];
    array.copy_from_slice(chunk);
    *cursor = end;
    Ok(u32::from_be_bytes(array))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64> {
    let end = cursor.checked_add(8).ok_or_else(|| WalError::Corrupt {
        message: "WAL input cursor overflow".to_owned(),
    })?;
    let chunk = bytes.get(*cursor..end).ok_or_else(|| WalError::Corrupt {
        message: "unexpected end of WAL input".to_owned(),
    })?;
    let mut array = [0_u8; 8];
    array.copy_from_slice(chunk);
    *cursor = end;
    Ok(u64::from_be_bytes(array))
}

fn read_prefixed_bytes(bytes: &[u8], cursor: &mut usize) -> Result<Vec<u8>> {
    let length = read_u32(bytes, cursor)? as usize;
    if length > MAX_WAL_BINARY_FIELD_BYTES {
        return Err(WalError::FieldTooLarge {
            field: "prefixed_data",
            length,
            limit: MAX_WAL_BINARY_FIELD_BYTES,
        });
    }

    let end = cursor
        .checked_add(length)
        .ok_or_else(|| WalError::Corrupt {
            message: "WAL length overflow".to_owned(),
        })?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| WalError::Corrupt {
            message: "unexpected end of WAL input".to_owned(),
        })?
        .to_vec();
    *cursor = end;
    Ok(value)
}

fn read_prefixed_string(bytes: &[u8], cursor: &mut usize) -> Result<String> {
    let value = read_prefixed_bytes(bytes, cursor)?;
    String::from_utf8(value).map_err(|error| WalError::Corrupt {
        message: format!("invalid UTF-8 string in WAL: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;

    #[test]
    fn current_millis_fits_in_u64_for_real_hardware() {
        let value = current_millis().expect("current timestamp");
        assert!(value > 0);
    }

    #[test]
    fn write_prefixed_round_trips_binary_vectors() {
        let values: &[&[u8]] = &[b"a", b"segment", b"\x00", b""];

        for value in values {
            let mut bytes = Vec::new();
            write_prefixed_bytes("roundtrip", value, &mut bytes).expect("encode");

            let mut cursor = 0;
            let actual = read_prefixed_bytes(&bytes, &mut cursor).expect("decode");
            assert_eq!(actual, value.to_vec());
            assert_eq!(cursor, bytes.len());
        }
    }

    #[test]
    fn write_prefixed_rejects_oversized_value() {
        let mut bytes = Vec::new();
        let oversized = vec![0_u8; MAX_WAL_BINARY_FIELD_BYTES + 1];
        let error =
            write_prefixed_bytes("source", &oversized, &mut bytes).expect_err("oversized payload");
        assert!(matches!(error, WalError::FieldTooLarge { field, .. } if field == "source"));
    }

    #[test]
    fn read_prefixed_rejects_length_above_limit() {
        let mut bytes = Vec::new();
        write_u32((MAX_WAL_BINARY_FIELD_BYTES as u32) + 1, &mut bytes);
        bytes.extend_from_slice(b"x");

        let mut cursor = 0;
        let error =
            read_prefixed_bytes(&bytes, &mut cursor).expect_err("oversize length should fail");
        assert!(matches!(
            error,
            WalError::FieldTooLarge {
                field: "prefixed_data",
                ..
            }
        ));
    }

    #[test]
    fn io_error_maps_context_message() {
        let error = io::Error::from(io::ErrorKind::NotFound);
        let mapped = io_error("read")(error);
        assert!(matches!(mapped, WalError::Io(message) if message.contains("read")));
    }
}
