use super::*;
use crate::entities::{AttributeEntity, Insert, Line, Polyline2D, Vertex2D};
use crate::io::dwg::crc::{crc16, CRC16_SEED};
use crate::io::dwg::r12::header::{HeaderLayout, HeaderReferences, TableLocator};
use crate::io::dwg::r12::{header, tables};
use crate::tables::{BlockRecord, TextStyle};
use crate::{DwgReader, DwgWriter, DxfVersion, Vector3};
use std::io::Cursor;

fn record(kind: &str, pairs: &[(i32, &str)]) -> EntityRecord {
    let mut record = EntityRecord::new(kind);
    record.pairs = pairs
        .iter()
        .map(|(code, value)| (*code, (*value).into()))
        .collect();
    record
}

fn add_style(document: &mut CadDocument, name: &str) -> Handle {
    let mut style = TextStyle::new(name);
    style.handle = document.allocate_handle();
    let handle = style.handle;
    document.text_styles.add(style).unwrap();
    handle
}

fn read(bytes: Vec<u8>) -> CadDocument {
    DwgReader::from_stream(Cursor::new(bytes)).read().unwrap()
}

fn roundtrip(document: &CadDocument) -> CadDocument {
    let first = DwgWriter::write_to_vec(document).unwrap();
    assert_eq!(first, DwgWriter::write_to_vec(document).unwrap());
    read(first)
}

fn shape(document: &CadDocument) -> &Shape {
    document
        .entities()
        .find_map(|entity| match entity {
            EntityType::Shape(shape) => Some(shape),
            _ => None,
        })
        .unwrap()
}

// Construct the native SHAPE layout directly, independently of the entity encoder.
fn native_shape(number: u8, style: u8, viewport: u16) -> Vec<u8> {
    let mut bytes = vec![4, 0x60, 0, 0, 0, 0, 2, 0, 4, 1, 0xA0];
    bytes.extend(viewport.to_le_bytes());
    for value in [1.0_f64, 2.0, 3.0] {
        bytes.extend(value.to_le_bytes());
    }
    bytes.extend([number, style]);
    let size = (bytes.len() + 2) as u16;
    bytes[2..4].copy_from_slice(&size.to_le_bytes());
    let crc = crc16(CRC16_SEED, &bytes);
    bytes.extend(crc.to_le_bytes());
    bytes
}

fn native_fixture(entity_bytes: &[u8], table_records: &[Vec<EntityRecord>; 10]) -> Vec<u8> {
    let entities_end = header::ENTITIES_START + entity_bytes.len() as u32;
    let layout = HeaderLayout {
        entities_start: header::ENTITIES_START,
        entities_end,
        blocks_start: entities_end + 32,
        blocks_size: 0,
        extras_start: entities_end + 64,
        extras_size: 0,
    };
    let mut address = layout.extras_start + 16;
    let mut locators = [TableLocator::default(); 10];
    let mut table_bytes = Vec::new();
    for (index, (size, payload)) in
        tables::encode_tables(table_records, &HashMap::new(), "ANSI_1252")
            .unwrap()
            .into_iter()
            .enumerate()
    {
        if payload.is_empty() {
            continue;
        }
        locators[index] = TableLocator {
            record_size: size,
            count: (payload.len() / usize::from(size)) as u16,
            flags: 0,
            address: address + 16,
        };
        table_bytes.extend(header::TABLE_BEGIN[index]);
        table_bytes.extend(&payload);
        table_bytes.extend(tables::TABLE_END[index]);
        address += payload.len() as u32 + 32;
    }
    let mut variables = crate::document::HeaderVariables::default();
    variables.handle_seed = 0x1000;
    let mut bytes = header::write_header(
        &variables,
        layout,
        &locators,
        HeaderReferences::default(),
        0,
        None,
    )
    .unwrap();
    bytes.extend(entity_bytes);
    bytes.extend(header::ENTITIES_END);
    bytes.extend(header::BLOCK_ENTITIES_BEGIN);
    bytes.extend(header::BLOCK_ENTITIES_END);
    bytes.extend(header::EXTRA_ENTITIES_BEGIN);
    bytes.extend(header::EXTRA_ENTITIES_END);
    bytes.extend(table_bytes);
    bytes.extend(header::write_auxheader(
        layout,
        &locators,
        variables.handle_seed,
        address,
    ));
    bytes
}

