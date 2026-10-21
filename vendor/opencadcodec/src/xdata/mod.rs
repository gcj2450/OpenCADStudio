//! Extended Data (XDATA) support
//!
//! Extended data is application-specific data that can be attached to entities.
//! It is stored in DXF files using group codes 1000-1071.

use crate::types::{Handle, Vector3};

/// Extended data value types
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum XDataValue {
    /// String value (group code 1000)
    String(String),
    /// Control string (group code 1002) - "{" or "}"
    ControlString(String),
    /// Layer name (group code 1003)
    LayerName(String),
    /// Binary data (group code 1004)
    BinaryData(Vec<u8>),
    /// Database handle (group code 1005)
    Handle(Handle),
    /// 3D point (group codes 1010, 1020, 1030)
    Point3D(Vector3),
    /// 3D position (group codes 1011, 1021, 1031)
    Position3D(Vector3),
    /// 3D displacement (group codes 1012, 1022, 1032)
    Displacement3D(Vector3),
    /// 3D direction (group codes 1013, 1023, 1033)
    Direction3D(Vector3),
    /// Real value (group code 1040)
    Real(f64),
    /// Distance (group code 1041)
    Distance(f64),
    /// Scale factor (group code 1042)
    ScaleFactor(f64),
    /// 16-bit integer (group code 1070)
    Integer16(i16),
    /// 32-bit integer (group code 1071)
    Integer32(i32),
}

/// Extended data record for a single application
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExtendedDataRecord {
    /// Application name (from group code 1001)
    pub application_name: String,
    /// Extended data values
    pub values: Vec<XDataValue>,
}

impl ExtendedDataRecord {
    /// Create a new extended data record
    pub fn new(application_name: impl Into<String>) -> Self {
        Self {
            application_name: application_name.into(),
            values: Vec::new(),
        }
    }

    /// Add a value to the extended data
    pub fn add_value(&mut self, value: XDataValue) {
        self.values.push(value);
    }

    /// Get the number of values
    pub fn len(&self) -> usize {
        self.values.len()
    }

    /// Check if the record is empty
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

/// Extended data collection for an entity
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ExtendedData {
    /// Extended data records, keyed by application name
    records: Vec<ExtendedDataRecord>,
    /// Raw DWG EED blobs captured during DWG read, used for verbatim
    /// round-trip. Each tuple is (app_handle_value, raw_bytes).
    /// This field is ignored by the DXF reader/writer.
    #[cfg_attr(feature = "serde", serde(skip))]
    pub raw_dwg_eed: Vec<(u64, Vec<u8>)>,
}

impl ExtendedData {
    /// Create a new extended data collection
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            raw_dwg_eed: Vec::new(),
        }
    }

    /// Add a record to the collection
    pub fn add_record(&mut self, record: ExtendedDataRecord) {
        self.records.push(record);
    }

    /// Insert a record, replacing the existing record for the same application.
    ///
    /// XDATA permits only one logical payload per registered application on an
    /// entity. Keeping this invariant here prevents callers from accidentally
    /// writing ambiguous duplicate records when metadata is edited repeatedly.
    pub fn upsert_record(&mut self, record: ExtendedDataRecord) {
        if let Some(existing) = self
            .records
            .iter_mut()
            .find(|existing| existing.application_name == record.application_name)
        {
            *existing = record;
        } else {
            self.records.push(record);
        }
    }

    // Update the modeled fields while retaining unrelated values owned by the app.
    pub(crate) fn merge_modeled_record(&mut self, replacement: &ExtendedDataRecord) {
        let mut record = self.remove_record(&replacement.application_name)
            .unwrap_or_else(|| ExtendedDataRecord::new(&replacement.application_name));
        let labeled_group = matches!(replacement.values.get(1), Some(XDataValue::ControlString(value)) if value == "{");
        if labeled_group {
            let start = record.values.iter().position(|value| Some(value) == replacement.values.first());
            if let Some(start) = start {
                let mut depth = 0;
                let end = record.values.iter().enumerate().skip(start + 1).find_map(|(index, value)| {
                    if let XDataValue::ControlString(value) = value {
                        if value == "{" { depth += 1; }
                        if value == "}" { depth -= 1; if depth == 0 { return Some(index + 1); } }
                    }
                    None
                });
                if let Some(end) = end {
                    record.values.splice(start..end, replacement.values.clone());
                } else {
                    record.values.extend(replacement.values.clone());
                }
            } else {
                record.values.extend(replacement.values.clone());
            }
        } else {
            let mut cursor = 0;
            for value in &replacement.values {
                if let Some(offset) = record.values[cursor..].iter().position(|old| std::mem::discriminant(old) == std::mem::discriminant(value)) {
                    cursor += offset;
                    record.values[cursor] = value.clone();
                    cursor += 1;
                } else {
                    record.values.push(value.clone());
                    cursor = record.values.len();
                }
            }
        }
        self.add_record(record);
    }

    /// Remove and return the record owned by `application_name`.
    pub fn remove_record(&mut self, application_name: &str) -> Option<ExtendedDataRecord> {
        let index = self
            .records
            .iter()
            .position(|record| record.application_name == application_name)?;
        Some(self.records.remove(index))
    }

    /// Get all records
    pub fn records(&self) -> &[ExtendedDataRecord] {
        &self.records
    }

    pub(crate) fn records_mut(&mut self) -> &mut [ExtendedDataRecord] {
        &mut self.records
    }

    /// Get a record by application name
    pub fn get_record(&self, application_name: &str) -> Option<&ExtendedDataRecord> {
        self.records
            .iter()
            .find(|r| r.application_name == application_name)
    }

    /// Get the number of records
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Check if the collection is empty
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Clear all records
    pub fn clear(&mut self) {
        self.records.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xdata_record_creation() {
        let mut record = ExtendedDataRecord::new("MYAPP");
        assert_eq!(record.application_name, "MYAPP");
        assert!(record.is_empty());

        record.add_value(XDataValue::String("test".to_string()));
        assert_eq!(record.len(), 1);
    }

    #[test]
    fn test_xdata_collection() {
        let mut xdata = ExtendedData::new();
        assert!(xdata.is_empty());

        let mut record = ExtendedDataRecord::new("APP1");
        record.add_value(XDataValue::Real(3.14));
        xdata.add_record(record);

        assert_eq!(xdata.len(), 1);
        assert!(xdata.get_record("APP1").is_some());
        assert!(xdata.get_record("APP2").is_none());
    }
}
