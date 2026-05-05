use surch_index::segment_infos::{
    file_name_from_generation, generation_from_segments_file_name, get_last_commit_generation,
    SegmentInfos, SegmentInfosError, PENDING_SEGMENTS, SEGMENTS,
};

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
