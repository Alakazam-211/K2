//! The records-only walker: against the full reader, on bounds and limits, and across readers.

mod support;

use std::io::{self, Read};

use k2_mpc_psd::differential::{self as diff, compare_with_full, walk_stepped, Recording, Stepped};
use k2_mpc_psd::{walk_records, LayerSource, WalkError, WalkLimits, WalkOptions, Walked};
use photocraft_psd::error::PsdError;
use photocraft_psd::testgen;
use photocraft_psd::PsdFile;
use support::*;

fn opts() -> WalkOptions<'static> {
    WalkOptions {
        keep_resources: diff::KEEP_RESOURCES,
        ..Default::default()
    }
}

fn walk(data: &[u8]) -> Result<Walked, WalkError> {
    walk_stepped(data, usize::MAX, &opts()).0
}

fn walk_with(data: &[u8], limits: WalkLimits) -> (Result<Walked, WalkError>, Recording) {
    walk_stepped(
        data,
        usize::MAX,
        &WalkOptions {
            limits,
            keep_resources: diff::KEEP_RESOURCES,
        },
    )
}

fn assert_full_agrees(data: &[u8]) {
    PsdFile::from_bytes(data).expect("check: the full reader accepts this file");
    compare_with_full(data).unwrap_or_else(|e| panic!("walk and full reader differ: {e}"));
}

#[test]
fn every_generated_file_matches_the_full_reader() {
    let mut layers = 0usize;
    let mut bytes_total = 0usize;
    for case in testgen::all_cases() {
        let bytes = case
            .file
            .to_bytes()
            .expect("check: generated file serializes");
        compare_with_full(&bytes).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        layers += walk(&bytes)
            .expect("check: generated files walk")
            .layers
            .len();
        bytes_total += bytes.len();
    }
    assert!(
        layers >= 100,
        "the generator should exercise real layers, walked {layers}"
    );
    eprintln!("generated files: {bytes_total} bytes, {layers} layers");
}

#[test]
fn how_the_reader_splits_the_stream_never_matters() {
    for (i, case) in testgen::all_cases()
        .iter()
        .enumerate()
        .filter(|(i, _)| i % 7 == 0)
    {
        let bytes = case
            .file
            .to_bytes()
            .expect("check: generated file serializes");
        let (base, base_sink) = walk_stepped(&bytes, usize::MAX, &opts());
        let base = base.unwrap_or_else(|e| panic!("{}: {e}", case.name));
        for step in [1usize, 2, 3, 7, 13, 4096, 70_000] {
            let (got, sink) = walk_stepped(&bytes, step, &opts());
            let got = got.unwrap_or_else(|e| panic!("{} step {step}: {e}", case.name));
            assert_eq!(got, base, "case {i} step {step}: outcome");
            assert_eq!(
                sink.channels, base_sink.channels,
                "case {i} step {step}: channel bytes"
            );
            assert_eq!(
                sink.records, base_sink.records,
                "case {i} step {step}: records"
            );
        }
    }
}

#[test]
fn channel_offsets_point_at_the_encoded_bytes() {
    let mut seen = 0;
    for case in testgen::all_cases()
        .iter()
        .filter(|c| c.name.starts_with("small"))
    {
        let bytes = case
            .file
            .to_bytes()
            .expect("check: generated file serializes");
        let w = walk(&bytes).expect("check: generated files walk");
        for (l, want) in w.layers.iter().zip(
            &case
                .file
                .layer_info
                .as_ref()
                .expect("check: layered")
                .layers,
        ) {
            for (c, wc) in l.channels.iter().zip(&want.channels) {
                if wc.compression.is_none() {
                    continue;
                }
                let at = c.data_offset as usize;
                assert_eq!(
                    &bytes[at..at + wc.data.len()],
                    &wc.data[..],
                    "{}",
                    case.name
                );
                seen += 1;
            }
        }
    }
    assert!(seen > 0, "no channels checked");
}

