//! S8's calls: auth, `me`, gateway ticket, health (S8 §17.4, S25 §6.5).

use bytes::Bytes;
use http::Method;
use sew_proto::api::auth::{
    GatewayTicketResponse, K2PassRequest, LoginRequest, PasswordSetRequest, RenewRequest,
    SessionResponse,
};
use sew_proto::api::members::Member;
use sew_proto::http::{EmptyRequest, OkResponse};
use sew_proto::mgmt::Health;
use sew_proto::routes::{
    AUTH_K2, AUTH_LOGIN, AUTH_LOGOUT, AUTH_PASSWORD_SET, AUTH_RENEW, GATEWAY_TICKET, HEALTH, ME,
};

use crate::api::{Client, decode};
use crate::config::RequestOptions;
use crate::error::ClientError;
use crate::session::Provider;

const NO_AUTH_AS_AGENT: &str = "auth calls are not available when acting as an agent (C9)";

impl Client {
    /// `POST` an auth body and store the session it returns in the token cell.
    async fn session_call<B: serde::Serialize>(
        &self,
        route: &str,
        body: &B,
        provider: Provider,
    ) -> Result<SessionResponse, ClientError> {
        self.refuse_act_as(NO_AUTH_AS_AGENT)?;
        let bytes = serde_json::to_vec(body)
            .map_err(|e| ClientError::invalid("body", format!("does not serialize: {e}")))?;
        let (resp, meta) = self
            .execute(
                Method::POST,
                route.to_owned(),
                Some(Bytes::from(bytes)),
                RequestOptions::default(),
            )
            .await?;
        let s: SessionResponse = decode(resp.status, &resp.body)?;
        let cell = self.session_cell()?;
        cell.set_session(&s, meta.server_date, provider, self.clock().as_ref());
        Ok(s)
    }

    /// `POST /api/v1/auth/k2` (S8): a K2 pass for a session. On success the token is stored in
    /// this client's token cell. Errors: 401 `unauthenticated` (bad pass), 403
    /// `not_provisioned`, 503 `unavailable` (JWKS down). Never retried once sent (a pass is
    /// single use).
    pub async fn auth_k2(&self, pass: &str) -> Result<SessionResponse, ClientError> {
        let body = K2PassRequest {
            pass: pass.to_owned(),
        };
        self.session_call(AUTH_K2, &body, Provider::K2).await
    }

    /// `POST /api/v1/auth/renew` (S8) with the current Bearer and a fresh pass (same `sub` and
    /// `sid`). On success the new token replaces the old in the token cell. Errors: 401
    /// `unauthenticated`, `session_expired`.
    pub async fn auth_renew(&self, pass: &str) -> Result<SessionResponse, ClientError> {
        let body = RenewRequest {
            pass: pass.to_owned(),
        };
        self.session_call(AUTH_RENEW, &body, Provider::K2).await
    }

    /// `POST /api/v1/auth/login` (S8, standalone). On success the token is stored in the token
    /// cell. Errors: 400 `validation_failed`, 401 `unauthenticated`, 429 `rate_limited`
    /// (throttle; not retried).
    pub async fn auth_login(
        &self,
        username: &str,
        password: &str,
    ) -> Result<SessionResponse, ClientError> {
        let username = sew_proto::addr::Handle::new(username)
            .map_err(|e| ClientError::invalid("username", e.to_string()))?;
        let body = LoginRequest {
            username,
            password: password.to_owned(),
        };
        self.session_call(AUTH_LOGIN, &body, Provider::Standalone)
            .await
    }

    /// `POST /api/v1/auth/password/set` (S8, standalone) with a one-time setup token. Returns
    /// no session. Errors: 400 `validation_failed`, 401 `unauthenticated`.
    pub async fn auth_password_set(
        &self,
        token: &str,
        password: &str,
    ) -> Result<OkResponse, ClientError> {
        self.refuse_act_as(NO_AUTH_AS_AGENT)?;
        self.post(
            AUTH_PASSWORD_SET,
            &PasswordSetRequest {
                token: token.to_owned(),
                password: password.to_owned(),
            },
        )
        .await
    }

    /// `POST /api/v1/auth/logout` (S8): revokes this session server-side and clears the token
    /// cell after a 200. A live gateway then gets 4011.
    pub async fn auth_logout(&self) -> Result<OkResponse, ClientError> {
        self.refuse_act_as(NO_AUTH_AS_AGENT)?;
        let r: OkResponse = self.post(AUTH_LOGOUT, &EmptyRequest {}).await?;
        if let Ok(cell) = self.session_cell() {
            cell.clear();
        }
        Ok(r)
    }

    /// `GET /api/v1/me` (S8): the signed-in member, or the agent under act-as. Errors: 401
    /// `unauthenticated`, `session_expired`; 403 `act_as_person_forbidden` under act-as.
    pub async fn me(&self) -> Result<Member, ClientError> {
        let m: Member = self.get(ME).await?;
        self.set_member_id(m.id);
        Ok(m)
    }

    /// `POST /api/v1/gateway/ticket` (S8): a single-use gateway ticket (30 s). Unsupported under
    /// act-as (agents have no gateway, C9).
    pub async fn gateway_ticket(&self) -> Result<GatewayTicketResponse, ClientError> {
        self.refuse_act_as("agents have no gateway (C9)")?;
        self.post(GATEWAY_TICKET, &EmptyRequest {}).await
    }

    /// `GET /api/v1/health` (S8, public).
    pub async fn health(&self) -> Result<Health, ClientError> {
        self.get(HEALTH).await
    }
}
