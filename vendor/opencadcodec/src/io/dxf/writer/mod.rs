//! DXF writer module

mod binary_writer;
mod r12;
mod section_writer;
mod stream_writer;
mod text_writer;

pub use binary_writer::DxfBinaryWriter;
pub use section_writer::SectionWriter;
pub use stream_writer::{value_type_for_code, DxfStreamWriter, DxfStreamWriterExt};
pub use text_writer::DxfTextWriter;

use crate::document::CadDocument;
use crate::entities::EntityType;
use crate::error::Result;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// DXF file writer
pub struct DxfWriter<'a> {
    document: &'a CadDocument,
    /// Whether to write binary DXF format
    pub binary: bool,
    /// Explicit target version; `None` writes the document's own version.
    version: Option<crate::types::DxfVersion>,
}

impl<'a> DxfWriter<'a> {
    /// Create a new DXF writer for ASCII output
    pub fn new(document: &'a CadDocument) -> Self {
        Self {
            document,
            binary: false,
            version: None,
        }
    }

    /// Create a new DXF writer for binary output
    pub fn new_binary(document: &'a CadDocument) -> Self {
        Self {
            document,
            binary: true,
            version: None,
        }
    }

    /// Write the document as `version` instead of its own version.
    ///
    /// Requesting [`DxfVersion::AC1009`](crate::types::DxfVersion::AC1009)
    /// produces a genuine R12 file (no handles, no CLASSES/OBJECTS sections,
    /// newer entities converted to R12 entities; ASCII only). A document that
    /// merely *carries* `AC1009` without this call keeps being written as R13,
    /// because the R13 structure preserves everything the document holds.
    pub fn with_version(mut self, version: crate::types::DxfVersion) -> Self {
        self.version = Some(version);
        self
    }

    /// Set whether to write binary format
    pub fn set_binary(&mut self, binary: bool) {
        self.binary = binary;
    }