#[test]
fn a_prefix_of_the_section_is_truncated_never_partial() {
    for case in testgen::all_cases().iter().filter(|c| {
        c.name.starts_with("small") || c.name.contains("layered Psb Grayscale 16bit Raw")
    }) {
        let bytes = case
            .file
            .to_bytes()
            .expect("check: generated file serializes");
        let whole = walk(&bytes).expect("check: generated files walk");
        let stride = if case.name.starts_with("small") {
            1
        } else {
            31
        };
        for cut in (0..whole.consumed as usize).step_by(stride) {
            match walk(&bytes[..cut]) {
                Err(WalkError::Truncated { offset }) => assert!(
                    offset <= cut as u64,
                    "{} cut {cut}: offset {offset}",
                    case.name
                ),
                other => panic!("{} cut {cut}: expected Truncated, got {other:?}", case.name),
            }
        }
        // The merged image needn't be there: the walk stops at the section's end.
        let got = walk(&bytes[..whole.consumed as usize])
            .unwrap_or_else(|e| panic!("{}: cut at the section end: {e}", case.name));
        assert_eq!(got, whole, "{}: cut at the section end", case.name);
    }
}

#[test]
fn empty_and_absent_sections_walk() {
    for psb in [false, true] {
        let w = walk(&hand_file(psb, 8, &[], &[])).unwrap();
        assert_eq!(w.source, LayerSource::None);
        assert!(w.layers.is_empty());
        let w = walk(&hand_file(psb, 8, &[], &section(psb, &[], &[], &[]))).unwrap();
        assert_eq!(
            w.source,
            LayerSource::None,
            "a zero layer info length holds no layer info"
        );
        assert_full_agrees(&hand_file(psb, 8, &[], &section(psb, &[], &[], &[])));
    }
}

#[test]
fn walk_stops_at_the_end_of_the_section() {
    let psb = false;
    let lm = section(
        psb,
        &layer_info_one(psb, b"one", 2, 7),
        &[0u8; 6],
        &block(b"Patt", &[], &[]),
    );
    let file = hand_file(psb, 8, &[], &lm);
    let w = walk(&file).unwrap();
    assert_eq!(
        w.consumed as usize,
        file.len() - 3,
        "stops before the merged image"
    );
    assert_full_agrees(&file);
}

#[test]
fn hand_built_records_match_the_full_reader() {
    // Odd-length names (padding), PSD and PSB, a kept and a skipped resource.
    for psb in [false, true] {
        let res = [
            resource(1005, b"", &[0u8; 16]),
            resource(4000, b"abc", b"xyz"),
            resource(1044, b"n", &1u32.to_be_bytes()),
        ]
        .concat();
        let li = layer_info_one(psb, b"hello", 2, 9);
        let lm = section(psb, &li, &[0u8; 2], &[]);
        let file = hand_file(psb, 8, &res, &lm);
        assert_full_agrees(&file);
        let w = walk(&file).unwrap();
        assert_eq!(w.layers.len(), 1);
        assert_eq!(w.source, LayerSource::Section);
        assert_eq!(
            w.resources.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![1005, 1044],
            "kept the asked-for ids only"
        );
        assert_eq!(w.resources[1].name, b"n", "kept the resource's name");
    }
}

#[test]
fn a_layer_info_length_past_the_section_is_format() {
    let psb = false;
    let mut lm = section(psb, &[0u8; 2], &[], &[]);
    lm[..4].copy_from_slice(&be32(100_000));
    let mut file = hand_file(psb, 8, &[], &lm);
    file.extend(vec![0u8; 200_000]); // stream continues: only the section's bound can refuse it
    assert!(
        matches!(
            walk(&file),
            Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
        ),
        "{:?}",
        walk(&file).err()
    );
}

