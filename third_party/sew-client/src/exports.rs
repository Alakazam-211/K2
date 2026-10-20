//! S23 calls (prd-sew-23 §6, §7, §14.1): the compliance flag, eDiscovery exports and the
//! audit log. Every export route needs the compliance flag (`403 compliance_required`).

use sew_proto::api::audit::{AuditChainStatus, AuditEntry, AuditQuery};
use sew_proto::api::exports::{
    ComplianceFlagResponse, ComplianceOfficers, CreateEdiscoveryRequest, Export, ExportEstimate,
    ExportsQuery, SetComplianceRequest, SignedLink,
};
use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::Id;
use sew_proto::mgmt::{AuditVerifyRequest, StartJobResponse};
use sew_proto::routes::{self as r, with_id};

use crate::api::with_query;
use crate::files::{ByteRange, Download};
use crate::{Client, ClientError};

/// The query string of an [`AuditQuery`]: `action` repeats once per value (the one list the
/// URL-encoded form cannot carry).
pub(crate) fn audit_path(base: &str, q: &AuditQuery) -> Result<String, ClientError> {
    let mut single = q.clone();
    let actions = std::mem::take(&mut single.action);
    let mut path = with_query(base, Some(&single))?;
    for a in &actions {
        let sep = if path.contains('?') { '&' } else { '?' };
        let enc: String = serde_urlencoded::to_string([("action", a)])
            .map_err(|e| ClientError::invalid("action", format!("does not encode: {e}")))?;
        path.push(sep);
        path.push_str(&enc);
    }
    Ok(path)
}

impl Client {
    /// `POST /api/v1/install/members/{id}/compliance` (install owner only).
    pub async fn compliance_set(
        &self,
        member: Id,
        on: bool,
    ) -> Result<ComplianceFlagResponse, ClientError> {
        self.post(
            &with_id(r::INSTALL_MEMBER_COMPLIANCE, member),
            &SetComplianceRequest { compliance: on },
        )
        .await
    }

    /// `GET /api/v1/compliance/officers`: who holds the flag.
    pub async fn compliance_officers(&self) -> Result<ComplianceOfficers, ClientError> {
        self.get(r::COMPLIANCE_OFFICERS).await
    }

    /// `POST /api/v1/compliance/exports` (`202`: the export runs as a job).
    pub async fn ediscovery_create(
        &self,
        req: &CreateEdiscoveryRequest,
    ) -> Result<Export, ClientError> {
        self.post(r::COMPLIANCE_EXPORTS, req).await
    }

    /// `GET /api/v1/compliance/exports`.
    pub async fn ediscovery_list(&self, q: &ExportsQuery) -> Result<Page<Export>, ClientError> {
        self.get(&with_query(r::COMPLIANCE_EXPORTS, Some(q))?).await
    }

    /// `POST /api/v1/compliance/exports/estimate` (recipients optional).
    pub async fn ediscovery_estimate(
        &self,
        req: &CreateEdiscoveryRequest,
    ) -> Result<ExportEstimate, ClientError> {
        self.post(r::COMPLIANCE_EXPORTS_ESTIMATE, req).await
    }

    /// `GET /api/v1/compliance/exports/{id}`.
    pub async fn ediscovery(&self, id: Id) -> Result<Export, ClientError> {
        self.get(&with_id(r::COMPLIANCE_EXPORT, id)).await
    }

    /// `POST /api/v1/compliance/exports/{id}/cancel` (queued or running).
    pub async fn ediscovery_cancel(&self, id: Id) -> Result<Export, ClientError> {
        self.post(
            &with_id(r::COMPLIANCE_EXPORT_CANCEL, id),
            &EmptyRequest::default(),
        )
        .await
    }

    /// `POST /api/v1/compliance/exports/{id}/delete` (done, failed or cancelled).
    pub async fn ediscovery_delete(&self, id: Id) -> Result<Export, ClientError> {
        self.post(
            &with_id(r::COMPLIANCE_EXPORT_DELETE, id),
            &EmptyRequest::default(),
        )
        .await
    }

    /// `POST /api/v1/compliance/exports/{id}/link`: a 1 h download link bound to the caller.
    pub async fn ediscovery_link(&self, id: Id) -> Result<SignedLink, ClientError> {
        self.post(
            &with_id(r::COMPLIANCE_EXPORT_LINK, id),
            &EmptyRequest::default(),
        )
        .await
    }

    /// The archive of export `id` through a fresh signed link (`GET
    /// /api/v1/compliance/exports/{id}/archive?m=…`), optionally one byte range.
    pub async fn ediscovery_archive(
        &self,
        id: Id,
        range: Option<ByteRange>,
    ) -> Result<Download, ClientError> {
        let link = self.ediscovery_link(id).await?;
        let path = with_id(r::COMPLIANCE_EXPORT_ARCHIVE, id);
        if !link.url.contains(&path) {
            return Err(ClientError::invalid(
                "url",
                "the signed link does not point at the export archive",
            ));
        }
        self.download(&link.url, range).await
    }

    /// `GET /api/v1/audit`: the audit rows the caller may see (§7.2).
    pub async fn audit(&self, q: &AuditQuery) -> Result<Page<AuditEntry>, ClientError> {
        self.get(&audit_path(r::AUDIT, q)?).await
    }

    /// `GET /api/v1/audit/chain`: the chain head and the last check (owner or flag).
    pub async fn audit_chain(&self) -> Result<AuditChainStatus, ClientError> {
        self.get(r::AUDIT_CHAIN).await
    }

    /// `POST /api/v1/audit/verify` (`202`): start (or join) a chain check.
    pub async fn audit_verify(&self, full: bool) -> Result<StartJobResponse, ClientError> {
        self.post(r::AUDIT_VERIFY, &AuditVerifyRequest { full })
            .await
    }
}