    /// Write to a file
    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        self.write_to_writer(writer)
    }

    /// Write to any writer
    pub fn write_to_writer<W: Write>(&self, writer: W) -> Result<()> {
        if self.version == Some(crate::types::DxfVersion::AC1009) {
            return self.write_r12(writer);
        }
        // BricsCAD's binary DXF importer rejects the R2000/R2004 headers even
        // when their records are otherwise valid.  The same records are
        // accepted as R2010 binary DXF, whose strings are UTF-8.  Keep this
        // compatibility promotion local to binary output; ASCII and DWG
        // continue to preserve the requested legacy version and code page.
        let output_version = self.output_version();
        let encoding = if output_version < crate::types::DxfVersion::AC1021 {
            crate::io::dxf::code_page::encoding_from_code_page(&self.document.header.code_page)
        } else {
            None
        };
        if self.binary {
            let mut stream_writer = DxfBinaryWriter::new(writer)?;
            if let Some(encoding) = encoding {
                stream_writer.set_encoding(encoding);
            }
            self.write_dxf(&mut stream_writer, output_version)?;
            stream_writer.flush()?;
        } else {
            let mut stream_writer = DxfTextWriter::new(writer);
            if let Some(encoding) = encoding {
                stream_writer.set_encoding(encoding);
            }
            self.write_dxf(&mut stream_writer, output_version)?;
            stream_writer.flush()?;
        }
        Ok(())
    }

    /// Write to a byte vector (useful for testing)
    pub fn write_to_vec(&self) -> Result<Vec<u8>> {
        // Pre-allocate based on entity count: ~512 bytes per entity is a reasonable estimate
        let entity_count = self.document.entities().count();
        let estimated = (entity_count + 64) * 512;
        let mut buffer = Vec::with_capacity(estimated);
        self.write_to_writer(&mut buffer)?;
        Ok(buffer)
    }

    /// Write DXF content to a stream writer
    fn write_dxf<W: DxfStreamWriter>(
        &self,
        writer: &mut W,
        output_version: crate::types::DxfVersion,
    ) -> Result<()> {
        let mut prepared = crate::io::loft_parameters::prepared(self.document);
        // DXF and DWG share the same ownership graph. Repair table-backed
        // entities before either writer snapshots handles, otherwise an
        // ACAD_TABLE can emit a null/nonexistent block-record pointer.
        crate::io::dwg::dwg_writer::prepare_database_references(&mut prepared);
        // An in-place table rename leaves the entry keyed under its old name,
        // so entity layer/linetype lookups miss and reference a name the
        // written table no longer defines (issue #80).
        crate::io::dwg::dwg_writer::prepare_table_keys(&mut prepared);
        // Table entries added after the document was built or read
        // (`Layer::new` + `layers.add`, `AppId::new` + `app_ids.add`) still
        // carry NULL handles. Written as they are, each becomes a `5 / 0`
        // record, a duplicate of the others, and each also claims the root
        // dictionary (owned by NULL) as its extension dictionary. Assign them
        // on the output copy, as the DWG writer does.
        if prepared.has_null_table_entries() {
            prepared.to_mut().assign_table_entry_handles();
        }
        crate::io::dwg::dwg_writer::prepare_active_viewport_elevation(&mut prepared);
        self.write_prepared_dxf(writer, prepared.as_ref(), output_version)
    }

    fn write_prepared_dxf<W: DxfStreamWriter>(
        &self,
        writer: &mut W,
        document: &CadDocument,
        output_version: crate::types::DxfVersion,
    ) -> Result<()> {
        let handle_start = compute_max_handle(document);
        // Count against the version actually emitted, not the one the document
        // was loaded as, so the handle budget matches what the writer produces.
        let extra_handles = count_extra_handles(
            document,
            SectionWriter::<W>::writable_version(output_version),
        );
        let handle_seed = handle_start + extra_handles + 1;
        let mut section_writer = SectionWriter::new(writer, handle_start, handle_seed);
        section_writer.set_version(output_version);
        section_writer.build_valid_handles(document);

        // Write all sections
        section_writer.write_header(document)?;
        section_writer.write_classes(document)?;
        section_writer.write_tables(document)?;
        section_writer.write_blocks(document)?;
        section_writer.write_entities(document)?;
        section_writer.write_objects(document)?;
        section_writer.write_acdsdata()?;

        // Write EOF
        writer.write_string(0, "EOF")?;

        Ok(())
    }

    fn output_version(&self) -> crate::types::DxfVersion {
        let version = self.version.unwrap_or(self.document.version);
        if self.binary && version < crate::types::DxfVersion::AC1021 {
            crate::types::DxfVersion::AC1024
        } else {
            version
        }
    }

    /// Genuine R12 output (see [`DxfWriter::with_version`]).
    fn write_r12<W: Write>(&self, writer: W) -> Result<()> {
        if self.binary {
            // R12 binary DXF uses one-byte group codes, which the binary writer does not emit
            return Err(crate::error::DxfError::UnsupportedVersion(
                "binary DXF R12 (AC1009); write R12 as ASCII DXF".into(),
            ));
        }
        let mut stream_writer = DxfTextWriter::new(writer);
        if let Some(encoding) = crate::io::dxf::code_page::encoding_from_code_page(&self.document.header.code_page) {
            stream_writer.set_encoding(encoding);
        }
        r12::write_r12(&mut stream_writer, self.document)?;
        stream_writer.flush()?;
        Ok(())
    }

    /// Get a reference to the document
    pub fn document(&self) -> &CadDocument {
        &self.document
    }
}

