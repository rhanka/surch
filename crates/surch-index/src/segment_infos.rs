use thiserror::Error;

pub const SEGMENTS: &str = "segments";
pub const PENDING_SEGMENTS: &str = "pending_segments";
pub const OLD_SEGMENTS_GEN: &str = "segments.gen";

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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfos {
    index_created_version_major: i32,
    generation: i64,
    last_generation: i64,
    pub counter: i64,
    pub version: i64,
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
