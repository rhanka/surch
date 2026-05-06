use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::field_infos::FieldInfos;
use crate::segment_field_infos::{write_segment_field_infos, SegmentFieldInfosError};
use crate::segment_infos::{
    SegmentCommitInfo, SegmentInfos, SegmentInfosCommit, SegmentInfosError, SegmentMetadata,
};

pub type Result<T> = std::result::Result<T, SegmentManifestError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SegmentManifestError {
    #[error("segment field infos error: {0}")]
    FieldInfos(#[from] SegmentFieldInfosError),
    #[error("segment infos error: {0}")]
    SegmentInfos(#[from] SegmentInfosError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentFile {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentManifest {
    pub files: Vec<SegmentFile>,
}

pub fn build_single_segment_manifest(
    segment_name: &str,
    field_infos: &FieldInfos,
    commit_id: &[u8; 16],
) -> Result<SegmentManifest> {
    let field_infos_file = write_segment_field_infos(segment_name, field_infos)?;
    let field_infos_file_name = field_infos_file.file_name.clone();

    let mut infos = SegmentInfos::new(10)?;
    infos.counter = 1;
    infos.segments.push(SegmentCommitInfo {
        metadata: SegmentMetadata {
            name: segment_name.to_owned(),
            id: *commit_id,
            codec: "Lucene90".to_owned(),
        },
        del_gen: -1,
        del_count: 0,
        field_infos_gen: -1,
        doc_values_gen: -1,
        soft_del_count: 0,
        sci_id: Some(*commit_id),
        field_infos_files: BTreeSet::from([field_infos_file_name]),
        doc_values_update_files: BTreeMap::new(),
    });
    let commit = infos.write_commit(commit_id)?;

    Ok(SegmentManifest {
        files: vec![
            SegmentFile {
                name: field_infos_file.file_name,
                bytes: field_infos_file.bytes,
            },
            segment_infos_file(commit),
        ],
    })
}

fn segment_infos_file(commit: SegmentInfosCommit) -> SegmentFile {
    SegmentFile {
        name: commit.file_name,
        bytes: commit.bytes,
    }
}
