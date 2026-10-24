//! Vault vocabulary: namespaces, scopes, kinds, deliveries, consumers.

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};

macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub enum $name { $($(#[$vm])* #[serde(rename = $s)] $v),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];
            pub fn as_str(&self) -> &'static str { match self { $($name::$v => $s),+ } }
            pub fn parse(s: &str) -> Result<$name> {
                match s { $($s => Ok($name::$v),)+ _ => Err(VaultError::bad_request(
                    concat!("bad_", stringify!($name)),
                    format!("unknown {}: {s}", stringify!($name)))) }
            }
            /// Stable byte for the binary AAD/MAC encoding.
            pub fn code(&self) -> u8 {
                $name::ALL.iter().position(|x| x == self).expect("listed") as u8
            }
        }
        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
        }
    };
}

str_enum!(
    /// `system`: K2's own secrets; daemon only, never returned by a route.
    /// `user`: owner, agent and app secrets, used through grants.
    Namespace { System = "system", User = "user" }
);

str_enum!(
    ScopeKind {
        Server = "server",
        Workspace = "workspace",
        /// Reserved until app installs exist (KV27).
        App = "app",
        Session = "session",
        Engine = "engine",
        /// A person's wallet (an App user, 0.46.3): `system` namespace
        /// only ([`Scope::validate_for`]); the `user` namespace and `k2v://`
        /// refs still refuse it.
        Person = "person",
    }
);

str_enum!(
    Kind {
        Generic = "generic",
        Password = "password",
        ApiKey = "api_key",
        OauthToken = "oauth_token",
        SshKey = "ssh_key",
        TlsKey = "tls_key",
        Dsn = "dsn",
        /// Webhook / endpoint URLs (VS11).
        Url = "url",
        LoginSlot = "login_slot",
        AppPass = "app_pass",
        EngineCred = "engine_cred",
        SigningKey = "signing_key",
        HmacKey = "hmac_key",
    }
);

str_enum!(
    /// How a consumer receives a secret. `Fetch`/`Proxy` never show the
    /// value; `File`/`Env` let the consumer read it; `Api` prints it.
    Delivery {
        Fetch = "fetch",
        Proxy = "proxy",
        File = "file",
        Env = "env",
        Api = "api",
        None = "none",
    }
);

str_enum!(
    SecretStatus { Active = "active", Disabled = "disabled", Destroyed = "destroyed" }
);

str_enum!(
    VersionState { Active = "active", Revoked = "revoked", Destroyed = "destroyed" }
);

str_enum!(
    ConsumerKind {
        Workspace = "workspace",
        Session = "session",
        PublishedService = "published_service",
        App = "app",
        Engine = "engine",
        Daemon = "daemon",
        /// Reserved for Zen v3 widgets (§10.8).
        Widget = "widget",
        /// A daemon-run scheduled job (`k2 job`, prd-vault-gaps-v1 item 5).
        /// Appended last (R3: `code()` is the position in `ALL`). The id is
        /// `<workspace id>/<job id>`, so the rules can tell an agent's own
        /// jobs from another workspace's without asking k2.db.
        Job = "job",
    }
);

impl Delivery {
    /// Exposure level: fetch/proxy (0) < file (1) < env (2) < api (3).
    /// `None` is below everything (the secret isn't delivered at all).
    pub fn level(&self) -> i8 {
        match self {
            Delivery::None => -1,
            Delivery::Fetch | Delivery::Proxy => 0,
            Delivery::File => 1,
            Delivery::Env => 2,
            Delivery::Api => 3,
        }
    }

    /// Can the consumer read the value with this delivery?
    pub fn readable(&self) -> bool {
        self.level() >= 1
    }
}

impl Kind {
    pub fn default_delivery(&self) -> Delivery {
        match self {
            Kind::SshKey | Kind::TlsKey => Delivery::File,
            Kind::Password | Kind::Dsn | Kind::Url => Delivery::Env,
            _ => Delivery::Fetch,
        }
    }

