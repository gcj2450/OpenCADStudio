//! Native entity fields which are not represented by the intermediate DXF.

use std::collections::HashMap;

use super::entities::EntityRecord;
use crate::entities::{EntityCommon, EntityType, R12EntityMetadata, Shape};
use crate::error::{DxfError, Result};
use crate::{CadDocument, Handle};

fn invalid(message: impl Into<String>) -> DxfError {
    DxfError::InvalidFormat(message.into())
}

fn record_handle(record: &EntityRecord) -> Option<Handle> {
    u64::from_str_radix(record.string(5).trim(), 16)
        .ok()
        .filter(|value| *value != 0)
        .map(Handle::new)
}

fn child_range(records: &[EntityRecord], parent: usize) -> std::ops::Range<usize> {
    let kind = match records[parent].kind.as_str() {
        "POLYLINE" => "VERTEX",
        "INSERT" if records[parent].integer(66, 0) != 0 => "ATTRIB",
        _ => return parent + 1..parent + 1,
    };
    let start = parent + 1;
    let mut end = start;
    while end < records.len() && records[end].kind == kind {
        end += 1;
    }
    if records
        .get(end)
        .is_some_and(|record| record.kind == "SEQEND")
    {
        end += 1;
    }
    start..end
}

fn viewport_metadata(record: &EntityRecord) -> Option<R12EntityMetadata> {
    record.native_viewport.map(|viewport| R12EntityMetadata {
        viewport: Some(viewport),
        child_viewports: Vec::new(),
    })
}

/// Restore native viewport references and SHAPE identity after DXF import.
pub(super) fn restore(
    document: &mut CadDocument,
    entities: &[EntityRecord],
    blocks: &[EntityRecord],
    style_records: &[EntityRecord],
) -> Result<()> {
    let styles: HashMap<_, _> = document
        .text_styles
        .iter()
        .map(|style| {
            (
                style.name.to_ascii_uppercase(),
                (style.name.clone(), style.handle),
            )
        })
        .collect();
    let mut source = HashMap::new();
    for records in [entities, blocks] {
        for (index, record) in records.iter().enumerate() {
            if let Some(handle) = record_handle(record) {
                let children = child_range(records, index);
                source.insert(handle, (record, &records[children]));
            }
        }
    }
    for entity in document.entities_mut_keep_raw() {
        let Some((record, children)) = source.get(&entity.common().handle) else {
            continue;
        };
        let child_viewports: Vec<_> = children.iter().map(|child| child.native_viewport).collect();
        if record.native_viewport.is_some() || child_viewports.iter().any(Option::is_some) {
            entity.common_mut().r12_metadata = Some(R12EntityMetadata {
                viewport: record.native_viewport,
                child_viewports,
            });
        }
        if let EntityType::Insert(insert) = entity {
            for (attribute, child) in insert.attributes.iter_mut().zip(children.iter()) {
                attribute.common.r12_metadata = viewport_metadata(child);
            }
        }
        if let EntityType::Shape(shape) = entity {
            let number = record.string(70).parse::<u8>().map_err(|_| {
                invalid(format!(
                    "Invalid AC1009 SHAPE number {:?}",
                    record.string(70)
                ))
            })?;
            shape.shape_number = i32::from(number);
            let (name, handle) = if let Some(index) = record.native_shape_style {
                let style = style_records.get(usize::from(index)).ok_or_else(|| {
                    invalid(format!(
                        "AC1009 SHAPE references missing style index {index}"
                    ))
                })?;
                let handle = record_handle(style)
                    .ok_or_else(|| invalid("AC1009 SHAPE style identity is unavailable"))?;
                (style.string(2).to_owned(), handle)
            } else {
                let style_name = if record.string(7).is_empty() {
                    "STANDARD"
                } else {
                    record.string(7)
                };
                styles
                    .get(&style_name.to_ascii_uppercase())
                    .cloned()
                    .ok_or_else(|| {
                        invalid(format!(
                            "AC1009 SHAPE references missing style {style_name:?}"
                        ))
                    })?
            };
            shape.style_name = name;
            shape.style_handle = Some(handle);
        }
    }
    Ok(())
}

