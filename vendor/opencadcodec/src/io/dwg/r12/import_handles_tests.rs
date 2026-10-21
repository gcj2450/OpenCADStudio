use super::*;

pub(super) fn record(kind: &str, pairs: &[(i32, &str)]) -> EntityRecord {
    EntityRecord {
        kind: kind.into(),
        pairs: pairs.iter().map(|(c, v)| (*c, (*v).into())).collect(),
        ..EntityRecord::default()
    }
}

pub(super) fn fixture(
    seed: u64,
    tables: &[Vec<EntityRecord>; 10],
    entities: &[EntityRecord],
    blocks: &[EntityRecord],
) -> Vec<u8> {
    fixture_with_references(seed, tables, entities, blocks, HeaderReferences::default())
}

fn fixture_with_references(
    seed: u64,
    tables: &[Vec<EntityRecord>; 10],
    entities: &[EntityRecord],
    blocks: &[EntityRecord],
    references: HeaderReferences,
) -> Vec<u8> {
    let refs = ensure_table_defaults(tables::entity_tables(tables));
    let entity_bytes =
        encode_entities(entities, &refs, header::ENTITIES_START, "ANSI_1252").unwrap();
    let entities_end = header::ENTITIES_START + entity_bytes.len() as u32;
    let blocks_start = entities_end + 32;
    let block_bytes = encode_entities(blocks, &refs, blocks_start, "ANSI_1252").unwrap();
    let extras_start = blocks_start + block_bytes.len() as u32 + 32;
    let mut address = extras_start + 16;
    let mut locators = [TableLocator::default(); 10];
    let mut bytes = Vec::new();
    for (i, (size, payload)) in
        tables::encode_tables(tables, &std::collections::HashMap::new(), "ANSI_1252")
            .unwrap()
            .into_iter()
            .enumerate()
    {
        if payload.is_empty() {
            continue;
        }
        locators[i] = TableLocator {
            record_size: size,
            count: (payload.len() / size as usize) as u16,
            flags: 0,
            address: address + 16,
        };
        bytes.extend_from_slice(&header::TABLE_BEGIN[i]);
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&tables::TABLE_END[i]);
        address += payload.len() as u32 + 32;
    }
    let layout = HeaderLayout {
        entities_start: header::ENTITIES_START,
        entities_end,
        blocks_start,
        blocks_size: block_bytes.len() as u32,
        extras_start,
        extras_size: 0,
    };
    let mut vars = crate::document::HeaderVariables::default();
    vars.handle_seed = seed;
    let mut out = write_header(&vars, layout, &locators, references, 0, None).unwrap();
    out[0x4ec..0x4ee].copy_from_slice(&u16::from(seed != 0).to_le_bytes()); // HANDLING disabled: every entity receives an import handle.
    let crc = crate::io::dwg::crc::crc16(
        crate::io::dwg::crc::CRC16_SEED,
        &out[..header::HEADER_VARIABLES_END],
    );
    out[header::HEADER_VARIABLES_END..header::HEADER_VARIABLES_END + 2]
        .copy_from_slice(&crc.to_le_bytes());
    out.extend(entity_bytes);
    out.extend(ENTITIES_END);
    out.extend(BLOCK_ENTITIES_BEGIN);
    out.extend(block_bytes);
    out.extend(BLOCK_ENTITIES_END);
    out.extend(EXTRA_ENTITIES_BEGIN);
    out.extend(EXTRA_ENTITIES_END);
    out.extend(bytes);
    out.extend(write_auxheader(layout, &locators, seed, address));
    out
}

#[test]
fn handleless_text_matches_independent_autocad_imports() {
    let mut tables: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    tables[1] = vec![
        record("LAYER", &[(2, "0"), (62, "7"), (6, "CONTINUOUS")]),
        record("LAYER", &[(2, "PROBE"), (62, "2"), (6, "CONTINUOUS")]),
    ];
    tables[2] = [
        ("STANDARD", "txt"),
        ("ALPHA", "simplex"),
        ("BRAVO", "simplex"),
        ("CHARLIE", "txt"),
        ("DELTA", "iso"),
    ]
    .map(|(n, f)| record("STYLE", &[(2, n), (3, f)]))
    .to_vec();
    tables[3] = vec![record("LTYPE", &[(2, "CONTINUOUS")])];
    tables[7] = vec![record("APPID", &[(2, "ACAD")])];
    let styles = ["ALPHA", "BRAVO", "STANDARD", "CHARLIE", "DELTA", "ALPHA"];
    let entities = styles
        .iter()
        .map(|s| {
            record(
                "TEXT",
                &[
                    (8, "0"),
                    (1, "Probe"),
                    (7, s),
                    (40, "2"),
                    (10, "0"),
                    (20, "0"),
                ],
            )
        })
        .collect::<Vec<_>>();
    let doc = read_document(&fixture(0, &tables, &entities, &[]), None, None).unwrap();
    let handles = doc
        .entities()
        .filter(|e| matches!(e, crate::EntityType::Text(_)))
        .map(|e| e.common().handle.value())
        .collect::<Vec<_>>();
    assert_eq!(handles, [0x7c, 0x7e, 0x7f, 0x81, 0x82, 0x84]);
    assert_eq!(doc.layers.get("PROBE").unwrap().handle.value(), 0x76);
    assert_eq!(
        doc.block_records
            .get("*Model_Space")
            .unwrap()
            .handle
            .value(),
        0x6f
    );
    assert_eq!(
        doc.block_records
            .get("*Model_Space")
            .unwrap()
            .block_entity_handle
            .value(),
        0x79
    );
    assert_eq!(
        doc.block_records
            .get("*Model_Space")
            .unwrap()
            .layout
            .value(),
        0x72
    );
}