#[test]
fn a_record_extra_length_past_the_layer_info_is_format() {
    let psb = false;
    let mut li = 1i16.to_be_bytes().to_vec();
    let rec = record(psb, &[], &extra(b"x", &[]));
    li.extend_from_slice(&rec);
    let at = 2 + 16 + 2 + 16 - 4; // the extra-length field
    li[at..at + 4].copy_from_slice(&be32(40_000));
    let mut file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    file.extend(vec![0u8; 100_000]);
    assert!(
        matches!(
            walk(&file),
            Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
        ),
        "{:?}",
        walk(&file).err()
    );
}

#[test]
fn a_channel_length_past_the_layer_info_is_format() {
    for (psb, claimed) in [(false, 500_000u64), (true, 500_000), (true, u64::MAX)] {
        let li = layer_info(
            1,
            &[record(psb, &[(0, claimed)], &extra(b"x", &[]))],
            &[vec![0u8, 0, 9]],
        );
        let lm = section(psb, &li, &[], &[]);
        let mut file = hand_file(psb, 8, &[], &lm);
        file.extend(vec![0u8; 700_000]);
        let (out, sink) = walk_with(&file, WalkLimits::default());
        assert!(
            matches!(out, Err(WalkError::Format(PsdError::UnexpectedEof { .. }))),
            "psb {psb} claimed {claimed}: {:?}",
            out.err()
        );
        assert!(sink.channels.is_empty(), "no channel starts past its bound");
    }
}

#[test]
fn a_channel_of_length_one_is_format() {
    let psb = false;
    let li = layer_info(
        1,
        &[record(psb, &[(0, 1)], &extra(b"x", &[]))],
        &[vec![0u8]],
    );
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    assert!(
        matches!(walk(&file), Err(WalkError::Format(PsdError::Invalid(_)))),
        "{:?}",
        walk(&file).err()
    );
}

#[test]
fn a_layer_count_that_cannot_fit_is_refused_before_any_record() {
    let psb = false;
    let mut li = 1000i16.to_be_bytes().to_vec();
    li.extend_from_slice(&record(psb, &[], &extra(b"x", &[])));
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    let (out, sink) = walk_with(&file, WalkLimits::default());
    assert!(
        matches!(out, Err(WalkError::Format(PsdError::UnexpectedEof { .. }))),
        "{:?}",
        out.err()
    );
    assert!(
        sink.records.is_empty(),
        "no record is read when the count can't fit"
    );
}

#[test]
fn more_than_the_channel_limit_is_format_and_read_no_further() {
    let psb = false;
    let mut li = 1i16.to_be_bytes().to_vec();
    li.extend_from_slice(&[0u8; 16]);
    li.extend(be16(65_535)); // far more than fits: the limit refuses it before the table is read
    li.extend(vec![0u8; 500]);
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    assert!(
        matches!(
            walk(&file),
            Err(WalkError::Format(PsdError::LimitExceeded(_)))
        ),
        "{:?}",
        walk(&file).err()
    );
}

#[test]
fn over_the_record_limit_refuses_before_holding_it() {
    let psb = false;
    let li = layer_info_one(psb, b"big", 1, 0);
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    let limits = WalkLimits {
        max_record_bytes: 20,
        ..Default::default()
    };
    let (out, sink) = walk_with(&file, limits);
    assert!(
        matches!(out, Err(WalkError::Limit("a layer record is too large"))),
        "{:?}",
        out.err()
    );
    assert!(
        sink.records.is_empty(),
        "the oversized record is never given to the sink"
    );
}

#[test]
fn a_claimed_extra_past_the_region_is_a_format_error_not_a_limit() {
    // A record claiming more extra data than its layer info holds is the file's own error, as in
    // the slice parser, however large the claim: the limit is only for records that are really
    // there.
    let psb = false;
    let mut li = 1i16.to_be_bytes().to_vec();
    li.extend_from_slice(&record(psb, &[], &[]));
    let at = 2 + 16 + 2 + 16 - 4;
    li[at..at + 4].copy_from_slice(&be32(4_000_000_000));
    li.extend(vec![0u8; 2_000]);
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    let (out, _sink) = walk_with(&file, WalkLimits::default());
    assert!(
        matches!(out, Err(WalkError::Format(PsdError::UnexpectedEof { .. }))),
        "{:?}",
        out.err()
    );
}