fn count_extra_handles(document: &CadDocument, version: crate::types::DxfVersion) -> u64 {
    use crate::objects::ObjectType;

    let mut count = 0u64;

    for entity in document.entities() {
        match entity {
            EntityType::Polyline(polyline) => {
                // Vertices + SEQEND all use allocate_handle()
                count += polyline.vertices.len() as u64 + 1;
            }
            EntityType::Polyline2D(polyline) => {
                // Plain 2D polylines down-save as LWPOLYLINE for R2000+,
                // which emits no child records (see SectionWriter).
                if version >= crate::types::DxfVersion::AC1015
                    && section_writer::polyline2d_downsaves_to_lwpolyline(polyline)
                {
                    continue;
                }
                // Vertices + SEQEND all use allocate_handle()
                count += polyline.vertices.len() as u64 + 1;
            }
            EntityType::Polyline3D(polyline) => {
                for vertex in &polyline.vertices {
                    if vertex.handle.is_null() {
                        count += 1;
                    }
                }
                // SEQEND always written
                count += 1;
            }
            EntityType::PolyfaceMesh(mesh) => {
                for vertex in &mesh.vertices {
                    if vertex.common.handle.is_null() {
                        count += 1;
                    }
                }
                for face in &mesh.faces {
                    if face.common.handle.is_null() {
                        count += 1;
                    }
                }
                if mesh.seqend_handle.is_none() {
                    count += 1;
                }
            }
            EntityType::PolygonMesh(mesh) => {
                for vertex in &mesh.vertices {
                    if vertex.common.handle.is_null() {
                        count += 1;
                    }
                }
                // SEQEND always needs a handle
                count += 1;
            }
            EntityType::Insert(insert) => {
                if insert.has_attributes() {
                    count += insert
                        .attributes
                        .iter()
                        .filter(|attribute| attribute.common.handle.is_null())
                        .count() as u64;
                    // SEQEND for attribute sequence
                    count += 1;
                }
            }
            _ => {}
        }
    }

    // Count SortEntitiesTable sort entries (allocate_handle used for each)
    for obj in document.objects.values() {
        if let ObjectType::SortEntitiesTable(table) = obj {
            count += table.len() as u64;
        }
    }

    // Null layer handles are allocated while writing the LAYER table.
    count += document
        .layers
        .iter()
        .filter(|layer| layer.handle.is_null())
        .count() as u64;

    count
}

/// Compute the true maximum handle across all document objects.
/// Returns max_handle + 1 (the first safe handle to allocate).
/// This is needed because DWG-loaded documents may have next_handle
/// below some object handles (DWG reader doesn't call resolve_references).
fn compute_max_handle(document: &CadDocument) -> u64 {
    let mut max = document.next_handle();

    for entity in document.entities() {
        let h = entity.common().handle.value();
        if h >= max {
            max = h + 1;
        }
        if let EntityType::Insert(insert) = entity {
            for attribute in &insert.attributes {
                max = max.max(attribute.common.handle.value().saturating_add(1));
            }
        }
    }
    for (handle, _) in &document.objects {
        let h = handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for br in document.block_records.iter() {
        let h = br.handle.value();
        if h >= max {
            max = h + 1;
        }
        for eh in &br.entity_handles {
            let h = eh.value();
            if h >= max {
                max = h + 1;
            }
        }
        // BLOCK/ENDBLK markers are excluded from document.entities(), so scan
        // their handles here explicitly to avoid re-issuing them.
        let h = br.block_entity_handle.value();
        if h >= max {
            max = h + 1;
        }
        let h = br.block_end_handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.layers.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.line_types.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.text_styles.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.dim_styles.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.app_ids.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.views.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.vports.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }
    for r in document.ucss.iter() {
        let h = r.handle.value();
        if h >= max {
            max = h + 1;
        }
    }

    max
}

/// Convenience function to write a document to a file
pub fn write_dxf<P: AsRef<Path>>(document: &CadDocument, path: P) -> Result<()> {
    let writer = DxfWriter::new(document);
    writer.write_to_file(path)
}

/// Convenience function to write a document to a binary DXF file
pub fn write_binary_dxf<P: AsRef<Path>>(document: &CadDocument, path: P) -> Result<()> {
    let writer = DxfWriter::new_binary(document);
    writer.write_to_file(path)
}
