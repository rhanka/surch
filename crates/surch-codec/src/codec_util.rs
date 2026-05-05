use thiserror::Error;

pub const CODEC_MAGIC: i32 = 0x3fd7_6c17;
pub const FOOTER_MAGIC: i32 = !CODEC_MAGIC;

pub type Result<T> = std::result::Result<T, CodecUtilError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedHeader {
    pub version: i32,
    pub length: usize,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CodecUtilError {
    #[error("codec must be simple ASCII and less than 128 characters: {codec}")]
    InvalidCodecName { codec: String },
    #[error("input ended before codec value could be read")]
    UnexpectedEof,
    #[error("codec header mismatch: actual header={actual} vs expected header={expected}")]
    HeaderMismatch { actual: i32, expected: i32 },
    #[error("codec mismatch: actual codec={actual} vs expected codec={expected}")]
    CodecMismatch { actual: String, expected: String },
    #[error("index format too old: actual={actual}, min={min}, max={max}")]
    VersionTooOld { actual: i32, min: i32, max: i32 },
    #[error("index format too new: actual={actual}, min={min}, max={max}")]
    VersionTooNew { actual: i32, min: i32, max: i32 },
    #[error("codec name was not valid UTF-8")]
    InvalidUtf8,
    #[error(
        "misplaced codec footer: length={length} but footerLength=={}",
        footer_length()
    )]
    FooterTooShort { length: usize },
    #[error("codec footer mismatch: actual footer={actual} vs expected footer={expected}")]
    FooterMismatch { actual: i32, expected: i32 },
    #[error("codec footer mismatch: unknown algorithmID: {algorithm_id}")]
    UnknownChecksumAlgorithm { algorithm_id: i32 },
    #[error("illegal CRC-32 checksum: {checksum}")]
    IllegalChecksum { checksum: u64 },
    #[error("checksum failed: expected={expected:08x} actual={actual:08x}")]
    ChecksumMismatch { expected: u32, actual: u32 },
}

pub fn header_length(codec: &str) -> usize {
    9 + codec.len()
}

pub fn footer_length() -> usize {
    16
}

pub fn write_header(output: &mut Vec<u8>, codec: &str, version: i32) -> Result<()> {
    validate_codec_name(codec)?;
    write_be_i32(output, CODEC_MAGIC);
    write_string(output, codec);
    write_be_i32(output, version);
    Ok(())
}

pub fn check_header(
    input: &[u8],
    codec: &str,
    min_version: i32,
    max_version: i32,
) -> Result<CheckedHeader> {
    let mut position = 0;
    let actual_header = read_be_i32(input, &mut position)?;
    if actual_header != CODEC_MAGIC {
        return Err(CodecUtilError::HeaderMismatch {
            actual: actual_header,
            expected: CODEC_MAGIC,
        });
    }

    let actual_codec = read_string(input, &mut position)?;
    if actual_codec != codec {
        return Err(CodecUtilError::CodecMismatch {
            actual: actual_codec,
            expected: codec.to_owned(),
        });
    }

    let actual_version = read_be_i32(input, &mut position)?;
    if actual_version < min_version {
        return Err(CodecUtilError::VersionTooOld {
            actual: actual_version,
            min: min_version,
            max: max_version,
        });
    }
    if actual_version > max_version {
        return Err(CodecUtilError::VersionTooNew {
            actual: actual_version,
            min: min_version,
            max: max_version,
        });
    }

    Ok(CheckedHeader {
        version: actual_version,
        length: position,
    })
}

pub fn write_footer(output: &mut Vec<u8>) {
    write_be_i32(output, FOOTER_MAGIC);
    write_be_i32(output, 0);
    let checksum = crc32_zlib(output);
    write_be_u64(output, u64::from(checksum));
}

pub fn retrieve_checksum(input: &[u8]) -> Result<u32> {
    let checksum = validate_footer(input)?;
    Ok(checksum as u32)
}