#[test]
fn a_layer_block_with_a_claimed_extra_past_the_block_reads_as_flattened() {
    // In a depth 16 file, the bad block must be unreadable, as in the full reader, not refuse the
    // whole file with a limit.
    let psb = false;
    let mut block_li = 1i16.to_be_bytes().to_vec();
    block_li.extend_from_slice(&record(psb, &[], &[]));
    let at = 2 + 16 + 2 + 16 - 4;
    block_li[at..at + 4].copy_from_slice(&be32(4_000_000_000));
    let file = hand_file(
        psb,
        16,
        &[],
        &section(psb, &[], &[], &block(b"Lr16", &block_li, &[])),
    );
    assert_full_agrees(&file);
    let w = walk(&file).expect("check: an unreadable layer block is not a refusal");
    assert!(w.layers.is_empty());
    assert!(w.unreadable.is_some(), "the walk says the layers were lost");
}

#[test]
fn over_the_total_limit_stops_where_it_goes_over() {
    let psb = false;
    let recs: Vec<Vec<u8>> = (0..4)
        .map(|_| record(psb, &[], &extra(b"abcd", &[])))
        .collect();
    let one = recs[0].len() as u64;
    let li = layer_info(4, &recs, &[]);
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    let (out, sink) = walk_with(
        &file,
        WalkLimits {
            max_total_record_bytes: 2 * one,
            ..Default::default()
        },
    );
    assert!(
        matches!(
            out,
            Err(WalkError::Limit("the layer records are too large"))
        ),
        "{:?}",
        out.err()
    );
    assert_eq!(sink.records.len(), 2, "records up to the limit were seen");
    let (ok, _) = walk_with(
        &file,
        WalkLimits {
            max_total_record_bytes: 4 * one,
            ..Default::default()
        },
    );
    assert!(ok.is_ok(), "exactly the limit passes: {:?}", ok.err());
}

#[test]
fn over_the_layer_limit_is_refused() {
    let psb = false;
    let li = layer_info(
        2,
        &[
            record(psb, &[], &extra(b"a", &[])),
            record(psb, &[], &extra(b"b", &[])),
        ],
        &[],
    );
    let file = hand_file(psb, 8, &[], &section(psb, &li, &[], &[]));
    let (out, sink) = walk_with(
        &file,
        WalkLimits {
            max_layers: 1,
            ..Default::default()
        },
    );
    assert!(
        matches!(out, Err(WalkError::Limit("too many layers"))),
        "{:?}",
        out.err()
    );
    assert!(sink.records.is_empty());
    let (ok, _) = walk_with(
        &file,
        WalkLimits {
            max_layers: 2,
            ..Default::default()
        },
    );
    assert!(ok.is_ok());
}

#[test]
fn over_the_kept_resource_limit_is_refused() {
    let res = resource(1044, b"", &[1u8; 100]);
    let file = hand_file(false, 8, &res, &[]);
    let (out, _) = walk_with(
        &file,
        WalkLimits {
            max_kept_resource_bytes: 99,
            ..Default::default()
        },
    );
    assert!(
        matches!(
            out,
            Err(WalkError::Limit("a kept image resource is too large"))
        ),
        "{:?}",
        out.err()
    );
    let (ok, _) = walk_with(
        &file,
        WalkLimits {
            max_kept_resource_bytes: 100,
            ..Default::default()
        },
    );
    assert_eq!(
        ok.expect("check: exactly the limit passes").resources[0]
            .data
            .len(),
        100
    );
}

#[test]
fn a_skipped_resource_never_needs_room() {
    // A resource nobody asked for may be larger than the kept-resource limit.
    let res = resource(4000, b"", &[1u8; 100_000]);
    let file = hand_file(false, 8, &res, &[]);
    let (ok, _) = walk_with(
        &file,
        WalkLimits {
            max_kept_resource_bytes: 16,
            ..Default::default()
        },
    );
    assert!(ok.is_ok(), "{:?}", ok.err());
}

