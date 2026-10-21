//! High-level failsafe recovery results.

use crate::error::{DxfError, Result};
use crate::io::read::{ReadOutcome, ReadStats};
use crate::{CadDocument, RepairReport};

/// A recovered document together with its reader statistics and repair report.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RecoverOutcome {
    /// The usable document reconstructed from the source.
    pub document: CadDocument,
    /// Source/decode diagnostics and record counts.
    pub stats: ReadStats,
    /// Audit and conservative repair results.
    pub repair: RepairReport,
}

impl RecoverOutcome {
    /// Finish a failsafe reader outcome by applying conservative repairs.
    pub fn from_read_outcome(read: ReadOutcome) -> Result<Self> {
        if !read.stats.has_usable_drawing_data() {
            let details = if read.stats.diagnostics.is_empty() {
                "the reader produced no usable drawing records".to_string()
            } else {
                format!(
                    "the reader produced no usable drawing records ({} diagnostics)",
                    read.stats.diagnostics.len()
                )
            };
            return Err(DxfError::RecoveryFailed(details));
        }

        let ReadOutcome {
            mut document,
            stats,
        } = read;
        let repair = document.repair();
        Ok(Self {
            document,
            stats,
            repair,
        })
    }
}
