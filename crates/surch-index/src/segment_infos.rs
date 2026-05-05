use std::collections::{BTreeMap, BTreeSet};

use surch_codec::codec_util::{
    check_footer, check_header, footer_length, write_footer, write_header, CodecUtilError,
};
use thiserror::Error;

pub const SEGMENTS: &str = "segments";
pub const PENDING_SEGMENTS: &str = "pending_segments";
pub const OLD_SEGMENTS_GEN: &str = "segments.gen";
pub const SEGMENTS_CODEC: &str = "segments";
pub const SEGMENTS_VERSION_74: i32 = 9;
pub const SEGMENTS_VERSION_86: i32 = 10;
pub const SEGMENTS_VERSION_CURRENT: i32 = SEGMENTS_VERSION_86;
pub const LUCENE_LATEST_VERSION: (i32, i32, i32) = (11, 0, 0);

pub type Result<T> = std::result::Result<T, SegmentInfosError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SegmentInfosError {
    #[error("\"segments.gen\" is not a valid segment file name since 4.0")]
    OldSegmentsGen,
    #[error("fileName \"{file_name}\" is not a segments file")]
    NotSegmentsFile { file_name: String },
    #[error("invalid generation in segments file name \"{file_name}\"")]
    InvalidGeneration { file_name: String },
    #[error("indexCreatedVersionMajor must be >= 6, got: {major}")]
    UnsupportedIndexCreatedVersion { major: i32 },
    #[error("cannot decrease generation to {requested} from current generation {current}")]
    GenerationDecrease { requested: i64, current: i64 },
    #[error("codec error: {0}")]
    Codec(#[from] CodecUtilError),
    #[error("unexpected end of segment infos input")]
    UnexpectedEof,
    #[error("invalid index header id length: {length}")]
    InvalidIdLength { length: usize },
    #[error("file mismatch, expected suffix={expected}, got={actual}")]
    SuffixMismatch { expected: String, actual: String },
    #[error("invalid segment count: {count}")]
    InvalidSegmentCount { count: i32 },
    #[error("invalid segment commit info id marker: {marker}")]
    InvalidSegmentCommitInfoIdMarker { marker: u8 },
    #[error("empty commit reader does not support segment count: {count}")]
    UnsupportedSegmentCount { count: i32 },
    #[error("empty commit reader does not support commit user data count: {count}")]
    UnsupportedUserData { count: i32 },
    #[error("segment infos body has {count} trailing byte(s) before footer")]
    TrailingBytes { count: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfos {
    index_created_version_major: i32,
    generation: i64,
    last_generation: i64,
    id: Option<[u8; 16]>,
    lucene_version: Option<(i32, i32, i32)>,
    pub user_data: BTreeMap<String, String>,
    pub segments: Vec<SegmentCommitInfo>,
    pub counter: i64,
    pub version: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfosCommit {
    pub file_name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentMetadata {
    pub name: String,
    pub id: [u8; 16],
    pub codec: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentCommitInfo {
    pub metadata: SegmentMetadata,
    pub del_gen: i64,
    pub del_count: i32,
    pub field_infos_gen: i64,
    pub doc_values_gen: i64,
    pub soft_del_count: i32,
    pub sci_id: Option<[u8; 16]>,
    pub field_infos_files: BTreeSet<String>,
    pub doc_values_update_files: BTreeMap<i32, BTreeSet<String>>,
}

impl SegmentInfos {
    pub fn new(index_created_version_major: i32) -> Result<Self> {
        if index_created_version_major < 6 {
            return Err(SegmentInfosError::UnsupportedIndexCreatedVersion {
                major: index_created_version_major,
            });
        }

        Ok(Self {
            index_created_version_major,
            generation: 0,
            last_generation: 0,
            id: None,
            lucene_version: None,
            user_data: BTreeMap::new(),
            segments: Vec::new(),
            counter: 0,
            version: 0,
        })
    }

    pub fn index_created_version_major(&self) -> i32 {
        self.index_created_version_major
    }

    pub fn generation(&self) -> i64 {
        self.generation
    }

    pub fn last_generation(&self) -> i64 {
        self.last_generation
    }

    pub fn id(&self) -> Option<[u8; 16]> {
        self.id
    }

    pub fn lucene_version(&self) -> Option<(i32, i32, i32)> {
        self.lucene_version
    }

    pub fn get_segments_file_name(&self) -> Option<String> {
        file_name_from_generation(SEGMENTS, "", self.last_generation)
    }

    pub fn get_next_pending_generation(&self) -> i64 {
        if self.generation == -1 {
            1
        } else {
            self.generation + 1
        }
    }

    pub fn set_next_write_generation(&mut self, generation: i64) -> Result<()> {
        if generation < self.generation {
            return Err(SegmentInfosError::GenerationDecrease {
                requested: generation,
                current: self.generation,
            });
        }
        self.generation = generation;
        Ok(())
    }

    pub fn write_empty_commit(&mut self, id: &[u8; 16]) -> Result<SegmentInfosCommit> {
        self.write_commit(id)
    }

    pub fn write_commit(&mut self, id: &[u8; 16]) -> Result<SegmentInfosCommit> {
        let generation = self.get_next_pending_generation();
        self.generation = generation;
        self.last_generation = generation;
        self.id = Some(*id);
        self.lucene_version = Some(LUCENE_LATEST_VERSION);

        let suffix = format_base36(generation);
        let mut bytes = Vec::new();
        write_header(&mut bytes, SEGMENTS_CODEC, SEGMENTS_VERSION_CURRENT)?;
        bytes.extend_from_slice(id);
        bytes.push(suffix.len() as u8);
        bytes.extend_from_slice(suffix.as_bytes());

        write_vint(&mut bytes, LUCENE_LATEST_VERSION.0);
        write_vint(&mut bytes, LUCENE_LATEST_VERSION.1);
        write_vint(&mut bytes, LUCENE_LATEST_VERSION.2);
        write_vint(&mut bytes, self.index_created_version_major);
        write_be_i64(&mut bytes, self.version);
        write_vlong(&mut bytes, self.counter as u64);
        write_be_i32(&mut bytes, self.segments.len() as i32);
        for segment in &self.segments {
            write_segment_commit_info(&mut bytes, segment);
        }
        write_map_of_strings(&mut bytes, &self.user_data);
        write_footer(&mut bytes);

        Ok(SegmentInfosCommit {
            file_name: file_name_from_generation(SEGMENTS, "", generation)
                .expect("positive generation must produce a file name"),
            bytes,
        })
    }

    pub fn read_empty_commit(file_name: &str, bytes: &[u8]) -> Result<Self> {
        let infos = Self::read_commit(file_name, bytes)?;
        if !infos.segments.is_empty() {
            return Err(SegmentInfosError::UnsupportedSegmentCount {
                count: infos.segments.len() as i32,
            });
        }
        Ok(infos)
    }

    pub fn read_commit(file_name: &str, bytes: &[u8]) -> Result<Self> {
        let generation = generation_from_segments_file_name(file_name)?;
        check_footer(bytes)?;

        if bytes.len() < footer_length() {
            return Err(SegmentInfosError::UnexpectedEof);
        }
        let body_end = bytes.len() - footer_length();
        let header = check_header(
            &bytes[..body_end],
            SEGMENTS_CODEC,
            SEGMENTS_VERSION_74,
            SEGMENTS_VERSION_CURRENT,
        )?;
        let mut position = header.length;
        let id = read_array::<16>(bytes, &mut position)?;
        let suffix = read_byte_string(bytes, &mut position)?;
        let expected_suffix = format_base36(generation);
        if suffix != expected_suffix {
            return Err(SegmentInfosError::SuffixMismatch {
                expected: expected_suffix,
                actual: suffix,
            });
        }

        let lucene_version = (
            read_vint(bytes, &mut position)?,
            read_vint(bytes, &mut position)?,
            read_vint(bytes, &mut position)?,
        );
        let index_created_version_major = read_vint(bytes, &mut position)?;
        let version = read_be_i64(bytes, &mut position)?;
        let counter = read_vlong(bytes, &mut position)? as i64;
        let segment_count = read_be_i32(bytes, &mut position)?;
        if segment_count < 0 {
            return Err(SegmentInfosError::InvalidSegmentCount {
                count: segment_count,
            });
        }
        let mut segments = Vec::with_capacity(segment_count as usize);
        for _ in 0..segment_count {
            segments.push(read_segment_commit_info(bytes, &mut position)?);
        }
        let user_data = read_map_of_strings(bytes, &mut position)?;
        if position != body_end {
            return Err(SegmentInfosError::TrailingBytes {
                count: body_end - position,
            });
        }

        let mut infos = SegmentInfos::new(index_created_version_major)?;
        infos.generation = generation;
        infos.last_generation = generation;
        infos.id = Some(id);
        infos.lucene_version = Some(lucene_version);
        infos.user_data = user_data;
        infos.segments = segments;
        infos.version = version;
        infos.counter = counter;
        Ok(infos)
    }
}

pub fn get_last_commit_generation<'a, I>(files: I) -> Result<i64>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut max_generation = -1;
    for file in files {
        if file.starts_with(SEGMENTS) && !file.starts_with(OLD_SEGMENTS_GEN) {
            let generation = generation_from_segments_file_name(file)?;
            if generation > max_generation {
                max_generation = generation;
            }
        }
    }
    Ok(max_generation)
}

pub fn file_name_from_generation(base: &str, extension: &str, generation: i64) -> Option<String> {
    if generation == -1 {
        return None;
    }
    if generation == 0 {
        return Some(segment_file_name(base, "", extension));
    }

    let mut file_name = String::with_capacity(base.len() + 6 + extension.len());
    file_name.push_str(base);
    file_name.push('_');
    file_name.push_str(&format_base36(generation));
    if !extension.is_empty() {
        file_name.push('.');
        file_name.push_str(extension);
    }
    Some(file_name)
}

pub fn generation_from_segments_file_name(file_name: &str) -> Result<i64> {
    if file_name == OLD_SEGMENTS_GEN {
        return Err(SegmentInfosError::OldSegmentsGen);
    }
    if file_name == SEGMENTS {
        return Ok(0);
    }
    if file_name.starts_with(SEGMENTS) {
        let generation_start = SEGMENTS.len() + 1;
        let Some(generation) = file_name.get(generation_start..) else {
            return Err(SegmentInfosError::InvalidGeneration {
                file_name: file_name.to_owned(),
            });
        };
        return parse_base36(generation).ok_or_else(|| SegmentInfosError::InvalidGeneration {
            file_name: file_name.to_owned(),
        });
    }

    Err(SegmentInfosError::NotSegmentsFile {
        file_name: file_name.to_owned(),
    })
}

fn segment_file_name(segment_name: &str, segment_suffix: &str, extension: &str) -> String {
    if extension.is_empty() && segment_suffix.is_empty() {
        return segment_name.to_owned();
    }

    let mut file_name =
        String::with_capacity(segment_name.len() + 2 + segment_suffix.len() + extension.len());
    file_name.push_str(segment_name);
    if !segment_suffix.is_empty() {
        file_name.push('_');
        file_name.push_str(segment_suffix);
    }
    if !extension.is_empty() {
        file_name.push('.');
        file_name.push_str(extension);
    }
    file_name
}

fn parse_base36(value: &str) -> Option<i64> {
    if value.is_empty() {
        return None;
    }

    let mut parsed = 0_i64;
    for ch in value.bytes() {
        let digit = match ch {
            b'0'..=b'9' => i64::from(ch - b'0'),
            b'a'..=b'z' => i64::from(ch - b'a' + 10),
            b'A'..=b'Z' => i64::from(ch - b'A' + 10),
            _ => return None,
        };
        parsed = parsed.checked_mul(36)?.checked_add(digit)?;
    }
    Some(parsed)
}

fn format_base36(value: i64) -> String {
    if value == 0 {
        return "0".to_owned();
    }

    let negative = value < 0;
    let mut remaining = value.unsigned_abs();
    let mut digits = Vec::new();
    while remaining > 0 {
        let digit = (remaining % 36) as u8;
        digits.push(match digit {
            0..=9 => char::from(b'0' + digit),
            _ => char::from(b'a' + digit - 10),
        });
        remaining /= 36;
    }
    if negative {
        digits.push('-');
    }
    digits.iter().rev().collect()
}

fn write_vint(output: &mut Vec<u8>, value: i32) {
    let mut bits = value as u32;
    while bits & !0x7f != 0 {
        output.push(((bits & 0x7f) | 0x80) as u8);
        bits >>= 7;
    }
    output.push(bits as u8);
}

fn write_string(output: &mut Vec<u8>, value: &str) {
    write_vint(output, value.len() as i32);
    output.extend_from_slice(value.as_bytes());
}

fn read_string(input: &[u8], position: &mut usize) -> Result<String> {
    let length = read_vint(input, position)?;
    if length < 0 {
        return Err(SegmentInfosError::UnexpectedEof);
    }
    let length = length as usize;
    let end = position
        .checked_add(length)
        .ok_or(SegmentInfosError::UnexpectedEof)?;
    if end > input.len() {
        return Err(SegmentInfosError::UnexpectedEof);
    }
    let value = std::str::from_utf8(&input[*position..end])
        .map_err(|_| SegmentInfosError::UnexpectedEof)?
        .to_owned();
    *position = end;
    Ok(value)
}

fn write_map_of_strings(output: &mut Vec<u8>, map: &BTreeMap<String, String>) {
    write_vint(output, map.len() as i32);
    for (key, value) in map {
        write_string(output, key);
        write_string(output, value);
    }
}

fn write_set_of_strings(output: &mut Vec<u8>, set: &BTreeSet<String>) {
    write_vint(output, set.len() as i32);
    for value in set {
        write_string(output, value);
    }
}

fn read_set_of_strings(input: &[u8], position: &mut usize) -> Result<BTreeSet<String>> {
    let count = read_vint(input, position)?;
    if count < 0 {
        return Err(SegmentInfosError::UnexpectedEof);
    }

    let mut set = BTreeSet::new();
    for _ in 0..count {
        set.insert(read_string(input, position)?);
    }
    Ok(set)
}

fn write_segment_commit_info(output: &mut Vec<u8>, segment: &SegmentCommitInfo) {
    write_string(output, &segment.metadata.name);
    output.extend_from_slice(&segment.metadata.id);
    write_string(output, &segment.metadata.codec);
    write_be_i64(output, segment.del_gen);
    write_be_i32(output, segment.del_count);
    write_be_i64(output, segment.field_infos_gen);
    write_be_i64(output, segment.doc_values_gen);
    write_be_i32(output, segment.soft_del_count);
    match segment.sci_id {
        Some(id) => {
            output.push(1);
            output.extend_from_slice(&id);
        }
        None => output.push(0),
    }
    write_set_of_strings(output, &segment.field_infos_files);
    write_be_i32(output, segment.doc_values_update_files.len() as i32);
    for (field_number, files) in &segment.doc_values_update_files {
        write_be_i32(output, *field_number);
        write_set_of_strings(output, files);
    }
}

fn read_segment_commit_info(input: &[u8], position: &mut usize) -> Result<SegmentCommitInfo> {
    let metadata = SegmentMetadata {
        name: read_string(input, position)?,
        id: read_array::<16>(input, position)?,
        codec: read_string(input, position)?,
    };
    let del_gen = read_be_i64(input, position)?;
    let del_count = read_be_i32(input, position)?;
    let field_infos_gen = read_be_i64(input, position)?;
    let doc_values_gen = read_be_i64(input, position)?;
    let soft_del_count = read_be_i32(input, position)?;
    let sci_marker = read_byte(input, position)?;
    let sci_id = match sci_marker {
        0 => None,
        1 => Some(read_array::<16>(input, position)?),
        _ => {
            return Err(SegmentInfosError::InvalidSegmentCommitInfoIdMarker { marker: sci_marker });
        }
    };
    let field_infos_files = read_set_of_strings(input, position)?;
    let doc_values_update_file_count = read_be_i32(input, position)?;
    if doc_values_update_file_count < 0 {
        return Err(SegmentInfosError::UnexpectedEof);
    }
    let mut doc_values_update_files = BTreeMap::new();
    for _ in 0..doc_values_update_file_count {
        let field_number = read_be_i32(input, position)?;
        let files = read_set_of_strings(input, position)?;
        doc_values_update_files.insert(field_number, files);
    }

    Ok(SegmentCommitInfo {
        metadata,
        del_gen,
        del_count,
        field_infos_gen,
        doc_values_gen,
        soft_del_count,
        sci_id,
        field_infos_files,
        doc_values_update_files,
    })
}

fn read_map_of_strings(input: &[u8], position: &mut usize) -> Result<BTreeMap<String, String>> {
    let count = read_vint(input, position)?;
    if count < 0 {
        return Err(SegmentInfosError::UnsupportedUserData { count });
    }

    let mut map = BTreeMap::new();
    for _ in 0..count {
        let key = read_string(input, position)?;
        let value = read_string(input, position)?;
        map.insert(key, value);
    }
    Ok(map)
}

fn read_vint(input: &[u8], position: &mut usize) -> Result<i32> {
    let mut value = 0_u32;
    for shift in (0..32).step_by(7) {
        let byte = read_byte(input, position)?;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Ok(value as i32)
}

fn write_vlong(output: &mut Vec<u8>, mut value: u64) {
    while value & !0x7f != 0 {
        output.push(((value & 0x7f) | 0x80) as u8);
        value >>= 7;
    }
    output.push(value as u8);
}

fn read_vlong(input: &[u8], position: &mut usize) -> Result<u64> {
    let mut value = 0_u64;
    for shift in (0..64).step_by(7) {
        let byte = read_byte(input, position)?;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    Ok(value)
}

fn write_be_i32(output: &mut Vec<u8>, value: i32) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn read_be_i32(input: &[u8], position: &mut usize) -> Result<i32> {
    Ok(i32::from_be_bytes(read_array::<4>(input, position)?))
}

fn write_be_i64(output: &mut Vec<u8>, value: i64) {
    output.extend_from_slice(&value.to_be_bytes());
}

fn read_be_i64(input: &[u8], position: &mut usize) -> Result<i64> {
    Ok(i64::from_be_bytes(read_array::<8>(input, position)?))
}

fn read_byte_string(input: &[u8], position: &mut usize) -> Result<String> {
    let length = usize::from(read_byte(input, position)?);
    let end = position
        .checked_add(length)
        .ok_or(SegmentInfosError::UnexpectedEof)?;
    if end > input.len() {
        return Err(SegmentInfosError::UnexpectedEof);
    }
    let value = std::str::from_utf8(&input[*position..end])
        .map_err(|_| SegmentInfosError::UnexpectedEof)?
        .to_owned();
    *position = end;
    Ok(value)
}

fn read_byte(input: &[u8], position: &mut usize) -> Result<u8> {
    let Some(byte) = input.get(*position).copied() else {
        return Err(SegmentInfosError::UnexpectedEof);
    };
    *position += 1;
    Ok(byte)
}

fn read_array<const N: usize>(input: &[u8], position: &mut usize) -> Result<[u8; N]> {
    let end = position
        .checked_add(N)
        .ok_or(SegmentInfosError::UnexpectedEof)?;
    if end > input.len() {
        return Err(SegmentInfosError::UnexpectedEof);
    }
    let mut bytes = [0_u8; N];
    bytes.copy_from_slice(&input[*position..end]);
    *position = end;
    Ok(bytes)
}