#[test]
fn resource_bounds_are_format() {
    // Longer than its section.
    let mut res = resource(1005, b"", &[0u8; 16]);
    let len = res.len();
    res[8..12].copy_from_slice(&be32(1_000));
    assert!(
        matches!(
            walk(&hand_file(false, 8, &res, &[])),
            Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
        ),
        "len {len}"
    );
    // A bad signature.
    let mut res = resource(1005, b"", &[]);
    res[..4].copy_from_slice(b"XXXX");
    assert!(matches!(
        walk(&hand_file(false, 8, &res, &[])),
        Err(WalkError::Format(PsdError::InvalidSignature { .. }))
    ));
    // Odd data whose pad byte is missing.
    let mut res = b"8BIM".to_vec();
    res.extend(be16(1005));
    res.extend([0u8, 0]);
    res.extend(be32(1));
    res.push(9);
    assert!(matches!(
        walk(&hand_file(false, 8, &res, &[])),
        Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
    ));
    // A kept resource claiming more than its section holds: the file's own error, not a limit.
    let mut res = resource(1005, b"", &[0u8; 16]);
    res[8..12].copy_from_slice(&be32(4_000_000_000));
    assert!(matches!(
        walk(&hand_file(false, 8, &res, &[])),
        Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
    ));
    // Name padding that overruns the section.
    let mut res = b"8BIM".to_vec();
    res.extend(be16(1005));
    res.push(0);
    assert!(matches!(
        walk(&hand_file(false, 8, &res, &[])),
        Err(WalkError::Format(PsdError::UnexpectedEof { .. }))
    ));
}

#[test]
fn a_bad_magic_is_format() {
    let mut file = hand_file(false, 8, &[], &[]);
    file[0] = b'X';
    assert!(matches!(
        walk(&file),
        Err(WalkError::Format(PsdError::InvalidSignature { .. }))
    ));
}

#[test]
fn a_depth_16_layer_block_can_be_unreadable_and_says_so() {
    let psb = false;
    let garbage = 1i16.to_be_bytes().to_vec(); // a layer count with no record behind it
    let blocks = [
        block(b"Lr16", &garbage, &[]),
        block(b"Patt", &[1, 2], &[0, 0]),
    ]
    .concat();
    let lm = section(psb, &[], &[], &blocks);
    let file = hand_file(psb, 16, &[], &lm);
    let full = PsdFile::from_bytes(&file).expect("check: the full reader reads it as flattened");
    assert!(full.layer_info.is_none(), "the full reader lost the layers");
    assert!(
        full.unreadable_layer_block().is_some(),
        "the full reader says so"
    );
    let (out, sink) = walk_with(&file, WalkLimits::default());
    let w = out.expect("check: an unreadable layer block is not a refusal");
    assert_full_agrees(&file);
    assert!(w.layers.is_empty());
    assert_eq!(w.source, LayerSource::None);
    assert!(
        w.unreadable.is_some(),
        "the walk must say the layers were lost"
    );
    assert_eq!(sink.resets, 2, "reset before trying, and after failing");
    assert_eq!(
        w.consumed as usize,
        file.len() - 4,
        "kept going past the bad block"
    );
}

#[test]
fn a_depth_16_layer_block_replaces_the_section_layers() {
    let psb = false;
    let section_li = layer_info_one(psb, b"section", 1, 1);
    let block_li = layer_info(
        2,
        &[
            record(psb, &[(0, 3)], &extra(b"block-a", &[])),
            record(psb, &[(0, 3)], &extra(b"block-b", &[])),
        ],
        &[vec![0, 0, 5], vec![0, 0, 6]],
    );
    let lm = section(psb, &section_li, &[], &block(b"Lr16", &block_li, &[0; 2]));
    let file = hand_file(psb, 16, &[], &lm);
    assert_full_agrees(&file);
    let (out, sink) = walk_with(&file, WalkLimits::default());
    let w = out.unwrap();
    assert_eq!(w.layers.len(), 2);
    assert_eq!(
        w.source,
        LayerSource::GlobalBlock {
            index: 0,
            key: *b"Lr16"
        }
    );
    assert_eq!(sink.resets, 1);
    assert_eq!(
        sink.channels.len(),
        2,
        "only the block's channels are left at the sink"
    );
    assert_eq!(sink.channels[&(1, 0)].data, vec![6u8]);
}