#[test]
fn legacy_font_substitution_shares_the_transient_identity() {
    let mut tables: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    tables[1] = vec![
        record("LAYER", &[(2, "0"), (62, "7"), (6, "CONTINUOUS")]),
        record("LAYER", &[(2, "PROBE"), (62, "2"), (6, "CONTINUOUS")]),
    ];
    tables[2] = [
        ("STANDARD", "txt"),
        ("ALPHA", "simplex"),
        ("BRAVO", "simplex"),
        ("CHARLIE", "txt"),
        ("DELTA", "unavailable_indium_probe_font"),
    ]
    .map(|(n, f)| record("STYLE", &[(2, n), (3, f)]))
    .to_vec();
    tables[3] = vec![record("LTYPE", &[(2, "CONTINUOUS")])];
    tables[7] = vec![record("APPID", &[(2, "ACAD")])];
    let entities = ["DELTA", "STANDARD", "ALPHA", "CHARLIE"].map(|s| {
        record(
            "TEXT",
            &[
                (8, "0"),
                (1, "Probe"),
                (7, s),
                (40, "2"),
                (10, "0"),
                (20, "0"),
            ],
        )
    });
    let bytes = fixture(0, &tables, &entities, &[]);
    let mut reader =
        crate::DwgReader::from_stream(Cursor::new(bytes)).with_legacy_font_resolver(|font| {
            if font == "unavailable_indium_probe_font" {
                "SIMPLEX".into()
            } else {
                font.to_ascii_uppercase()
            }
        });
    let doc = reader.read().unwrap();
    assert_eq!(
        doc.entities()
            .filter(|e| matches!(e, crate::EntityType::Text(_)))
            .map(|e| e.common().handle.value())
            .collect::<Vec<_>>(),
        [0x7c, 0x7e, 0x80, 0x81]
    );
}

#[test]
fn stored_compound_handles_and_owners_survive_import() {
    use crate::{EntityType, Handle};
    let mut tables: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    tables[0] = vec![record("BLOCK_RECORD", &[(2, "PART")])];
    tables[1] = vec![record("LAYER", &[(2, "0"), (62, "7"), (6, "CONTINUOUS")])];
    tables[2] = vec![record("STYLE", &[(2, "STANDARD"), (3, "txt")])];
    tables[3] = vec![record("LTYPE", &[(2, "CONTINUOUS")])];
    tables[7] = vec![record("APPID", &[(2, "ACAD")])];
    let blocks = [
        record("BLOCK", &[(2, "PART"), (5, "11"), (8, "0")]),
        record(
            "ATTDEF",
            &[
                (5, "12"),
                (8, "0"),
                (2, "TAG"),
                (1, "default"),
                (3, "Prompt"),
                (40, "2"),
            ],
        ),
        record("ENDBLK", &[(5, "13"), (8, "0")]),
    ];
    let entities = [
        record("INSERT", &[(5, "20"), (8, "0"), (2, "PART"), (66, "1")]),
        record(
            "ATTRIB",
            &[(5, "21"), (8, "0"), (2, "TAG"), (1, "value"), (40, "2")],
        ),
        record("SEQEND", &[(5, "22"), (8, "0")]),
    ];
    let doc = read_document(&fixture(0x1000, &tables, &entities, &blocks), None, None).unwrap();
    let part = doc.block_records.get("PART").unwrap();
    assert_eq!(part.handle.value(), 0x1072);
    assert_eq!(part.block_entity_handle.value(), 0x11);
    assert_eq!(part.block_end_handle.value(), 0x13);
    assert_eq!(
        doc.get_entity(Handle::new(0x12))
            .unwrap()
            .common()
            .owner_handle,
        part.handle
    );
    let EntityType::Insert(insert) = doc.get_entity(Handle::new(0x20)).unwrap() else {
        panic!("missing insert")
    };
    assert_eq!(
        insert.common.owner_handle,
        doc.header.model_space_block_handle
    );
    assert_eq!(insert.attributes[0].common.handle.value(), 0x21);
    assert_eq!(insert.seqend_handle, Some(Handle::new(0x22)));
    assert!(doc.next_handle() > 0x1072);
}

#[test]
fn import_handle_overflow_is_rejected() {
    let mut tables = std::array::from_fn(|_| Vec::new());
    assert!(import_handles::ImportHandles::assign(
        u64::MAX,
        &mut tables,
        &mut [],
        &mut [],
        None,
        None
    )
    .is_err());
}

#[test]
fn legacy_header_indices_resolve_to_imported_table_handles() {
    let mut tables: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    tables[1] = vec![
        record("LAYER", &[(2, "0")]),
        record("LAYER", &[(2, "WORK")]),
    ];
    tables[2] = vec![
        record("STYLE", &[(2, "STANDARD")]),
        record("STYLE", &[(2, "LABEL")]),
    ];
    tables[3] = vec![
        record("LTYPE", &[(2, "CONTINUOUS")]),
        record("LTYPE", &[(2, "DASHED")]),
    ];
    for index in [1, -1, -2] {
        let refs = HeaderReferences {
            layer_index: 1,
            text_style_index: 1,
            linetype_index: index,
            ..Default::default()
        };
        let doc = read_document(
            &fixture_with_references(0, &tables, &[], &[], refs),
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            doc.header.current_layer_handle,
            doc.layers.get("WORK").unwrap().handle
        );
        assert_eq!(
            doc.header.current_text_style_handle,
            doc.text_styles.get("LABEL").unwrap().handle
        );
        let name = match index {
            -1 => "ByLayer",
            -2 => "ByBlock",
            _ => "DASHED",
        };
        assert_eq!(
            doc.header.current_linetype_handle,
            doc.line_types.get(name).unwrap().handle
        );
    }
}
