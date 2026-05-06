use thiserror::Error;

use crate::field_infos::FieldInfos;
use crate::field_infos_codec::{decode_field_infos, encode_field_infos, FieldInfosCodecError};

const FIELD_INFOS_EXTENSION: &str = ".fnm";

pub type Result<T> = std::result::Result<T, SegmentFieldInfosError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SegmentFieldInfosError {
    #[error("invalid segment name: {segment_name}")]
    InvalidSegmentName { segment_name: String },
    #[error("invalid field infos file extension: {file_name}")]
    InvalidFileExtension { file_name: String },
    #[error("field infos codec error: {0}")]
    Codec(#[from] FieldInfosCodecError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFieldInfosFile {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

pub fn field_infos_file_name(segment_name: &str) -> Result<String> {
    validate_segment_name(segment_name)?;
    Ok(format!("{segment_name}{FIELD_INFOS_EXTENSION}"))
}

pub fn write_segment_field_infos(
    segment_name: &str,
    field_infos: &FieldInfos,
) -> Result<SegmentFieldInfosFile> {
    Ok(SegmentFieldInfosFile {
        file_name: field_infos_file_name(segment_name)?,
        bytes: encode_field_infos(field_infos)?,
    })
}

pub fn read_segment_field_infos(file_name: &str, bytes: &[u8]) -> Result<FieldInfos> {
    validate_field_infos_extension(file_name)?;
    Ok(decode_field_infos(bytes)?)
}

fn validate_segment_name(segment_name: &str) -> Result<()> {
    if segment_name.starts_with('_')
        && !segment_name.contains('.')
        && !segment_name.contains('/')
        && !segment_name.contains('\\')
    {
        return Ok(());
    }

    Err(SegmentFieldInfosError::InvalidSegmentName {
        segment_name: segment_name.to_owned(),
    })
}

fn validate_field_infos_extension(file_name: &str) -> Result<()> {
    if file_name.ends_with(FIELD_INFOS_EXTENSION) {
        return Ok(());
    }

    Err(SegmentFieldInfosError::InvalidFileExtension {
        file_name: file_name.to_owned(),
    })
}
