use thiserror::Error;

use crate::field_infos::{DocValuesType, FieldInfo, FieldInfos, FieldInfosError, IndexOptions};
use surch_store::data_io::{
    ByteArrayDataInput, ByteArrayDataOutput, DataInput, DataIoError, DataOutput,
};

const MAGIC: &[u8; 4] = b"SFI0";

pub type Result<T> = std::result::Result<T, FieldInfosCodecError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FieldInfosCodecError {
    #[error("invalid field infos codec magic: {actual:?}")]
    InvalidMagic { actual: [u8; 4] },
    #[error("field infos count {count} exceeds i32::MAX")]
    FieldCountOverflow { count: usize },
    #[error("field infos count cannot be negative: {count}")]
    NegativeFieldCount { count: i32 },
    #[error("field number {number} exceeds i32::MAX")]
    FieldNumberOverflow { number: u32 },
    #[error("field number cannot be negative: {number}")]
    NegativeFieldNumber { number: i32 },
    #[error("unknown index options tag: {value}")]
    UnknownIndexOptions { value: u8 },
    #[error("unknown doc values type tag: {value}")]
    UnknownDocValuesType { value: u8 },
    #[error("invalid boolean byte for {field}: {value}")]
    InvalidBoolean { field: &'static str, value: u8 },
    #[error(transparent)]
    DataIo(#[from] DataIoError),
    #[error(transparent)]
    FieldInfos(#[from] FieldInfosError),
}

pub fn encode_field_infos(field_infos: &FieldInfos) -> Result<Vec<u8>> {
    let mut output = ByteArrayDataOutput::new();
    output.write_bytes(MAGIC)?;
    output.write_vint(checked_len(field_infos.len())?)?;

    for field in field_infos.iter() {
        output.write_string(field.name())?;
        output.write_vint(checked_field_number(field.number())?)?;
        output.write_byte(index_options_to_byte(field.index_options()))?;
        output.write_byte(doc_values_type_to_byte(field.doc_values_type()))?;
        output.write_byte(bool_to_byte(field.omit_norms()))?;
        output.write_byte(bool_to_byte(field.store_payloads()))?;
    }

    Ok(output.into_inner())
}

pub fn decode_field_infos(bytes: &[u8]) -> Result<FieldInfos> {
    let mut input = ByteArrayDataInput::new(bytes);
    let mut actual_magic = [0_u8; 4];
    input.read_bytes(&mut actual_magic)?;
    if actual_magic != *MAGIC {
        return Err(FieldInfosCodecError::InvalidMagic {
            actual: actual_magic,
        });
    }

    let count = checked_count(input.read_vint()?)?;
    let mut fields = Vec::with_capacity(count);
    for _ in 0..count {
        let name = input.read_string()?;
        let number = checked_read_field_number(input.read_vint()?)?;
        let index_options = index_options_from_byte(input.read_byte()?)?;
        let doc_values_type = doc_values_type_from_byte(input.read_byte()?)?;
        let omit_norms = bool_from_byte("omit_norms", input.read_byte()?)?;
        let store_payloads = bool_from_byte("store_payloads", input.read_byte()?)?;

        fields.push(FieldInfo::new(
            name,
            number,
            index_options,
            doc_values_type,
            omit_norms,
            store_payloads,
        ));
    }

    Ok(FieldInfos::new(fields)?)
}

fn checked_len(count: usize) -> Result<i32> {
    i32::try_from(count).map_err(|_| FieldInfosCodecError::FieldCountOverflow { count })
}

fn checked_count(count: i32) -> Result<usize> {
    usize::try_from(count).map_err(|_| FieldInfosCodecError::NegativeFieldCount { count })
}

fn checked_field_number(number: u32) -> Result<i32> {
    i32::try_from(number).map_err(|_| FieldInfosCodecError::FieldNumberOverflow { number })
}

fn checked_read_field_number(number: i32) -> Result<u32> {
    u32::try_from(number).map_err(|_| FieldInfosCodecError::NegativeFieldNumber { number })
}

fn index_options_to_byte(index_options: IndexOptions) -> u8 {
    match index_options {
        IndexOptions::None => 0,
        IndexOptions::Docs => 1,
        IndexOptions::DocsAndFreqs => 2,
        IndexOptions::DocsAndFreqsAndPositions => 3,
        IndexOptions::DocsAndFreqsAndPositionsAndOffsets => 4,
    }
}

fn index_options_from_byte(value: u8) -> Result<IndexOptions> {
    match value {
        0 => Ok(IndexOptions::None),
        1 => Ok(IndexOptions::Docs),
        2 => Ok(IndexOptions::DocsAndFreqs),
        3 => Ok(IndexOptions::DocsAndFreqsAndPositions),
        4 => Ok(IndexOptions::DocsAndFreqsAndPositionsAndOffsets),
        value => Err(FieldInfosCodecError::UnknownIndexOptions { value }),
    }
}

fn doc_values_type_to_byte(doc_values_type: DocValuesType) -> u8 {
    match doc_values_type {
        DocValuesType::None => 0,
        DocValuesType::Numeric => 1,
        DocValuesType::Binary => 2,
        DocValuesType::Sorted => 3,
        DocValuesType::SortedNumeric => 4,
        DocValuesType::SortedSet => 5,
    }
}

fn doc_values_type_from_byte(value: u8) -> Result<DocValuesType> {
    match value {
        0 => Ok(DocValuesType::None),
        1 => Ok(DocValuesType::Numeric),
        2 => Ok(DocValuesType::Binary),
        3 => Ok(DocValuesType::Sorted),
        4 => Ok(DocValuesType::SortedNumeric),
        5 => Ok(DocValuesType::SortedSet),
        value => Err(FieldInfosCodecError::UnknownDocValuesType { value }),
    }
}

fn bool_to_byte(value: bool) -> u8 {
    u8::from(value)
}

fn bool_from_byte(field: &'static str, value: u8) -> Result<bool> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(FieldInfosCodecError::InvalidBoolean { field, value }),
    }
}