    /// Values of these kinds carry a destination (VS11): an agent's new
    /// version must keep the old host and port.
    pub fn carries_destination(&self) -> bool {
        matches!(self, Kind::Dsn | Kind::Url)
    }

    /// Pasted values of these kinds are trimmed of edge whitespace (VS5).
    pub fn trims_on_save(&self) -> bool {
        matches!(self, Kind::ApiKey | Kind::Password | Kind::OauthToken)
    }
}

/// A secret's scope.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Scope {
    pub kind: ScopeKind,
    /// Empty for `server`.
    #[serde(default)]
    pub id: String,
}

impl Scope {
    pub fn server() -> Scope {
        Scope { kind: ScopeKind::Server, id: String::new() }
    }
    pub fn workspace(id: impl Into<String>) -> Scope {
        Scope { kind: ScopeKind::Workspace, id: id.into() }
    }
    pub fn session(id: impl Into<String>) -> Scope {
        Scope { kind: ScopeKind::Session, id: id.into() }
    }
    pub fn engine(id: impl Into<String>) -> Scope {
        Scope { kind: ScopeKind::Engine, id: id.into() }
    }
    /// A person's wallet: `id` = the App user's principal id.
    pub fn person(id: impl Into<String>) -> Scope {
        Scope { kind: ScopeKind::Person, id: id.into() }
    }

    /// [`Self::validate`] for a write into `namespace`: the `person` scope
    /// opens in the `system` namespace only (a person's wallet is K2's
    /// own secret: no route ever returns a system value). Everything else
    /// is [`Self::validate`].
    pub fn validate_for(&self, namespace: Namespace) -> Result<()> {
        if self.kind == ScopeKind::Person && namespace == Namespace::System {
            if self.id.is_empty() || self.id.len() > 128 || self.id.chars().any(|c| c.is_control()) {
                return Err(VaultError::bad_request("bad_scope", "person scope needs an id"));
            }
            return Ok(());
        }
        self.validate()
    }

    pub fn validate(&self) -> Result<()> {
        match self.kind {
            ScopeKind::Server if !self.id.is_empty() => {
                Err(VaultError::bad_request("bad_scope", "server scope has no id"))
            }
            ScopeKind::Server => Ok(()),
            ScopeKind::App | ScopeKind::Person => {
                Err(VaultError::bad_request("scope_reserved", format!("{} scope isn't available yet", self.kind)))
            }
            _ if self.id.is_empty() || self.id.len() > 128 || self.id.chars().any(|c| c.is_control()) => {
                Err(VaultError::bad_request("bad_scope", format!("{} scope needs an id", self.kind)))
            }
            _ => Ok(()),
        }
    }
}

/// Who receives a secret.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Consumer {
    pub kind: ConsumerKind,
    /// `*` with `Workspace` means "all my agents".
    pub id: String,
}

impl Consumer {
    pub fn workspace(id: impl Into<String>) -> Consumer {
        Consumer { kind: ConsumerKind::Workspace, id: id.into() }
    }
    pub fn session(id: impl Into<String>) -> Consumer {
        Consumer { kind: ConsumerKind::Session, id: id.into() }
    }
    pub fn published_service(id: impl Into<String>) -> Consumer {
        Consumer { kind: ConsumerKind::PublishedService, id: id.into() }
    }
    pub fn engine(id: impl Into<String>) -> Consumer {
        Consumer { kind: ConsumerKind::Engine, id: id.into() }
    }
    /// A `k2 job` in `workspace_id` (`job_id` = `jobs.id` in k2.db).
    pub fn job(workspace_id: &str, job_id: &str) -> Consumer {
        Consumer { kind: ConsumerKind::Job, id: format!("{workspace_id}/{job_id}") }
    }
    /// The workspace half of a job consumer's id.
    pub fn job_workspace(&self) -> Option<&str> {
        if self.kind != ConsumerKind::Job {
            return None;
        }
        self.id.split_once('/').map(|(ws, _)| ws).filter(|ws| !ws.is_empty())
    }
    pub fn all_workspaces() -> Consumer {
        Consumer { kind: ConsumerKind::Workspace, id: "*".into() }
    }
    pub fn label(&self) -> String {
        format!("{}:{}", self.kind, self.id)
    }
}