#[test]
fn independent_native_shape_retains_number_style_and_unsigned_viewport() {
    let mut table_records: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    table_records[1].push(record("LAYER", &[(2, "0"), (62, "7"), (6, "CONTINUOUS")]));
    table_records[2].push(record("STYLE", &[(2, "STANDARD"), (3, "txt")]));
    table_records[2].push(record("STYLE", &[(2, "SHAPES"), (3, "shapes.shx")]));
    table_records[3].push(record("LTYPE", &[(2, "CONTINUOUS")]));
    let document = read(native_fixture(&native_shape(241, 1, 65535), &table_records));
    let imported = shape(&document);
    assert_eq!(imported.shape_number, 241);
    assert_eq!(imported.style_name, "SHAPES");
    assert_eq!(
        imported.style_handle,
        Some(document.text_styles.get("SHAPES").unwrap().handle)
    );
    assert_eq!(
        imported.common.r12_metadata.as_ref().unwrap().viewport,
        Some(65535)
    );
    let document = roundtrip(&document);
    assert_eq!(shape(&document).shape_number, 241);
    assert_eq!(shape(&document).style_name, "SHAPES");
    assert_eq!(
        shape(&document)
            .common
            .r12_metadata
            .as_ref()
            .unwrap()
            .viewport,
        Some(65535)
    );
}

#[test]
fn native_shape_retains_identity_of_duplicate_unnamed_shape_file_styles() {
    let mut table_records: [Vec<EntityRecord>; 10] = std::array::from_fn(|_| Vec::new());
    table_records[1].push(record("LAYER", &[(2, "0"), (62, "7"), (6, "CONTINUOUS")]));
    table_records[2].push(record("STYLE", &[(2, "STANDARD"), (3, "txt")]));
    table_records[2].push(record("STYLE", &[(2, ""), (70, "1"), (3, "first.shx")]));
    table_records[2].push(record("STYLE", &[(2, ""), (70, "1"), (3, "second.shx")]));
    table_records[3].push(record("LTYPE", &[(2, "CONTINUOUS")]));
    let mut document = read(native_fixture(&native_shape(17, 2, 42), &table_records));
    for _ in 0..2 {
        let shape = shape(&document);
        assert_eq!(shape.shape_number, 17);
        assert!(shape.style_name.is_empty());
        let style = document
            .text_styles
            .iter()
            .find(|style| Some(style.handle) == shape.style_handle)
            .unwrap();
        assert_eq!(style.font_file, "second.shx");
        document = roundtrip(&document);
    }
}

#[test]
fn typed_shape_identity_survives_writes_and_edits() {
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    add_style(&mut document, "FIRST");
    add_style(&mut document, "SECOND");
    let mut source = Shape::with_number(Vector3::ZERO, 255, 2.0);
    source.shape_name = "ARROW".into();
    source.style_name = "FIRST".into();
    let handle = document.add_entity(EntityType::Shape(source)).unwrap();
    let mut restored = roundtrip(&document);
    assert_eq!(shape(&restored).shape_number, 255);
    assert_eq!(shape(&restored).style_name, "FIRST");
    let EntityType::Shape(edited) = restored.get_entity_mut(handle).unwrap() else {
        panic!("SHAPE");
    };
    edited.shape_number = 0;
    edited.style_name = "SECOND".into();
    let restored = roundtrip(&restored);
    assert_eq!(shape(&restored).shape_number, 0);
    assert_eq!(shape(&restored).style_name, "SECOND");
    assert_eq!(
        shape(&restored).style_handle,
        Some(restored.text_styles.get("SECOND").unwrap().handle)
    );
}

