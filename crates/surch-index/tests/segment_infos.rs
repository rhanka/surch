use surch_index::segment_infos::{
    file_name_from_generation, generation_from_segments_file_name, get_last_commit_generation,
    SegmentInfos, SegmentInfosError, PENDING_SEGMENTS, SEGMENTS,
};

use std::collections::BTreeMap;

use surch_codec::codec_util::{check_footer, footer_length, write_footer, CODEC_MAGIC};

#[test]
fn segment_infos_parses_lucene_segments_generations() {
    assert_eq!(generation_from_segments_file_name("segments").unwrap(), 0);
    assert_eq!(generation_from_segments_file_name("segments_1").unwrap(), 1);
    assert_eq!(
        generation_from_segments_file_name("segments_a").unwrap(),
        10
    );
    assert_eq!(
        generation_from_segments_file_name("segments_z").unwrap(),
        35
    );
    assert_eq!(
        generation_from_segments_file_name("segments_10").unwrap(),
        36
    );
}

#[test]
fn segment_infos_rejects_invalid_segments_generation_names() {
    let err = generation_from_segments_file_name("segments.gen").expect_err("old segments.gen");
    assert!(matches!(err, SegmentInfosError::OldSegmentsGen));

    let err = generation_from_segments_file_name("not_segments_1").expect_err("not segments");
    assert!(matches!(
        err,
        SegmentInfosError::NotSegmentsFile {
            file_name
        } if file_name == "not_segments_1"
    ));

    let err = generation_from_segments_file_name("segments_").expect_err("empty generation");
    assert!(matches!(err, SegmentInfosError::InvalidGeneration { .. }));
}

#[test]
fn segment_infos_formats_lucene_generation_file_names() {
    assert_eq!(file_name_from_generation(SEGMENTS, "", -1), None);
    assert_eq!(
        file_name_from_generation(SEGMENTS, "", 0).as_deref(),
        Some("segments")
    );
    assert_eq!(
        file_name_from_generation(SEGMENTS, "", 1).as_deref(),
        Some("segments_1")
    );
    assert_eq!(
        file_name_from_generation(SEGMENTS, "", 36).as_deref(),
        Some("segments_10")
    );
    assert_eq!(
        file_name_from_generation(PENDING_SEGMENTS, "", 2).as_deref(),
        Some("pending_segments_2")
    );
    assert_eq!(
        file_name_from_generation("base", "ext", 0).as_deref(),
        Some("base.ext")
    );
    assert_eq!(
        file_name_from_generation("base", "ext", 35).as_deref(),
        Some("base_z.ext")
    );
}

#[test]
fn segment_infos_finds_latest_lucene_commit_generation() {
    let files = [
        "write.lock",
        "segments.gen",
        "segments",
        "segments_2",
        "segments_a",
        "pending_segments_z",
    ];

    assert_eq!(get_last_commit_generation(files).unwrap(), 10);
}

#[test]
fn segment_infos_tracks_next_pending_generation() {
    let mut infos = SegmentInfos::new(9).expect("segment infos");

    assert_eq!(infos.generation(), 0);
    assert_eq!(infos.last_generation(), 0);
    assert_eq!(infos.get_segments_file_name().as_deref(), Some("segments"));
    assert_eq!(infos.get_next_pending_generation(), 1);

    infos
        .set_next_write_generation(4)
        .expect("increase generation");

    assert_eq!(infos.generation(), 4);
    assert_eq!(infos.get_next_pending_generation(), 5);
}

#[test]
fn segment_infos_rejects_decreasing_write_generation() {
    let mut infos = SegmentInfos::new(9).expect("segment infos");
    infos
        .set_next_write_generation(4)
        .expect("increase generation");

    let err = infos
        .set_next_write_generation(3)
        .expect_err("decrease generation");

    assert!(matches!(
        err,
        SegmentInfosError::GenerationDecrease {
            requested: 3,
            current: 4
        }
    ));
}

#[test]
fn segment_infos_writes_and_reads_empty_lucene_commit() {
    let mut infos = SegmentInfos::new(10).expect("segment infos");
    infos.version = 3;
    infos.counter = 7;
    let id = [0x42; 16];

    let commit = infos.write_empty_commit(&id).expect("write commit");

    assert_eq!(commit.file_name, "segments_1");
    assert_eq!(infos.generation(), 1);
    assert_eq!(infos.last_generation(), 1);
    assert_eq!(&commit.bytes[..4], &CODEC_MAGIC.to_be_bytes());
    check_footer(&commit.bytes).expect("valid footer");

    let decoded =
        SegmentInfos::read_empty_commit(&commit.file_name, &commit.bytes).expect("read commit");

    assert_eq!(decoded.generation(), 1);
    assert_eq!(decoded.last_generation(), 1);
    assert_eq!(decoded.index_created_version_major(), 10);
    assert_eq!(decoded.version, 3);
    assert_eq!(decoded.counter, 7);
    assert_eq!(decoded.id(), Some(id));
    assert_eq!(decoded.lucene_version(), Some((11, 0, 0)));
}

#[test]
fn segment_infos_rejects_empty_commit_with_corrupt_footer() {
    let mut infos = SegmentInfos::new(10).expect("segment infos");
    let id = [0x24; 16];
    let mut commit = infos.write_empty_commit(&id).expect("write commit");
    let last = commit.bytes.len() - 1;
    commit.bytes[last] ^= 1;

    let err = SegmentInfos::read_empty_commit(&commit.file_name, &commit.bytes)
        .expect_err("corrupt footer");

    assert!(matches!(err, SegmentInfosError::Codec(_)));
}

#[test]
fn segment_infos_rejects_empty_commit_with_trailing_body_bytes() {
    let mut infos = SegmentInfos::new(10).expect("segment infos");
    let id = [0x18; 16];
    let mut commit = infos.write_empty_commit(&id).expect("write commit");
    let _footer = commit.bytes.split_off(commit.bytes.len() - footer_length());
    commit.bytes.push(0xaa);
    write_footer(&mut commit.bytes);

    let err = SegmentInfos::read_empty_commit(&commit.file_name, &commit.bytes)
        .expect_err("trailing body bytes");

    assert!(matches!(err, SegmentInfosError::TrailingBytes { count: 1 }));
}

#[test]
fn segment_infos_round_trips_commit_user_data() {
    let mut infos = SegmentInfos::new(10).expect("segment infos");
    infos.user_data = BTreeMap::from([
        ("opaque".to_owned(), "client-value".to_owned()),
        ("source".to_owned(), "surch".to_owned()),
    ]);
    let id = [0x33; 16];

    let commit = infos.write_empty_commit(&id).expect("write commit");
    let decoded =
        SegmentInfos::read_empty_commit(&commit.file_name, &commit.bytes).expect("read commit");

    assert_eq!(decoded.user_data, infos.user_data);
}
