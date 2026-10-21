use super::super::entities::{
    decode_entities, decode_positioned_entities, encode_entities, EntityTables,
};
use super::*;
use crate::io::dwg::crc::crc16;

const ENTITY_BASE: u32 = 0x1000;
const BLOCK_BASE: u32 = 0x2000;
const EXTRA_BASE: u32 = 0x3000;

fn tables() -> EntityTables {
    EntityTables {
        layers: vec!["0".into()],
        styles: vec!["STANDARD".into()],
        blocks: vec!["B".into()],
        ..EntityTables::default()
    }
}

fn frame(kind: u8, flag: u8, body: &[u8], padding: usize) -> Vec<u8> {
    let size = (4 + body.len() + 2 + padding) as u16;
    let mut raw = vec![kind, flag];
    raw.extend(size.to_le_bytes());
    raw.extend(body);
    raw.extend(crc16(0xc0c1, &raw).to_le_bytes());
    raw.resize(size as usize, 0xee);
    raw
}

fn common(kind: u8, flag: u8, geometry: &[u8]) -> Vec<u8> {
    let mut body = vec![0; 4];
    body.extend(geometry);
    frame(kind, flag, &body, 0)
}

fn jump(address: u32, padding: usize) -> Vec<u8> {
    frame(18, 0, &address.to_le_bytes(), padding)
}

fn vertex(x: f64) -> Vec<u8> {
    let mut geometry = Vec::from(x.to_le_bytes());
    geometry.extend(2_f64.to_le_bytes());
    common(20, 0, &geometry)
}

fn point(x: f64) -> Vec<u8> {
    let mut geometry = Vec::from(x.to_le_bytes());
    geometry.extend(2_f64.to_le_bytes());
    geometry.extend(0_f64.to_le_bytes());
    common(2, 0, &geometry)
}

fn seqend(owner: u32) -> Vec<u8> {
    common(17, 0, &owner.to_le_bytes())
}

fn insert() -> Vec<u8> {
    let mut geometry = vec![0; 2];
    geometry.extend(0_f64.to_le_bytes());
    geometry.extend(0_f64.to_le_bytes());
    common(14, 128, &geometry)
}

fn attrib() -> Vec<u8> {
    let mut geometry = vec![0; 16];
    geometry.extend(1_f64.to_le_bytes());
    geometry.extend([1, 0, b'V', 1, 0, b'T', 0]);
    common(16, 0, &geometry)
}

fn section(base: u32, raw: Vec<u8>) -> SectionRecords {
    SectionRecords {
        base_address: base,
        size: raw.len() as u32,
        records: decode_positioned_entities(&raw, &tables(), base, "ANSI_1252").unwrap(),
    }
}

fn assemble(
    main: Vec<u8>,
    blocks: Vec<u8>,
    extras: Vec<u8>,
) -> Result<(Vec<EntityRecord>, Vec<EntityRecord>)> {
    reassemble_sections(
        section(ENTITY_BASE, main),
        section(BLOCK_BASE, blocks),
        section(EXTRA_BASE, extras),
    )
}

fn kinds(records: &[EntityRecord]) -> Vec<&str> {
    records.iter().map(|record| record.kind.as_str()).collect()
}

#[test]
fn padded_jump_checks_crc_before_padding_and_preserves_unsigned_target() {
    let raw = jump(0x8000_1234, 19);
    let decoded = decode_positioned_entities(&raw, &tables(), 0, "ANSI_1252").unwrap();
    assert_eq!(decoded[0].jump_address, Some(0x8000_1234));
    assert_eq!(decoded[0].size, 29);
    let mut corrupted = raw.clone();
    corrupted[8] ^= 1;
    assert!(decode_positioned_entities(&corrupted, &tables(), 0, "ANSI_1252").is_err());
    let mut altered_padding = raw;
    altered_padding[10] ^= 1;
    assert!(decode_positioned_entities(&altered_padding, &tables(), 0, "ANSI_1252").is_ok());
}

#[test]
fn unsigned_jump_address_survives_low_level_codec() {
    let raw = jump(0x8000_1234, 0);
    let decoded = decode_entities(&raw, &tables(), 0, "ANSI_1252").unwrap();
    assert_eq!(
        encode_entities(&decoded, &tables(), 0, "ANSI_1252").unwrap(),
        raw
    );
}

#[test]
fn polyline_continuation_follows_extra_and_return_jump() {
    let mut main = common(19, 128, &[]);
    main.extend(jump(0x8000_0000, 14));
    let resume = main.len() as u32;
    main.extend(point(9.));
    let mut extras = vertex(3.);
    extras.extend(vertex(4.));
    extras.extend(seqend(ENTITY_BASE));
    extras.extend(jump(resume, 7));
    let (entities, blocks) = assemble(main, Vec::new(), extras).unwrap();
    assert!(blocks.is_empty());
    assert_eq!(
        kinds(&entities),
        ["POLYLINE", "VERTEX", "VERTEX", "SEQEND", "POINT"]
    );
    assert_eq!(entities[1].number(10, 0.), 3.);
    assert_eq!(entities[2].number(10, 0.), 4.);
    assert_eq!(entities[4].number(10, 0.), 9.);
}