#[test]
fn an_unreadable_depth_16_block_clears_the_section_layers() {
    let psb = false;
    let section_li = layer_info_one(psb, b"section", 1, 1);
    let lm = section(
        psb,
        &section_li,
        &[],
        &block(b"Lr16", &1i16.to_be_bytes(), &[0; 2]),
    );
    let file = hand_file(psb, 16, &[], &lm);
    assert_full_agrees(&file);
    let (out, sink) = walk_with(&file, WalkLimits::default());
    let w = out.unwrap();
    assert!(
        w.layers.is_empty(),
        "as the full reader: the failed block empties the layers"
    );
    assert_eq!(w.source, LayerSource::None);
    assert!(w.unreadable.is_some());
    assert_eq!(sink.resets, 2);
    assert!(sink.channels.is_empty(), "nothing is left at the sink");
}

#[test]
fn only_the_first_candidate_block_is_read() {
    let psb = false;
    let good = layer_info_one(psb, b"first", 1, 1);
    let bad = 1i16.to_be_bytes().to_vec();
    let blocks = [
        block(b"Lr16", &good, &[0; 2]),
        block(b"Lr16", &bad, &[0; 2]),
    ]
    .concat();
    let file = hand_file(psb, 16, &[], &section(psb, &[], &[], &blocks));
    assert_full_agrees(&file);
    let w = walk(&file).unwrap();
    assert_eq!(w.layers.len(), 1);
    assert_eq!(w.layers[0].record.name, b"first".to_vec());
    assert!(w.unreadable.is_none());
}

#[test]
fn an_8_bit_file_reads_a_layer_block_only_when_the_section_has_no_layers() {
    let psb = false;
    let empty_li = 0i16.to_be_bytes().to_vec();
    let block_li = layer_info_one(psb, b"blocked", 1, 4);
    // Section layer info present but empty: the block supplies the layers.
    let file = hand_file(
        psb,
        8,
        &[],
        &section(psb, &empty_li, &[], &block(b"Layr", &block_li, &[0; 2])),
    );
    assert_full_agrees(&file);
    assert_eq!(
        walk(&file).unwrap().source,
        LayerSource::GlobalBlock {
            index: 0,
            key: *b"Layr"
        }
    );
    // The section has layers: the block stays an ordinary block.
    let section_li = layer_info_one(psb, b"section", 1, 2);
    let file = hand_file(
        psb,
        8,
        &[],
        &section(psb, &section_li, &[], &block(b"Layr", &block_li, &[0; 2])),
    );
    assert_full_agrees(&file);
    let w = walk(&file).unwrap();
    assert_eq!(w.source, LayerSource::Section);
    assert_eq!(w.layers[0].record.name, b"section".to_vec());
}

#[test]
fn psb_blocks_use_long_lengths_for_the_long_keys() {
    let psb = true;
    let li = layer_info_one(psb, b"in-psb", 1, 8);
    let blocks = [
        long_block(b"Lr16", &li, &[0; 2]),
        block(b"Patt", &[1, 2, 3, 4], &[]),
    ]
    .concat();
    let file = hand_file(psb, 16, &[], &section(psb, &[], &[], &blocks));
    assert_full_agrees(&file);
    let w = walk(&file).unwrap();
    assert_eq!(w.layers.len(), 1);
    assert_eq!(
        w.source,
        LayerSource::GlobalBlock {
            index: 0,
            key: *b"Lr16"
        }
    );
}

