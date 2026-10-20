//! One method per C9 route (S9 stage A, contracts revs 11, 17; S25 §12.2).

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::stream::{self, Stream};
use http::Method;
use sew_proto::api::media::{ClearSecretRequest, SecretStatus, SecretsResponse, SetSecretRequest};
use sew_proto::api::members::Member;
use sew_proto::http::EmptyRequest;
use sew_proto::id::Id;
use sew_proto::mgmt::AuditVerifyRequest;
use sew_proto::mgmt::local::{
    LocalGuestRequest, LocalRoleRequest, LocalUserCreateRequest, LocalUserRef, LocalUserResponse,
    SetupLinkRequest,
};
use sew_proto::mgmt::{
    DeliveriesPage, DeliveriesQuery, DeprovisionRequest, DeprovisionResponse, ExportRequest,
    Health, JobStatus, JobStatusPage, JobsQuery, MgmtBackupList, MgmtPrincipal, MgmtPrincipalPage,
    MgmtSpaceBindRequest, MgmtSpaceCreateRequest, MgmtSpaceMemberRemoveRequest,
    MgmtSpaceMemberRemoveResponse, MgmtSpaceMembersAddRequest, MgmtSpaceMembersAddResponse,
    MgmtSpaceResponse, OwnerSetRequest, OwnerSetResponse, PinRequest, PinResponse, PrincipalsQuery,
    ProvisionRequest, ProvisionResponse, RevokeSessionsRequest, RevokeSessionsResponse,
    SearchRebuildRequest, SearchRebuildResponse, StartJobResponse, Status, UpdatePrincipalRequest,
};
use sew_proto::policy::{PoliciesResponse, SetPoliciesRequest};
use sew_proto::routes::{
    MGMT_AGENT_DELIVERIES, MGMT_AGENTS_PIN, MGMT_AGENTS_UNPIN, MGMT_AUDIT_VERIFY, MGMT_BACKUP,
    MGMT_BACKUPS, MGMT_DEPROVISION, MGMT_EXPORT, MGMT_HEALTH, MGMT_JOB, MGMT_JOB_RETRY, MGMT_JOBS,
    MGMT_KEYS_ROTATE_MASTER, MGMT_LOCAL_REACTIVATE, MGMT_LOCAL_ROLE, MGMT_LOCAL_SETUP_LINK,
    MGMT_LOCAL_USERS, MGMT_LOCAL_USERS_GUEST, MGMT_METRICS, MGMT_OWNER_SET, MGMT_POLICIES,
    MGMT_PRINCIPALS, MGMT_PROVISION, MGMT_SEARCH_REBUILD, MGMT_SECRETS, MGMT_SECRETS_CLEAR,
    MGMT_SECRETS_SET, MGMT_SESSIONS_REVOKE, MGMT_SPACE_MEMBERS_ADD, MGMT_SPACE_MEMBERS_REMOVE,
    MGMT_SPACES_BIND, MGMT_SPACES_CREATE, MGMT_STATUS, MGMT_UPDATE, with_id,
};

use crate::api::{decode, with_query};
use crate::config::RequestOptions;
use crate::error::ClientError;

use super::MgmtClient;

/// Extra time on top of a deliveries long-poll before the request times out.
pub const LONG_POLL_SLACK: Duration = Duration::from_secs(10);

