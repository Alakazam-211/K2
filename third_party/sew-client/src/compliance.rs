//! S22 calls (prd-sew-22 §16.1): retention policies, previews, runs, legal holds and the
//! compliance channel list. Only compliance-flag holders get past `403 compliance_required`.

use sew_proto::api::compliance::{
    ComplianceChannel, ComplianceChannelsQuery, CreateHoldRequest, EffectiveRetention,
    EffectiveRetentionQuery, HoldsQuery, LegalHold, PreviewRetentionRequest, ReleaseHoldRequest,
    RetentionListQuery, RetentionPolicy, RetentionPreview, RetentionRun, RunRetentionResponse,
    RunsQuery, SetRetentionRequest, UpdateHoldRequest,
};
use sew_proto::http::{EmptyRequest, Page};
use sew_proto::id::Id;
use sew_proto::routes::{self as r, with_id};

use crate::api::with_query;
use crate::{Client, ClientError};

impl Client {
    /// `GET /api/v1/compliance/retention`.
    pub async fn retention_policies(
        &self,
        q: &RetentionListQuery,
    ) -> Result<Page<RetentionPolicy>, ClientError> {
        self.get(&with_query(r::COMPLIANCE_RETENTION, Some(q))?)
            .await
    }

    /// `GET /api/v1/compliance/retention/effective?channel_id=`.
    pub async fn retention_effective(
        &self,
        channel: Id,
    ) -> Result<EffectiveRetention, ClientError> {
        let q = EffectiveRetentionQuery {
            channel_id: channel,
        };
        self.get(&with_query(r::COMPLIANCE_RETENTION_EFFECTIVE, Some(&q))?)
            .await
    }

    /// `POST /api/v1/compliance/retention/preview` (`202`, the preview runs as a job).
    pub async fn retention_preview(
        &self,
        req: &PreviewRetentionRequest,
    ) -> Result<RetentionPreview, ClientError> {
        self.post(r::COMPLIANCE_RETENTION_PREVIEW, req).await
    }

    /// `GET /api/v1/compliance/retention/previews/{id}`.
    pub async fn retention_preview_get(&self, id: Id) -> Result<RetentionPreview, ClientError> {
        self.get(&with_id(r::COMPLIANCE_RETENTION_PREVIEW_GET, id))
            .await
    }

    /// `POST /api/v1/compliance/retention/set`.
    pub async fn retention_set(
        &self,
        req: &SetRetentionRequest,
    ) -> Result<RetentionPolicy, ClientError> {
        self.post(r::COMPLIANCE_RETENTION_SET, req).await
    }

    /// `POST /api/v1/compliance/retention/{id}/cancel`.
    pub async fn retention_cancel(&self, id: Id) -> Result<RetentionPolicy, ClientError> {
        self.post(
            &with_id(r::COMPLIANCE_RETENTION_CANCEL, id),
            &EmptyRequest::default(),
        )
        .await
    }

    /// `POST /api/v1/compliance/retention/run`.
    pub async fn retention_run(&self) -> Result<RunRetentionResponse, ClientError> {
        self.post(r::COMPLIANCE_RETENTION_RUN, &EmptyRequest::default())
            .await
    }

    /// `GET /api/v1/compliance/retention/runs`.
    pub async fn retention_runs(&self, q: &RunsQuery) -> Result<Page<RetentionRun>, ClientError> {
        self.get(&with_query(r::COMPLIANCE_RETENTION_RUNS, Some(q))?)
            .await
    }

    /// `GET /api/v1/compliance/holds`.
    pub async fn holds(&self, q: &HoldsQuery) -> Result<Page<LegalHold>, ClientError> {
        self.get(&with_query(r::COMPLIANCE_HOLDS, Some(q))?).await
    }

    /// `POST /api/v1/compliance/holds`.
    pub async fn hold_create(&self, req: &CreateHoldRequest) -> Result<LegalHold, ClientError> {
        self.post(r::COMPLIANCE_HOLDS, req).await
    }

    /// `GET /api/v1/compliance/holds/{id}`.
    pub async fn hold(&self, id: Id) -> Result<LegalHold, ClientError> {
        self.get(&with_id(r::COMPLIANCE_HOLD, id)).await
    }

    /// `POST /api/v1/compliance/holds/{id}/update` (additive only).
    pub async fn hold_update(
        &self,
        id: Id,
        req: &UpdateHoldRequest,
    ) -> Result<LegalHold, ClientError> {
        self.post(&with_id(r::COMPLIANCE_HOLD_UPDATE, id), req)
            .await
    }

    /// `POST /api/v1/compliance/holds/{id}/release`.
    pub async fn hold_release(
        &self,
        id: Id,
        req: &ReleaseHoldRequest,
    ) -> Result<LegalHold, ClientError> {
        self.post(&with_id(r::COMPLIANCE_HOLD_RELEASE, id), req)
            .await
    }

    /// `GET /api/v1/compliance/channels`.
    pub async fn compliance_channels(
        &self,
        q: &ComplianceChannelsQuery,
    ) -> Result<Page<ComplianceChannel>, ClientError> {
        self.get(&with_query(r::COMPLIANCE_CHANNELS, Some(q))?)
            .await
    }
}
