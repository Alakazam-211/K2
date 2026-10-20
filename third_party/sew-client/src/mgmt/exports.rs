//! S23 mgmt calls (prd-sew-23 §14.2): the audit log, the compliance flag by external id, full
//! exports, and the eDiscovery twins for the mgmt service. Every `POST` carries an
//! `Idempotency-Key` (C9). Archives stream over the socket with the `sew` CLI's downloader
//! (`GET /mgmt/v1/exports/{id}/archive`, `MGMT_EXPORT_ARCHIVE`); this client reads the rows.

use sew_proto::api::audit::{AuditEntry, AuditQuery};
use sew_proto::api::exports::{
    ComplianceFlagResponse, CreateEdiscoveryRequest, Export, ExportEstimate, ExportsQuery,
};
use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::Id;
use sew_proto::mgmt::PrincipalComplianceRequest;
use sew_proto::routes::{
    MGMT_AUDIT, MGMT_COMPLIANCE_EXPORT, MGMT_COMPLIANCE_EXPORT_CANCEL,
    MGMT_COMPLIANCE_EXPORT_DELETE, MGMT_COMPLIANCE_EXPORTS, MGMT_COMPLIANCE_EXPORTS_ESTIMATE,
    MGMT_EXPORT_CANCEL, MGMT_EXPORT_DELETE, MGMT_EXPORT_GET, MGMT_EXPORTS,
    MGMT_PRINCIPALS_COMPLIANCE, with_id,
};

use super::MgmtClient;
use crate::api::with_query;
use crate::config::RequestOptions;
use crate::error::ClientError;
use crate::exports::audit_path;

impl MgmtClient {
    /// `GET /mgmt/v1/audit`: every row (the service sees all classes).
    pub async fn mgmt_audit(&self, q: &AuditQuery) -> Result<Page<AuditEntry>, ClientError> {
        self.client.get(&audit_path(MGMT_AUDIT, q)?).await
    }

    /// `POST /mgmt/v1/principals/compliance`: grant or revoke the flag by external id.
    pub async fn principal_compliance(
        &self,
        req: &PrincipalComplianceRequest,
        opts: RequestOptions,
    ) -> Result<ComplianceFlagResponse, ClientError> {
        self.client
            .post_with(MGMT_PRINCIPALS_COMPLIANCE, req, opts)
            .await
    }

    /// `GET /mgmt/v1/exports`: full and eDiscovery exports.
    pub async fn exports(&self, q: &ExportsQuery) -> Result<Page<Export>, ClientError> {
        self.client.get(&with_query(MGMT_EXPORTS, Some(q))?).await
    }

    /// `GET /mgmt/v1/exports/{id}`.
    pub async fn export(&self, id: Id) -> Result<Export, ClientError> {
        self.client.get(&with_id(MGMT_EXPORT_GET, id)).await
    }

    /// `POST /mgmt/v1/exports/{id}/cancel`.
    pub async fn export_cancel(&self, id: Id, opts: RequestOptions) -> Result<Export, ClientError> {
        self.client
            .post_with(&with_id(MGMT_EXPORT_CANCEL, id), &EmptyRequest {}, opts)
            .await
    }

    /// `POST /mgmt/v1/exports/{id}/delete`.
    pub async fn export_delete(&self, id: Id, opts: RequestOptions) -> Result<Export, ClientError> {
        self.client
            .post_with(&with_id(MGMT_EXPORT_DELETE, id), &EmptyRequest {}, opts)
            .await
    }

    /// `POST /mgmt/v1/compliance/exports` (`202`).
    pub async fn mgmt_ediscovery_create(
        &self,
        req: &CreateEdiscoveryRequest,
        opts: RequestOptions,
    ) -> Result<Export, ClientError> {
        self.client
            .post_with(MGMT_COMPLIANCE_EXPORTS, req, opts)
            .await
    }

    /// `GET /mgmt/v1/compliance/exports`.
    pub async fn mgmt_ediscovery_list(
        &self,
        q: &ExportsQuery,
    ) -> Result<Page<Export>, ClientError> {
        self.client
            .get(&with_query(MGMT_COMPLIANCE_EXPORTS, Some(q))?)
            .await
    }

    /// `POST /mgmt/v1/compliance/exports/estimate`.
    pub async fn mgmt_ediscovery_estimate(
        &self,
        req: &CreateEdiscoveryRequest,
        opts: RequestOptions,
    ) -> Result<ExportEstimate, ClientError> {
        self.client
            .post_with(MGMT_COMPLIANCE_EXPORTS_ESTIMATE, req, opts)
            .await
    }

    /// `GET /mgmt/v1/compliance/exports/{id}`.
    pub async fn mgmt_ediscovery(&self, id: Id) -> Result<Export, ClientError> {
        self.client.get(&with_id(MGMT_COMPLIANCE_EXPORT, id)).await
    }

    /// `POST /mgmt/v1/compliance/exports/{id}/cancel`.
    pub async fn mgmt_ediscovery_cancel(
        &self,
        id: Id,
        opts: RequestOptions,
    ) -> Result<Export, ClientError> {
        self.client
            .post_with(
                &with_id(MGMT_COMPLIANCE_EXPORT_CANCEL, id),
                &EmptyRequest {},
                opts,
            )
            .await
    }

    /// `POST /mgmt/v1/compliance/exports/{id}/delete`.
    pub async fn mgmt_ediscovery_delete(
        &self,
        id: Id,
        opts: RequestOptions,
    ) -> Result<Export, ClientError> {
        self.client
            .post_with(
                &with_id(MGMT_COMPLIANCE_EXPORT_DELETE, id),
                &EmptyRequest {},
                opts,
            )
            .await
    }
}