fn shape_number(shape: &Shape) -> Result<u8> {
    let number = if shape.shape_number == 0
        && shape.style_handle.is_none()
        && !shape.shape_name.is_empty()
    {
        shape.shape_name.trim().parse::<i32>().map_err(|_| {
            invalid(format!(
                "AC1009 SHAPE {:?} requires a numeric shape_number",
                shape.shape_name
            ))
        })?
    } else {
        shape.shape_number
    };
    u8::try_from(number).map_err(|_| {
        invalid(format!(
            "AC1009 SHAPE number {number} must be between 0 and 255"
        ))
    })
}

fn prepare_shape(
    document: &CadDocument,
    shape: &Shape,
    record: &mut EntityRecord,
    style_records: &[EntityRecord],
) -> Result<()> {
    let number = shape_number(shape)?;
    let style = if !shape.style_name.is_empty() {
        document.text_styles.get(&shape.style_name)
    } else if let Some(handle) = shape.style_handle {
        document
            .text_styles
            .iter()
            .find(|style| style.handle == handle)
    } else {
        document.text_styles.get("STANDARD")
    }
    .ok_or_else(|| {
        invalid(format!(
            "AC1009 SHAPE references unavailable style {:?}",
            shape.style_name
        ))
    })?;
    let index = style_records
        .iter()
        .position(|record| {
            if style.handle.is_valid() {
                record_handle(record) == Some(style.handle)
            } else {
                record.string(2).eq_ignore_ascii_case(&style.name)
            }
        })
        .ok_or_else(|| invalid("AC1009 SHAPE style is unavailable in the native STYLE table"))?;
    record.native_shape_style =
        Some(u8::try_from(index).map_err(|_| invalid("AC1009 SHAPE style index exceeds 255"))?);
    record.pairs.retain(|(code, _)| !matches!(code, 70 | 7));
    record.pairs.push((70, number.to_string()));
    record.pairs.push((7, style.name.clone()));
    Ok(())
}

/// Apply typed entity fields to records immediately before native encoding.
pub(super) fn prepare(
    document: &CadDocument,
    records: &mut [EntityRecord],
    style_records: &[EntityRecord],
) -> Result<()> {
    let mut common: HashMap<Handle, &EntityCommon> = HashMap::new();
    let mut shapes = HashMap::new();
    let mut attributes = HashMap::new();
    for entity in document.entities.iter().map(|entity| entity.as_ref()) {
        common.insert(entity.common().handle, entity.common());
        if let EntityType::Shape(shape) = entity {
            shapes.insert(shape.common.handle, shape);
        }
        if let EntityType::Insert(insert) = entity {
            attributes.insert(insert.common.handle, &insert.attributes);
            for attribute in &insert.attributes {
                if !attribute.common.handle.is_null() {
                    common.insert(attribute.common.handle, &attribute.common);
                }
            }
        }
    }
    for index in 0..records.len() {
        let Some(handle) = record_handle(&records[index]) else {
            continue;
        };
        let Some(entity_common) = common.get(&handle) else {
            continue;
        };
        let metadata = entity_common.r12_metadata.as_ref();
        records[index].native_viewport = metadata.and_then(|metadata| metadata.viewport);
        if records[index].kind == "SHAPE" {
            let shape = shapes
                .get(&handle)
                .ok_or_else(|| invalid("AC1009 SHAPE identity is unavailable"))?;
            prepare_shape(document, shape, &mut records[index], style_records)?;
        }
        if let Some(metadata) = metadata {
            let children = child_range(records, index);
            for (ordinal, child_index) in children.enumerate() {
                let child = &mut records[child_index];
                let viewport = if child.kind == "SEQEND" {
                    metadata.child_viewports.last().copied().flatten()
                } else {
                    metadata
                        .child_viewports
                        .get(ordinal)
                        .filter(|_| ordinal + 1 < metadata.child_viewports.len())
                        .copied()
                        .flatten()
                };
                child.native_viewport = viewport;
            }
        }
        if let Some(attributes) = attributes.get(&handle) {
            let children = child_range(records, index);
            for (attribute, child_index) in attributes.iter().zip(children) {
                if records[child_index].kind == "ATTRIB" {
                    records[child_index].native_viewport = attribute
                        .common
                        .r12_metadata
                        .as_ref()
                        .and_then(|metadata| metadata.viewport);
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