impl MgmtClient {
    async fn mget<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.client.get(path).await
    }

    async fn mpost<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
        opts: RequestOptions,
    ) -> Result<T, ClientError> {
        self.client.post_with(path, body, opts).await
    }

    /// `GET /mgmt/v1/health` (needs the token, rev 11).
    pub async fn health(&self) -> Result<Health, ClientError> {
        self.mget(MGMT_HEALTH).await
    }

    /// `GET /mgmt/v1/status`: version, install id, provider, db, jobs, keys, sessions, alarms.
    pub async fn status(&self) -> Result<Status, ClientError> {
        self.mget(MGMT_STATUS).await
    }

    /// `GET /mgmt/v1/metrics`: OpenMetrics text.
    pub async fn metrics(&self) -> Result<String, ClientError> {
        let (resp, _) = self
            .client
            .execute(
                Method::GET,
                MGMT_METRICS.to_owned(),
                None,
                RequestOptions::default(),
            )
            .await?;
        Ok(String::from_utf8_lossy(&resp.body).into_owned())
    }

    /// `POST /mgmt/v1/principals/provision`: a declarative upsert by `external_id`. Errors: 400,
    /// 409 `conflict` (`field = username`), 422 `assistant_person_unknown` /
    /// `assistant_person_inactive`.
    pub async fn provision(
        &self,
        req: &ProvisionRequest,
        opts: RequestOptions,
    ) -> Result<ProvisionResponse, ClientError> {
        self.mpost(MGMT_PROVISION, req, opts).await
    }

    /// `POST /mgmt/v1/spaces/create` (SW11): a Space with an owner (`k2:owner` or a person),
    /// managers holding the non-ADMINISTRATOR manager role and an optional App. Idempotent on
    /// the slug. Errors: 400 (an agent owner), 409 `slug_taken`, 422 `target_not_eligible` /
    /// `principal_unknown`.
    pub async fn space_create(
        &self,
        req: &MgmtSpaceCreateRequest,
        opts: RequestOptions,
    ) -> Result<MgmtSpaceResponse, ClientError> {
        self.mpost(MGMT_SPACES_CREATE, req, opts).await
    }

    /// `POST /mgmt/v1/spaces/bind` (SW13): binds a Space to an App, once. Errors: 404, 409
    /// `space_bound`.
    pub async fn space_bind(
        &self,
        req: &MgmtSpaceBindRequest,
        opts: RequestOptions,
    ) -> Result<MgmtSpaceResponse, ClientError> {
        self.mpost(MGMT_SPACES_BIND, req, opts).await
    }

    /// `POST /mgmt/v1/spaces/members/add` (SW13): added, already and skipped per principal.
    /// Errors: 404, 409 `app_mismatch`.
    pub async fn space_members_add(
        &self,
        req: &MgmtSpaceMembersAddRequest,
        opts: RequestOptions,
    ) -> Result<MgmtSpaceMembersAddResponse, ClientError> {
        self.mpost(MGMT_SPACE_MEMBERS_ADD, req, opts).await
    }

    /// `POST /mgmt/v1/spaces/members/remove` (SW13). Errors: 404, 409 `app_mismatch`, 422 (the
    /// owner).
    pub async fn space_member_remove(
        &self,
        req: &MgmtSpaceMemberRemoveRequest,
        opts: RequestOptions,
    ) -> Result<MgmtSpaceMemberRemoveResponse, ClientError> {
        self.mpost(MGMT_SPACE_MEMBERS_REMOVE, req, opts).await
    }

    /// `POST /mgmt/v1/principals/update`: a partial profile update. Errors: 404, 409.
    pub async fn update_principal(
        &self,
        req: &UpdatePrincipalRequest,
        opts: RequestOptions,
    ) -> Result<Member, ClientError> {
        self.mpost(MGMT_UPDATE, req, opts).await
    }

    /// `POST /mgmt/v1/principals/deprovision`. Errors: 404, 422 (install owner).
    pub async fn deprovision(
        &self,
        req: &DeprovisionRequest,
        opts: RequestOptions,
    ) -> Result<DeprovisionResponse, ClientError> {
        self.mpost(MGMT_DEPROVISION, req, opts).await
    }

    /// `GET /mgmt/v1/principals`: one page, ascending by member id (gap S25-G14).
    pub async fn principals(&self, q: &PrincipalsQuery) -> Result<MgmtPrincipalPage, ClientError> {
        self.mget(&with_query(MGMT_PRINCIPALS, Some(q))?).await
    }

    /// Every principal matching `q`'s filters, page by page (`after` = the last id seen).
    pub fn principals_all(
        &self,
        q: PrincipalsQuery,
    ) -> impl Stream<Item = Result<MgmtPrincipal, ClientError>> + Send + '_ {
        struct St {
            q: PrincipalsQuery,
            buf: VecDeque<MgmtPrincipal>,
            done: bool,
        }
        let st = St {
            q,
            buf: VecDeque::new(),
            done: false,
        };
        stream::unfold(st, move |mut st| async move {
            loop {
                if let Some(p) = st.buf.pop_front() {
                    return Some((Ok(p), st));
                }
                if st.done {
                    return None;
                }
                match self.principals(&st.q).await {
                    Err(e) => {
                        st.done = true;
                        return Some((Err(e), st));
                    }
                    Ok(page) => {
                        st.done = !page.has_more || page.items.is_empty();
                        if let Some(last) = page.items.last() {
                            st.q.after = Some(last.member_id);
                        }
                        st.buf.extend(page.items);
                    }
                }
            }
        })
    }

    /// `POST /mgmt/v1/sessions/revoke` by external id and/or K2 `sid`; `200 {revoked: n}` even
    /// for unknown ids (rev 15).
    pub async fn sessions_revoke(
        &self,
        req: &RevokeSessionsRequest,
        opts: RequestOptions,
    ) -> Result<RevokeSessionsResponse, ClientError> {
        self.mpost(MGMT_SESSIONS_REVOKE, req, opts).await
    }

    /// `POST /mgmt/v1/owner/set` (with the current owner's approval when one exists).
    pub async fn owner_set(
        &self,
        req: &OwnerSetRequest,
        opts: RequestOptions,
    ) -> Result<OwnerSetResponse, ClientError> {
        self.mpost(MGMT_OWNER_SET, req, opts).await
    }

    /// `POST /mgmt/v1/agents/pin`: pin an assistant to a person (external ids).
    pub async fn agents_pin(
        &self,
        req: &PinRequest,
        opts: RequestOptions,
    ) -> Result<PinResponse, ClientError> {
        self.mpost(MGMT_AGENTS_PIN, req, opts).await
    }

    /// `POST /mgmt/v1/agents/unpin`.
    pub async fn agents_unpin(
        &self,
        req: &PinRequest,
        opts: RequestOptions,
    ) -> Result<PinResponse, ClientError> {
        self.mpost(MGMT_AGENTS_UNPIN, req, opts).await
    }

    /// `GET /mgmt/v1/agents/{id}/deliveries` (S9 stage B): one long-poll. The request timeout is
    /// `timeout_ms + 10 s`; an empty page after the long-poll is a normal `Ok`. Send the page's
    /// `next_after` as the next `after`. Errors: 400 (`after` past the head), 403
    /// `act_as_person_forbidden` / `not_provisioned`, 404, 410 `resync_required` (`floor`,
    /// `head`, `reason`).
    pub async fn agent_deliveries(
        &self,
        agent_member_id: Id,
        q: &DeliveriesQuery,
    ) -> Result<DeliveriesPage, ClientError> {
        let path = with_query(&with_id(MGMT_AGENT_DELIVERIES, agent_member_id), Some(q))?;
        let opts =
            RequestOptions::new().timeout(Duration::from_millis(q.timeout_ms()) + LONG_POLL_SLACK);
        let (resp, _) = self.client.execute(Method::GET, path, None, opts).await?;
        decode(resp.status, &resp.body)
    }

    /// [`MgmtClient::agent_deliveries`] plus the page's `sync_epoch` (contracts revs 16, 36), now
    /// the typed `DeliveriesPage::sync_epoch`.
    pub async fn agent_deliveries_with_epoch(
        &self,
        agent_member_id: Id,
        q: &DeliveriesQuery,
    ) -> Result<(DeliveriesPage, Option<String>), ClientError> {
        let page = self.agent_deliveries(agent_member_id, q).await?;
        let epoch = page.sync_epoch.clone();
        Ok((page, epoch))
    }

    /// `POST /mgmt/v1/backup`: start a backup job (S6).
    pub async fn backup_start(
        &self,
        opts: RequestOptions,
    ) -> Result<StartJobResponse, ClientError> {
        self.mpost(MGMT_BACKUP, &EmptyRequest {}, opts).await
    }

    /// `GET /mgmt/v1/backups` (SW15): the complete bundles, newest first. Errors: 503
    /// `backups_unavailable`.
    pub async fn backups(&self) -> Result<MgmtBackupList, ClientError> {
        self.mget(MGMT_BACKUPS).await
    }

    /// `POST /mgmt/v1/export`: start a full logical export to `req.recipients` (S23). The job id
    /// is the export id (`GET /mgmt/v1/exports/{id}`).
    pub async fn export_start(
        &self,
        req: &ExportRequest,
        opts: RequestOptions,
    ) -> Result<StartJobResponse, ClientError> {
        self.mpost(MGMT_EXPORT, req, opts).await
    }

    /// `POST /mgmt/v1/audit/verify`: start (or join) an audit chain check (S24 §7.7). The job's
    /// `result` is an `AuditVerifyReport` once it succeeded.
    pub async fn audit_verify_start(
        &self,
        req: &AuditVerifyRequest,
        opts: RequestOptions,
    ) -> Result<StartJobResponse, ClientError> {
        self.mpost(MGMT_AUDIT_VERIFY, req, opts).await
    }

    /// `GET /mgmt/v1/jobs`: newest first.
    pub async fn jobs(&self, q: &JobsQuery) -> Result<JobStatusPage, ClientError> {
        self.mget(&with_query(MGMT_JOBS, Some(q))?).await
    }

    /// `GET /mgmt/v1/jobs/{id}`. Errors: 404.
    pub async fn job(&self, id: Id) -> Result<JobStatus, ClientError> {
        self.mget(&with_id(MGMT_JOB, id)).await
    }

    /// `POST /mgmt/v1/jobs/{id}/retry`: requeue a dead job. Errors: 404, 409, 422.
    pub async fn job_retry(&self, id: Id, opts: RequestOptions) -> Result<JobStatus, ClientError> {
        self.mpost(&with_id(MGMT_JOB_RETRY, id), &EmptyRequest {}, opts)
            .await
    }

    /// `GET /mgmt/v1/policies`.
    pub async fn policies(&self) -> Result<PoliciesResponse, ClientError> {
        self.mget(MGMT_POLICIES).await
    }

    /// `POST /mgmt/v1/policies`: set org policies. Errors: 400.
    pub async fn policies_set(
        &self,
        req: &SetPoliciesRequest,
        opts: RequestOptions,
    ) -> Result<PoliciesResponse, ClientError> {
        self.mpost(MGMT_POLICIES, req, opts).await
    }

    /// `GET /mgmt/v1/secrets` (S16): install secrets' status; values are never returned.
    pub async fn secrets(&self) -> Result<SecretsResponse, ClientError> {
        self.mget(MGMT_SECRETS).await
    }

    /// `POST /mgmt/v1/secrets/set` (S16). Errors: 400 `unknown_secret`.
    pub async fn secrets_set(
        &self,
        req: &SetSecretRequest,
        opts: RequestOptions,
    ) -> Result<SecretStatus, ClientError> {
        self.mpost(MGMT_SECRETS_SET, req, opts).await
    }

    /// `POST /mgmt/v1/secrets/clear` (S16). Errors: 400 `unknown_secret`.
    pub async fn secrets_clear(
        &self,
        req: &ClearSecretRequest,
        opts: RequestOptions,
    ) -> Result<SecretStatus, ClientError> {
        self.mpost(MGMT_SECRETS_CLEAR, req, opts).await
    }

    /// `POST /mgmt/v1/keys/rotate-master`: start (or join) a master-key re-wrap job. Errors: 422
    /// `no_retiring_master_key`.
    pub async fn keys_rotate_master(
        &self,
        opts: RequestOptions,
    ) -> Result<StartJobResponse, ClientError> {
        self.mpost(MGMT_KEYS_ROTATE_MASTER, &EmptyRequest {}, opts)
            .await
    }

    /// `POST /mgmt/v1/search/rebuild` (S20 §14.5): enqueue an online rebuild of the search
    /// index (coalesced with a pending one); poll `GET /mgmt/v1/jobs/{id}` for the result.
    pub async fn search_rebuild(
        &self,
        req: &SearchRebuildRequest,
        opts: RequestOptions,
    ) -> Result<SearchRebuildResponse, ClientError> {
        self.mpost(MGMT_SEARCH_REBUILD, req, opts).await
    }

    /// `POST /mgmt/v1/mail/test` (S21 §12.6): send a fixed test mail through `[mail.smtp]`.
    /// Errors: 422 `mail_unconfigured`, 429.
    pub async fn mail_test(
        &self,
        req: &sew_proto::mgmt::MailTestRequest,
        opts: RequestOptions,
    ) -> Result<sew_proto::mgmt::MailTestResponse, ClientError> {
        self.mpost(sew_proto::routes::MGMT_MAIL_TEST, req, opts)
            .await
    }

    /// `POST /mgmt/v1/local/users` (standalone): create a local user. Errors: 403
    /// `provider_is_k2`, 409.
    pub async fn local_users_create(
        &self,
        req: &LocalUserCreateRequest,
        opts: RequestOptions,
    ) -> Result<LocalUserResponse, ClientError> {
        self.mpost(MGMT_LOCAL_USERS, req, opts).await
    }

    /// `POST /mgmt/v1/local/setup-link` (standalone): a setup link, or the first owner.
    pub async fn local_setup_link(
        &self,
        req: &SetupLinkRequest,
        opts: RequestOptions,
    ) -> Result<LocalUserResponse, ClientError> {
        self.mpost(MGMT_LOCAL_SETUP_LINK, req, opts).await
    }

    /// `POST /mgmt/v1/local/users/reactivate` (standalone).
    pub async fn local_users_reactivate(
        &self,
        req: &LocalUserRef,
        opts: RequestOptions,
    ) -> Result<LocalUserResponse, ClientError> {
        self.mpost(MGMT_LOCAL_REACTIVATE, req, opts).await
    }

    /// `POST /mgmt/v1/local/users/role` (standalone): set a local user's install role.
    pub async fn local_users_role(
        &self,
        req: &LocalRoleRequest,
        opts: RequestOptions,
    ) -> Result<LocalUserResponse, ClientError> {
        self.mpost(MGMT_LOCAL_ROLE, req, opts).await
    }

    /// `POST /mgmt/v1/local/users/guest` (standalone, S19): make a local user a guest or a
    /// member.
    pub async fn local_users_guest(
        &self,
        req: &LocalGuestRequest,
        opts: RequestOptions,
    ) -> Result<LocalUserResponse, ClientError> {
        self.mpost(MGMT_LOCAL_USERS_GUEST, req, opts).await
    }
}