#[test]
fn new_named_shape_file_style_does_not_require_an_allocated_handle() {
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    let mut style = TextStyle::new("SHAPES");
    style.is_shape_file = true;
    style.font_file = "ltypeshp.shx".into();
    document.text_styles.add(style).unwrap();
    let mut source = Shape::with_number(Vector3::ZERO, 132, 2.0);
    source.style_name = "SHAPES".into();
    document.add_entity(EntityType::Shape(source)).unwrap();
    let restored = roundtrip(&document);
    let imported = shape(&restored);
    assert_eq!(imported.style_name, "SHAPES");
    assert_eq!(imported.shape_number, 132);
    let style = restored.text_styles.get("SHAPES").unwrap();
    assert!(style.is_shape_file);
    assert_eq!(style.font_file, "ltypeshp.shx");
    assert_eq!(imported.style_handle, Some(style.handle));
}

#[test]
fn shape_writer_rejects_out_of_range_and_unresolved_named_identity() {
    for number in [-1, 256] {
        let mut document = CadDocument::with_version(DxfVersion::AC1009);
        document
            .add_entity(EntityType::Shape(Shape::with_number(
                Vector3::ZERO,
                number,
                1.0,
            )))
            .unwrap();
        let error = DwgWriter::write_to_vec(&document).unwrap_err().to_string();
        assert!(error.contains("between 0 and 255"), "{error}");
    }
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    document
        .add_entity(EntityType::Shape(Shape::with_name(
            Vector3::ZERO,
            "ARROW",
            1.0,
        )))
        .unwrap();
    assert!(DwgWriter::write_to_vec(&document)
        .unwrap_err()
        .to_string()
        .contains("numeric shape_number"));
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    document
        .add_entity(EntityType::Shape(Shape::with_name(
            Vector3::ZERO,
            "233",
            1.0,
        )))
        .unwrap();
    assert_eq!(shape(&roundtrip(&document)).shape_number, 233);
}

#[test]
fn native_viewports_survive_top_level_edits_and_polyline_vertex_addition() {
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    let mut line = Line::from_coords(0.0, 0.0, 0.0, 1.0, 2.0, 0.0);
    line.common.r12_metadata = Some(R12EntityMetadata {
        viewport: Some(0),
        child_viewports: Vec::new(),
    });
    let line_handle = document.add_entity(EntityType::Line(line)).unwrap();
    let mut polyline = Polyline2D::new();
    polyline.add_vertex(Vertex2D::new(Vector3::ZERO));
    polyline.add_vertex(Vertex2D::new(Vector3::new(1.0, 2.0, 0.0)));
    polyline.common.r12_metadata = Some(R12EntityMetadata {
        viewport: Some(32768),
        child_viewports: vec![Some(2), None, Some(65535)],
    });
    let handle = document
        .add_entity(EntityType::Polyline2D(polyline))
        .unwrap();
    let mut restored = roundtrip(&document);
    let EntityType::Line(line) = restored.get_entity_mut(line_handle).unwrap() else {
        panic!("LINE");
    };
    line.end.x = 5.0;
    let EntityType::Polyline2D(polyline) = restored.get_entity_mut(handle).unwrap() else {
        panic!("POLYLINE");
    };
    polyline.add_vertex(Vertex2D::new(Vector3::new(4.0, 3.0, 0.0)));
    let restored = roundtrip(&restored);
    assert_eq!(
        restored
            .get_entity(line_handle)
            .unwrap()
            .common()
            .r12_metadata
            .as_ref()
            .unwrap()
            .viewport,
        Some(0)
    );
    let metadata = restored
        .get_entity(handle)
        .unwrap()
        .common()
        .r12_metadata
        .as_ref()
        .unwrap();
    assert_eq!(metadata.viewport, Some(32768));
    assert_eq!(metadata.child_viewports, [Some(2), None, None, Some(65535)]);
}

