//! S22 mgmt calls (prd-sew-22 §16.2): the compliance routes for the mgmt service. Every `POST`
//! carries an `Idempotency-Key` (C9); a shorter retention still needs a preview.

use sew_proto::api::compliance::{
    ComplianceChannel, ComplianceChannelsQuery, CreateHoldRequest, EffectiveRetention,
    EffectiveRetentionQuery, HoldsQuery, LegalHold, PreviewRetentionRequest, ReleaseHoldRequest,
    RetentionListQuery, RetentionPolicy, RetentionPreview, RetentionRun, RunRetentionResponse,
    RunsQuery, SetRetentionRequest, UpdateHoldRequest,
};
use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::Id;
use sew_proto::routes::{
    MGMT_COMPLIANCE_CHANNELS, MGMT_COMPLIANCE_HOLD, MGMT_COMPLIANCE_HOLD_RELEASE,
    MGMT_COMPLIANCE_HOLD_UPDATE, MGMT_COMPLIANCE_HOLDS, MGMT_COMPLIANCE_RETENTION,
    MGMT_COMPLIANCE_RETENTION_CANCEL, MGMT_COMPLIANCE_RETENTION_EFFECTIVE,
    MGMT_COMPLIANCE_RETENTION_PREVIEW, MGMT_COMPLIANCE_RETENTION_PREVIEW_GET,
    MGMT_COMPLIANCE_RETENTION_RUN, MGMT_COMPLIANCE_RETENTION_RUNS, MGMT_COMPLIANCE_RETENTION_SET,
    with_id,
};

use super::MgmtClient;
use crate::api::with_query;
use crate::config::RequestOptions;
use crate::error::ClientError;

impl MgmtClient {
    /// `GET /mgmt/v1/compliance/retention`.
    pub async fn mgmt_retention_policies(
        &self,
        q: &RetentionListQuery,
    ) -> Result<Page<RetentionPolicy>, ClientError> {
        self.client
            .get(&with_query(MGMT_COMPLIANCE_RETENTION, Some(q))?)
            .await
    }

    /// `GET /mgmt/v1/compliance/retention/effective?channel_id=`.
    pub async fn mgmt_retention_effective(
        &self,
        channel: Id,
    ) -> Result<EffectiveRetention, ClientError> {
        let q = EffectiveRetentionQuery {
            channel_id: channel,
        };
        self.client
            .get(&with_query(MGMT_COMPLIANCE_RETENTION_EFFECTIVE, Some(&q))?)
            .await
    }

    /// `POST /mgmt/v1/compliance/retention/preview`.
    pub async fn mgmt_retention_preview(
        &self,
        req: &PreviewRetentionRequest,
        opts: RequestOptions,
    ) -> Result<RetentionPreview, ClientError> {
        self.client
            .post_with(MGMT_COMPLIANCE_RETENTION_PREVIEW, req, opts)
            .await
    }

    /// `GET /mgmt/v1/compliance/retention/previews/{id}`.
    pub async fn mgmt_retention_preview_get(
        &self,
        id: Id,
    ) -> Result<RetentionPreview, ClientError> {
        self.client
            .get(&with_id(MGMT_COMPLIANCE_RETENTION_PREVIEW_GET, id))
            .await
    }

    /// `POST /mgmt/v1/compliance/retention/set`.
    pub async fn mgmt_retention_set(
        &self,
        req: &SetRetentionRequest,
        opts: RequestOptions,
    ) -> Result<RetentionPolicy, ClientError> {
        self.client
            .post_with(MGMT_COMPLIANCE_RETENTION_SET, req, opts)
            .await
    }

    /// `POST /mgmt/v1/compliance/retention/{id}/cancel`.
    pub async fn mgmt_retention_cancel(
        &self,
        id: Id,
        opts: RequestOptions,
    ) -> Result<RetentionPolicy, ClientError> {
        self.client
            .post_with(
                &with_id(MGMT_COMPLIANCE_RETENTION_CANCEL, id),
                &EmptyRequest::default(),
                opts,
            )
            .await
    }

    /// `POST /mgmt/v1/compliance/retention/run`.
    pub async fn mgmt_retention_run(
        &self,
        opts: RequestOptions,
    ) -> Result<RunRetentionResponse, ClientError> {
        self.client
            .post_with(
                MGMT_COMPLIANCE_RETENTION_RUN,
                &EmptyRequest::default(),
                opts,
            )
            .await
    }

    /// `GET /mgmt/v1/compliance/retention/runs`.
    pub async fn mgmt_retention_runs(
        &self,
        q: &RunsQuery,
    ) -> Result<Page<RetentionRun>, ClientError> {
        self.client
            .get(&with_query(MGMT_COMPLIANCE_RETENTION_RUNS, Some(q))?)
            .await
    }

    /// `GET /mgmt/v1/compliance/holds`.
    pub async fn mgmt_holds(&self, q: &HoldsQuery) -> Result<Page<LegalHold>, ClientError> {
        self.client
            .get(&with_query(MGMT_COMPLIANCE_HOLDS, Some(q))?)
            .await
    }

    /// `POST /mgmt/v1/compliance/holds`.
    pub async fn mgmt_hold_create(
        &self,
        req: &CreateHoldRequest,
        opts: RequestOptions,
    ) -> Result<LegalHold, ClientError> {
        self.client
            .post_with(MGMT_COMPLIANCE_HOLDS, req, opts)
            .await
    }

    /// `GET /mgmt/v1/compliance/holds/{id}`.
    pub async fn mgmt_hold(&self, id: Id) -> Result<LegalHold, ClientError> {
        self.client.get(&with_id(MGMT_COMPLIANCE_HOLD, id)).await
    }

    /// `POST /mgmt/v1/compliance/holds/{id}/update`.
    pub async fn mgmt_hold_update(
        &self,
        id: Id,
        req: &UpdateHoldRequest,
        opts: RequestOptions,
    ) -> Result<LegalHold, ClientError> {
        self.client
            .post_with(&with_id(MGMT_COMPLIANCE_HOLD_UPDATE, id), req, opts)
            .await
    }

    /// `POST /mgmt/v1/compliance/holds/{id}/release`.
    pub async fn mgmt_hold_release(
        &self,
        id: Id,
        req: &ReleaseHoldRequest,
        opts: RequestOptions,
    ) -> Result<LegalHold, ClientError> {
        self.client
            .post_with(&with_id(MGMT_COMPLIANCE_HOLD_RELEASE, id), req, opts)
            .await
    }

    /// `GET /mgmt/v1/compliance/channels`.
    pub async fn mgmt_compliance_channels(
        &self,
        q: &ComplianceChannelsQuery,
    ) -> Result<Page<ComplianceChannel>, ClientError> {
        self.client
            .get(&with_query(MGMT_COMPLIANCE_CHANNELS, Some(q))?)
            .await
    }
}
