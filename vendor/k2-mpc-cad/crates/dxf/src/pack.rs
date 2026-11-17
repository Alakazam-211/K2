//! The DXF pack: the kit's tier-0 [`BlobPack`], indexing each version into the kit's
//! structure index through the format-neutral model ([`k2_mpc_cad_model::index`]).

use std::io::Read;

use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs};
use k2_mpc_base::session::{AgentWrites, BlobMerge, BlobPack};
use k2_mpc_cad_model::merge::MergeOutcome;

use crate::{mutant, read_doc, Limits};

/// The index's output format: bump when what a file indexes to changes, and the kit recomputes.
pub const FORMAT: u32 = 3;

/// The index verbs, under the noun `entity`.
pub const VERBS: IndexVerbs = IndexVerbs::new(
    "entity list",
    "entity show",
    "entity diff",
    "entity blame",
    None,
);

/// Reads ASCII DXF files.
#[derive(Debug, Clone, Copy, Default)]
pub struct DxfPack;

/// The index draft of `bytes`.
pub fn index_bytes(bytes: &[u8], limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    let doc = read_doc(bytes, &Limits::default())
        .map_err(|e| IndexRefused::Unreadable { why: e.summary() })?;
    k2_mpc_cad_model::index::draft(&doc, limits)
}

impl BlobPack for DxfPack {
    /// Agents never write a DXF file whole (`write`, `write --base`): the kit refuses it
    /// (`MPC-E715`) and the file is unchanged. This release is read-only; agents will change a
    /// drawing through its edit verbs, never by replacing the file.
    fn agent_writes(&self) -> AgentWrites {
        if mutant::active("AgentWritesAllowed") {
            return AgentWrites::Allowed;
        }
        AgentWrites::Refused
    }

    /// The merge by unit and field ([`crate::merge`]).
    fn can_merge(&self) -> bool {
        true
    }

    /// A person's app saved `theirs` over a version the doc has moved on from: merged by unit
    /// and field, else both kept (a conflict copy). Never one side's value over the other's.
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        match crate::merge::merge(base, ours, theirs) {
            MergeOutcome::Merged { bytes, .. } => BlobMerge::Merged(bytes),
            MergeOutcome::Conflict(_) => BlobMerge::Conflict,
            MergeOutcome::Unsupported(_) => BlobMerge::Unsupported,
        }
    }

    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new("dxf", FORMAT, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let rl = Limits::default();
        let mut bytes = Vec::new();
        src.take(rl.max_bytes.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(IndexRefused::Read)?;
        if bytes.len() as u64 > rl.max_bytes {
            return Err(IndexRefused::Unreadable {
                why: format!(
                    "DXF-E003 over_bound: over the size limit of {} bytes",
                    rl.max_bytes
                ),
            });
        }
        // the model holds what the index needs: the bytes go before the draft is built
        let doc =
            read_doc(&bytes, &rl).map_err(|e| IndexRefused::Unreadable { why: e.summary() })?;
        drop(bytes);
        k2_mpc_cad_model::index::draft(&doc, limits)
    }
}