/// The gap (at most three bytes) before a following block, checked by putting a layer block
/// behind each: a wrong gap loses it. Each case also goes through the full reader.
#[test]
fn a_gap_before_the_next_block_keeps_the_layer_block_reachable() {
    let psb = false;
    let li = layer_info_one(psb, b"after-gap", 1, 3);
    let cases: [(&str, Vec<u8>); 5] = [
        ("no gap", block(b"odd1", &[1], &[])),
        ("gap 1", block(b"odd1", &[1], &[0; 1])),
        ("gap 2", block(b"odd1", &[1, 2], &[0; 2])),
        ("gap 3", block(b"odd1", &[1], &[0; 3])),
        ("gap of nonzero bytes", block(b"odd1", &[1], &[9, 9, 9])),
    ];
    for (name, first) in cases {
        let blocks = [first, block(b"Layr", &li, &[0; 2])].concat();
        let file = hand_file(psb, 8, &[], &section(psb, &[], &[], &blocks));
        assert_full_agrees(&file);
        for step in [1usize, 2, 5, usize::MAX] {
            let (out, _) = walk_stepped(&file, step, &opts());
            let w = out.unwrap_or_else(|e| panic!("{name} step {step}: {e}"));
            assert_eq!(
                w.source,
                LayerSource::GlobalBlock {
                    index: 1,
                    key: *b"Layr"
                },
                "{name} step {step}: the layer block"
            );
            assert_eq!(w.layers.len(), 1, "{name} step {step}");
        }
    }
}

/// Ways the blocks end; every one must walk to the end of the section and agree with the full
/// reader.
#[test]
fn every_way_the_blocks_end_reaches_the_end_of_the_section() {
    let psb = false;
    let cases: [(&str, Vec<u8>); 10] = [
        ("right at the end", block(b"last", &[1, 2, 3, 4], &[])),
        (
            "three padding bytes to the end",
            block(b"last", &[1], &[0; 3]),
        ),
        (
            "two padding bytes to the end",
            block(b"last", &[1], &[0; 2]),
        ),
        (
            "zero padding then opaque bytes",
            [block(b"odd1", &[1], &[0; 3]), vec![9; 12]].concat(),
        ),
        (
            "nonzero bytes where padding would be",
            [block(b"odd1", &[1], &[]), vec![7; 12]].concat(),
        ),
        (
            "a foreign-order signature",
            [
                block(b"odd1", &[1], &[0; 1]),
                b"MIB8pmnt".to_vec(),
                be32(0).to_vec(),
            ]
            .concat(),
        ),
        (
            "short trailing bytes",
            [block(b"odd1", &[1, 2, 3, 4], &[]), vec![1, 2, 3]].concat(),
        ),
        (
            "a signature in the last six bytes",
            [block(b"odd1", &[1, 2, 3, 4], &[]), b"8BIMxx".to_vec()].concat(),
        ),
        (
            "a signature in the last eight bytes",
            [block(b"odd1", &[1, 2, 3, 4], &[]), b"8BIMabcd".to_vec()].concat(),
        ),
        (
            "a signature in the last eleven bytes",
            [block(b"odd1", &[1, 2, 3, 4], &[]), b"8BIMabcdefg".to_vec()].concat(),
        ),
    ];
    for (name, blocks) in cases {
        let file = hand_file(psb, 8, &[], &section(psb, &[], &[], &blocks));
        PsdFile::from_bytes(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        compare_with_full(&file).unwrap_or_else(|e| panic!("{name}: {e}"));
        for step in [1usize, 2, 5, usize::MAX] {
            let (out, _) = walk_stepped(&file, step, &opts());
            let w = out.unwrap_or_else(|e| panic!("{name} step {step}: {e}"));
            assert_eq!(
                w.consumed as usize,
                file.len() - 3,
                "{name} step {step}: reaches the end of the section"
            );
        }
    }
}

#[test]
fn a_block_length_past_the_section_is_format() {
    let psb = false;
    for (depth, key) in [(8u16, b"Patt"), (16, b"Lr16")] {
        let mut lm = section(psb, &[], &[], &block(key, &[], &[]));
        let at = 8 + 8; // past the layer-info and mask lengths, signature and key
        lm[at..at + 4].copy_from_slice(&be32(400_000));
        let mut file = hand_file(psb, depth, &[], &lm);
        file.extend(vec![0u8; 500_000]); // only the section's bound can refuse it
        let out = walk(&file);
        assert!(
            matches!(out, Err(WalkError::Format(PsdError::UnexpectedEof { .. }))),
            "{}: {:?}",
            String::from_utf8_lossy(key),
            out.err()
        );
    }
}

#[test]
fn a_kept_layer_block_does_not_count_for_the_block_index() {
    // A depth 16 file: an ordinary block, then the layer block: index 1, as the full reader says.
    let psb = false;
    let li = layer_info_one(psb, b"x", 1, 1);
    let blocks = [block(b"Patt", &[], &[]), block(b"Lr16", &li, &[0; 2])].concat();
    let file = hand_file(psb, 16, &[], &section(psb, &[], &[], &blocks));
    assert_full_agrees(&file);
    assert_eq!(
        walk(&file).unwrap().source,
        LayerSource::GlobalBlock {
            index: 1,
            key: *b"Lr16"
        }
    );
}

/// A reader that fails at offset `fail_at`.
struct FailAt<'a> {
    data: &'a [u8],
    at: usize,
    fail_at: usize,
    kind: io::ErrorKind,
}