/// Names: `[A-Za-z0-9_.-]{1,128}` (KV1). User-facing names ≤ 64.
pub fn validate_name(name: &str, user_facing: bool) -> Result<()> {
    let max = if user_facing { 64 } else { 128 };
    if name.is_empty() || name.len() > max {
        return Err(VaultError::bad_request("bad_name", format!("names are 1–{max} characters")));
    }
    if !name.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-')) {
        return Err(VaultError::bad_request("bad_name", "names use letters, digits, _ . -"));
    }
    if name == "." || name == ".." {
        return Err(VaultError::bad_request("bad_name", "not a name"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_round_trip_and_codes_are_stable() {
        for d in Delivery::ALL {
            assert_eq!(Delivery::parse(d.as_str()).expect("parses"), *d);
        }
        assert_eq!(Namespace::System.code(), 0);
        assert_eq!(Namespace::User.code(), 1);
        assert!(Kind::parse("nope").is_err());
        // R3: new enum values go last, so every older code stays put.
        assert_eq!(ConsumerKind::Job.code() as usize, ConsumerKind::ALL.len() - 1);
        assert_eq!(ConsumerKind::Widget.code(), 6);
    }

    #[test]
    fn job_consumers_carry_their_workspace() {
        let c = Consumer::job("ws-1", "job-9");
        assert_eq!(c.id, "ws-1/job-9");
        assert_eq!(c.label(), "job:ws-1/job-9");
        assert_eq!(c.job_workspace(), Some("ws-1"));
        assert_eq!(Consumer::workspace("ws-1").job_workspace(), None);
        assert_eq!(Consumer { kind: ConsumerKind::Job, id: "/x".into() }.job_workspace(), None);
    }

    #[test]
    fn names() {
        validate_name("STRIPE_KEY", true).expect("ok");
        validate_name("mailsec_admin_0123456789ab", false).expect("ok");
        validate_name("ext-inbox-12-oauth", false).expect("ok");
        assert!(validate_name("", true).is_err());
        assert!(validate_name("a b", true).is_err());
        assert!(validate_name("../x", true).is_err());
        assert!(validate_name(&"a".repeat(65), true).is_err());
        validate_name(&"a".repeat(128), false).expect("128 ok for system");
    }

    #[test]
    fn delivery_levels() {
        assert!(Delivery::Fetch.level() < Delivery::File.level());
        assert!(Delivery::File.level() < Delivery::Env.level());
        assert!(Delivery::Env.level() < Delivery::Api.level());
        assert_eq!(Delivery::Proxy.level(), Delivery::Fetch.level());
        assert!(!Delivery::Fetch.readable());
        assert!(Delivery::File.readable());
    }

    #[test]
    fn scopes() {
        Scope::server().validate().expect("ok");
        Scope::workspace("ws-1").validate().expect("ok");
        assert!(Scope::workspace("").validate().is_err());
        assert_eq!(Scope { kind: ScopeKind::App, id: "x".into() }.validate().expect_err("reserved").code, "scope_reserved");
    }

    #[test]
    fn the_person_scope_opens_in_the_system_namespace_only() {
        Scope::person("p-1").validate_for(Namespace::System).expect("system ok");
        assert_eq!(Scope::person("p-1").validate_for(Namespace::User).expect_err("user").code, "scope_reserved");
        assert_eq!(Scope::person("p-1").validate().expect_err("plain validate").code, "scope_reserved");
        assert_eq!(Scope::person("").validate_for(Namespace::System).expect_err("id").code, "bad_scope");
        // App stays reserved everywhere.
        assert_eq!(Scope { kind: ScopeKind::App, id: "x".into() }.validate_for(Namespace::System).expect_err("app").code, "scope_reserved");
    }
}