pub fn check_footer(input: &[u8]) -> Result<u32> {
    let expected = retrieve_checksum(input)?;
    let actual = crc32_zlib(&input[..input.len() - 8]);
    if expected != actual {
        return Err(CodecUtilError::ChecksumMismatch { expected, actual });
    }
    Ok(actual)
}

pub fn checksum_entire_file(input: &[u8]) -> Result<u32> {
    check_footer(input)
}

pub fn crc32_zlib(input: &[u8]) -> u32 {
    let mut crc = 0xffff_ffff_u32;
    for byte in input {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn validate_codec_name(codec: &str) -> Result<()> {
    if !codec.is_ascii() || codec.len() >= 128 {
        return Err(CodecUtilError::InvalidCodecName {
            codec: codec.to_owned(),
        });
    }
    Ok(())
}

fn validate_footer(input: &[u8]) -> Result<u64> {
    if input.len() < footer_length() {
        return Err(CodecUtilError::FooterTooShort {
            length: input.len(),
        });
    }

    let footer_start = input.len() - footer_length();
    let mut position = footer_start;
    let actual_footer = read_be_i32(input, &mut position)?;
    if actual_footer != FOOTER_MAGIC {
        return Err(CodecUtilError::FooterMismatch {
            actual: actual_footer,
            expected: FOOTER_MAGIC,
        });
    }

    let algorithm_id = read_be_i32(input, &mut position)?;
    if algorithm_id != 0 {
        return Err(CodecUtilError::UnknownChecksumAlgorithm { algorithm_id });
    }

    let checksum = read_be_u64(input, &mut position)?;
    if checksum & 0xffff_ffff_0000_0000 != 0 {
        return Err(CodecUtilError::IllegalChecksum { checksum });
    }
    Ok(checksum)
}

fn write_string(output: &mut Vec<u8>, value: &str) {
    write_vint(output, value.len() as u32);
    output.extend_from_slice(value.as_bytes());
}

fn read_string(input: &[u8], position: &mut usize) -> Result<String> {
    let length = read_vint(input, position)? as usize;
    let end = position
        .checked_add(length)
        .ok_or(CodecUtilError::UnexpectedEof)?;
    if end > input.len() {
        return Err(CodecUtilError::UnexpectedEof);
    }
    let value = std::str::from_utf8(&input[*position..end])
        .map_err(|_| CodecUtilError::InvalidUtf8)?
        .to_owned();
    *position = end;
    Ok(value)
}

fn write_vint(output: &mut Vec<u8>, mut value: u32) {
    while value & !0x7f != 0 {
        output.push(((value & 0x7f) | 0x80) as u8);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_vint(input: &[u8], position: &mut usize) -> Result<u32> {
    let mut value = 0_u32;
    for shift in (0..32).step_by(7) {
        let byte = read_byte(input, position)?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Ok(value)
}

fn write_be_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn write_be_u64(output: &mut Vec<u8>, value: u64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn read_be_i32(input: &[u8], position: &mut usize) -> Result<i32> {
    let bytes = read_array::<4>(input, position)?;
    Ok(i32::from_be_bytes(bytes))
}

fn read_be_u64(input: &[u8], position: &mut usize) -> Result<u64> {
    let bytes = read_array::<8>(input, position)?;
    Ok(u64::from_be_bytes(bytes))
}

fn read_byte(input: &[u8], position: &mut usize) -> Result<u8> {
    let Some(byte) = input.get(*position).copied() else {
        return Err(CodecUtilError::UnexpectedEof);
    };
    *position += 1;
    Ok(byte)
}

fn read_array<const N: usize>(input: &[u8], position: &mut usize) -> Result<[u8; N]> {
    let end = position
        .checked_add(N)
        .ok_or(CodecUtilError::UnexpectedEof)?;
    if end > input.len() {
        return Err(CodecUtilError::UnexpectedEof);
    }

    let mut bytes = [0_u8; N];
    bytes.copy_from_slice(&input[*position..end]);
    *position = end;
    Ok(bytes)
}