impl Read for FailAt<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.at >= self.fail_at {
            return Err(io::Error::new(self.kind, "planted"));
        }
        let end = self.fail_at.min(self.data.len());
        let n = out.len().min(end - self.at);
        out[..n].copy_from_slice(&self.data[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

#[test]
fn a_failing_reader_is_a_read_error_not_a_bad_file() {
    let file = testgen::small(
        photocraft_psd::Version::Psb,
        photocraft_psd::Compression::Raw,
    )
    .to_bytes()
    .expect("check: serializes");
    let whole = walk(&file).expect("check: walks");
    for at in (0..whole.consumed as usize).step_by(3) {
        let mut src = FailAt {
            data: &file,
            at: 0,
            fail_at: at,
            kind: io::ErrorKind::PermissionDenied,
        };
        match walk_records(&mut src, &opts(), &mut ()) {
            Err(WalkError::Read(e)) => assert_eq!(e.kind(), io::ErrorKind::PermissionDenied),
            other => panic!("at {at}: expected a read error, got {other:?}"),
        }
    }
    let mut src = FailAt {
        data: &file,
        at: 0,
        fail_at: whole.consumed as usize,
        kind: io::ErrorKind::PermissionDenied,
    };
    let got = walk_records(&mut src, &opts(), &mut ())
        .unwrap_or_else(|e| panic!("reading exactly the section must be enough: {e}"));
    assert_eq!(got, whole);
}

#[test]
fn interrupted_reads_are_retried() {
    struct Flaky<'a> {
        data: &'a [u8],
        interrupt_next: bool,
    }
    impl Read for Flaky<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            self.interrupt_next = !self.interrupt_next;
            if self.interrupt_next {
                return Err(io::Error::from(io::ErrorKind::Interrupted));
            }
            let n = out.len().min(self.data.len()).min(5);
            out[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }
    let file = testgen::small(
        photocraft_psd::Version::Psd,
        photocraft_psd::Compression::Rle,
    )
    .to_bytes()
    .expect("check: serializes");
    let want = walk(&file).expect("check: walks");
    let mut src = Flaky {
        data: &file,
        interrupt_next: false,
    };
    let got =
        walk_records(&mut src, &opts(), &mut ()).expect("check: interrupted reads are retried");
    assert_eq!(got, want);
    let mut stepped = Stepped {
        data: &file,
        step: 1,
    };
    let got = walk_records(&mut stepped, &opts(), &mut ()).expect("check: one byte at a time");
    assert_eq!(got, want);
}