#[test]
fn insert_attributes_keep_viewports_when_reordered() {
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    let mut block = BlockRecord::new("PART");
    block.handle = document.allocate_handle();
    document.block_records.add(block).unwrap();
    let mut insert = Insert::new("PART", Vector3::ZERO);
    for viewport in [0_u16, 65535] {
        let mut attribute = AttributeEntity::default();
        attribute.common.handle = document.allocate_handle();
        attribute.tag = format!("V{viewport}");
        attribute.common.r12_metadata = Some(R12EntityMetadata {
            viewport: Some(viewport),
            child_viewports: Vec::new(),
        });
        insert.attributes.push(attribute);
    }
    insert.common.r12_metadata = Some(R12EntityMetadata {
        viewport: Some(42),
        child_viewports: vec![Some(0), Some(65535), Some(32768)],
    });
    let handle = document.add_entity(EntityType::Insert(insert)).unwrap();
    let mut restored = roundtrip(&document);
    let EntityType::Insert(insert) = restored.get_entity_mut(handle).unwrap() else {
        panic!("INSERT");
    };
    insert.attributes.swap(0, 1);
    let restored = roundtrip(&restored);
    let EntityType::Insert(insert) = restored.get_entity(handle).unwrap() else {
        panic!("INSERT");
    };
    assert_eq!(
        insert.attributes[0]
            .common
            .r12_metadata
            .as_ref()
            .unwrap()
            .viewport,
        Some(65535)
    );
    assert_eq!(
        insert.attributes[1]
            .common
            .r12_metadata
            .as_ref()
            .unwrap()
            .viewport,
        Some(0)
    );
    assert_eq!(
        insert.common.r12_metadata.as_ref().unwrap().child_viewports,
        [Some(65535), Some(0), Some(32768)]
    );
}

#[test]
fn new_insert_attributes_keep_viewports_without_allocated_child_handles() {
    let mut document = CadDocument::with_version(DxfVersion::AC1009);
    let mut block = BlockRecord::new("PART");
    block.handle = document.allocate_handle();
    document.block_records.add(block).unwrap();
    let mut insert = Insert::new("PART", Vector3::ZERO);
    let mut attribute = AttributeEntity::default();
    attribute.tag = "NEW".into();
    attribute.common.r12_metadata = Some(R12EntityMetadata {
        viewport: Some(65535),
        child_viewports: Vec::new(),
    });
    insert.attributes.push(attribute);
    let handle = document.add_entity(EntityType::Insert(insert)).unwrap();
    let restored = roundtrip(&document);
    let EntityType::Insert(insert) = restored.get_entity(handle).unwrap() else {
        panic!("INSERT");
    };
    assert_eq!(
        insert.attributes[0]
            .common
            .r12_metadata
            .as_ref()
            .unwrap()
            .viewport,
        Some(65535)
    );
}

#[test]
fn storage_preservation_keeps_r12_native_metadata() {
    let mut source = EntityType::Line(Line::default());
    source.common_mut().r12_metadata = Some(R12EntityMetadata {
        viewport: Some(65535),
        child_viewports: Vec::new(),
    });
    let mut edited = EntityType::Line(Line::default());
    edited.preserve_storage_data_from(&source);
    assert_eq!(edited.common().r12_metadata, source.common().r12_metadata);
}

#[cfg(feature = "serde")]
#[test]
fn serde_defaults_missing_native_metadata() {
    let mut serialized = serde_json::to_value(EntityCommon::default()).unwrap();
    serialized.as_object_mut().unwrap().remove("r12_metadata");
    let common: EntityCommon = serde_json::from_value(serialized).unwrap();
    assert_eq!(common.r12_metadata, None);
}