#[test]
fn insert_attributes_reassemble_before_following_entities() {
    let mut main = insert();
    main.extend(jump(0x8000_0000, 0));
    let resume = main.len() as u32;
    main.extend(point(8.));
    let mut extras = attrib();
    extras.extend(seqend(ENTITY_BASE));
    extras.extend(jump(resume, 0));
    let (entities, _) = assemble(main, Vec::new(), extras).unwrap();
    assert_eq!(kinds(&entities), ["INSERT", "ATTRIB", "SEQEND", "POINT"]);
    assert_eq!(entities[1].string(1), "V");
    assert_eq!(entities[1].string(2), "T");
}

#[test]
fn block_owned_extra_vertices_stay_with_block() {
    let mut blocks = common(12, 0, &[0; 16]);
    let owner = BLOCK_BASE + blocks.len() as u32;
    blocks.extend(common(19, 128, &[]));
    blocks.extend(jump(0x8000_0000, 0));
    let resume = blocks.len() as u32;
    blocks.extend(common(13, 0, &[]));
    let mut extras = vertex(6.);
    extras.extend(seqend(owner));
    extras.extend(jump(0x4000_0000 | resume, 0));
    let (entities, blocks) = assemble(point(1.), blocks, extras).unwrap();
    assert_eq!(kinds(&entities), ["POINT"]);
    assert_eq!(
        kinds(&blocks),
        ["BLOCK", "POLYLINE", "VERTEX", "SEQEND", "ENDBLK"]
    );
    assert_eq!(blocks[2].number(10, 0.), 6.);
}

#[test]
fn backward_jump_is_valid_when_no_record_is_revisited() {
    let early_point = point(1.);
    let late_start = 10 + early_point.len() as u32 + 10;
    let final_offset = late_start + point(2.).len() as u32 + 10;
    let mut main = jump(late_start, 0);
    main.extend(early_point);
    main.extend(jump(final_offset, 0));
    main.extend(point(2.));
    main.extend(jump(10, 0));
    let (entities, _) = assemble(main, Vec::new(), Vec::new()).unwrap();
    assert_eq!(
        entities
            .iter()
            .map(|record| record.number(10, 0.))
            .collect::<Vec<_>>(),
        [2., 1.]
    );
}

#[test]
fn malformed_jump_boundaries_and_flags_are_rejected() {
    for target in [1, 11, 0x2000_0000, 0x4000_0001, 0x8000_0001] {
        assert!(
            assemble(jump(target, 0), Vec::new(), Vec::new()).is_err(),
            "target {target:#x}"
        );
    }
}

#[test]
fn jump_cycles_and_duplicate_ownership_are_rejected() {
    assert!(assemble(jump(0, 0), Vec::new(), Vec::new()).is_err());
    let main = jump(0x8000_0000, 0);
    let extras = jump(0, 0);
    assert!(assemble(main, Vec::new(), extras).is_err());

    let mut main = common(19, 128, &[]);
    main.extend(jump(0x8000_0000, 0));
    let second = main.len() as u32;
    main.extend(common(19, 128, &[]));
    main.extend(jump(0x8000_0000, 0));
    let mut extras = vertex(3.);
    extras.extend(seqend(ENTITY_BASE));
    extras.extend(jump(second, 0));
    assert!(assemble(main, Vec::new(), extras).is_err());
}

#[test]
fn orphan_children_missing_seqend_and_wrong_owner_are_rejected() {
    for child in [vertex(3.), attrib(), seqend(ENTITY_BASE)] {
        assert!(assemble(point(1.), Vec::new(), child).is_err());
    }
    assert!(assemble(common(19, 128, &[]), Vec::new(), Vec::new()).is_err());
    let mut main = common(19, 128, &[]);
    main.extend(vertex(3.));
    main.extend(seqend(ENTITY_BASE + 1));
    assert!(assemble(main, Vec::new(), Vec::new()).is_err());
}

#[test]
fn native_viewport_is_unsigned_and_follows_handle_before_geometry() {
    let mut body = vec![0; 4];
    body.extend([4, 2, 0xab, 0xcd]);
    body.extend(u16::MAX.to_le_bytes());
    body.extend(1_f64.to_le_bytes());
    body.extend(2_f64.to_le_bytes());
    body.extend(3_f64.to_le_bytes());
    let raw = frame(2, 64 | 32, &body, 0);
    let records = decode_positioned_entities(&raw, &tables(), 0, "ANSI_1252").unwrap();
    let record = records[0].record.as_ref().unwrap();
    assert_eq!(record.native_viewport, Some(u16::MAX));
    assert_eq!(record.string(5), "ABCD");
    assert_eq!(record.number(10, 0.), 1.);
    assert_eq!(record.number(30, 0.), 3.);
    assert!(!record.pairs.iter().any(|(code, _)| *code == 69));
    let encoded = encode_entities(&[record.clone()], &tables(), 0, "ANSI_1252").unwrap();
    assert_eq!(encoded, raw);
}

#[test]
fn seqend_address_metadata_is_not_exported_through_dxf_pairs() {
    let raw = seqend(0x1234_5678);
    let records = decode_positioned_entities(&raw, &tables(), EXTRA_BASE, "ANSI_1252").unwrap();
    assert_eq!(records[0].sequence_owner, Some(0x1234_5678));
    assert_eq!(records[0].record.as_ref().unwrap().pairs, [(8, "0".into())]);
}
