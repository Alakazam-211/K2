//! An API key's hire envelope (`api_keys.hire_limits`, 0160). The rule:
//! **an agent an app creates never exceeds the access of the key it uses.**
//! Parsed fail-closed: a stored envelope this daemon can't read means no
//! hire at all.

use serde::{Deserialize, Serialize};

use super::HireError;

pub const DEFAULT_MAX_PER_DAY: u32 = 20;
pub const DEFAULT_MAX_LIVE: u32 = 100;
pub const DEFAULT_MAX_PER_TEAM: u32 = 12;
/// Upper bounds the owner can't exceed at mint (sanity, not policy).
pub const CAP_MAX_PER_DAY: u32 = 1000;
pub const CAP_MAX_LIVE: u32 = 5000;
pub const CAP_MAX_PER_TEAM: u32 = super::recipe::MAX_MEMBERS as u32;

/// Which of the key's existing `/v1` doors also reach the agents it made
/// (T16, HT20). Never wider than the key's own doors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Reach {
    /// Hire, link and retire only (default).
    None,
    /// `POST /v1/w/<ws>/message` (if the key has canonical-message).
    Message,
    /// Message plus host sessions (if the key has those doors).
    HostSessions,
}

/// The stored envelope. Unknown fields refuse (fail closed).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HireLimits {
    pub max_members_per_day: u32,
    pub max_live_members: u32,
    pub max_members_per_team: u32,
    /// Highest `dbAccess` a hired agent may get. Above `off` also needs the
    /// key's own DB door.
    pub db: String,
    /// May create/remove team links.
    pub link: bool,
    /// `created` (only workspaces this key made) or explicit workspace ids.
    pub link_targets: LinkTargets,
    /// May tear down instances THIS key made.
    pub retire: bool,
    pub reach: Reach,
    /// Sub-folder of the hire root this key's agents go into (`""` = root).
    #[serde(default)]
    pub folder: String,
    /// S5 grant ceiling: the Apps a hired agent may get a login on, and
    /// which whole rooms / roles there (T17: no folder lines yet). Empty =
    /// no app logins.
    #[serde(default)]
    pub apps: Vec<AppCeiling>,
}

/// One App a key's hired agents may log in to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppCeiling {
    /// Published service id (resolved at mint).
    pub app: String,
    /// Workspace ids, or `self` (the hired member's own workspace).
    #[serde(default)]
    pub rooms: Vec<String>,
    /// App role names/ids a hired agent may be given.
    #[serde(default)]
    pub roles: Vec<String>,
}

/// The mint request's form of [`AppCeiling`].
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppCeilingIn {
    pub app: String,
    #[serde(default)]
    pub rooms: Option<Vec<String>>,
    #[serde(default)]
    pub roles: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LinkTargets {
    /// `"created"`.
    Keyword(String),
    /// Workspace ids the owner named (e.g. an owner-made Desk).
    Ids(Vec<String>),
}

impl Default for HireLimits {
    fn default() -> Self {
        Self {
            max_members_per_day: DEFAULT_MAX_PER_DAY,
            max_live_members: DEFAULT_MAX_LIVE,
            max_members_per_team: DEFAULT_MAX_PER_TEAM,
            db: "off".into(),
            link: false,
            link_targets: LinkTargets::Keyword("created".into()),
            retire: false,
            reach: Reach::None,
            folder: String::new(),
            apps: Vec::new(),
        }
    }
}

/// The mint request's `hire` object: every field optional (defaults
/// above). Unknown fields refuse.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HireLimitsIn {
    #[serde(default)]
    pub max_members_per_day: Option<u32>,
    #[serde(default)]
    pub max_live_members: Option<u32>,
    #[serde(default)]
    pub max_members_per_team: Option<u32>,
    #[serde(default)]
    pub db: Option<String>,
    #[serde(default)]
    pub link: Option<bool>,
    #[serde(default)]
    pub link_targets: Option<LinkTargets>,
    #[serde(default)]
    pub retire: Option<bool>,
    #[serde(default)]
    pub reach: Option<Reach>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub apps: Option<Vec<AppCeilingIn>>,
}

impl HireLimits {
    /// Normalize a mint request. `key_has_db` = the key's own DB door.
    pub fn from_request(v: &serde_json::Value, key_has_db: bool) -> Result<Self, HireError> {
        let input: HireLimitsIn = if v.is_null() || v.as_object().is_some_and(|o| o.is_empty()) || v.as_bool() == Some(true) {
            HireLimitsIn::default()
        } else {
            serde_json::from_value(v.clone()).map_err(|e| HireError::usage(format!("hire limits: {e}")))?
        };
        let d = Self::default();
        let bound = |name: &str, v: Option<u32>, def: u32, cap: u32| -> Result<u32, HireError> {
            let n = v.unwrap_or(def);
            if n == 0 || n > cap {
                return Err(HireError::usage(format!("hire limits: {name} must be 1–{cap}")));
            }
            Ok(n)
        };
        let db = input.db.unwrap_or_else(|| "off".into());
        if !super::recipe::DB_ACCESS.contains(&db.as_str()) {
            return Err(HireError::usage("hire limits: db is off, read or write"));
        }
        if db != "off" && !key_has_db {
            return Err(HireError::usage("hire limits: db above 'off' needs the key's own db door (allow db)"));
        }
        let link_targets = match input.link_targets {
            None => LinkTargets::Keyword("created".into()),
            Some(LinkTargets::Keyword(k)) if k == "created" => LinkTargets::Keyword(k),
            Some(LinkTargets::Keyword(k)) => {
                return Err(HireError::usage(format!("hire limits: linkTargets is \"created\" or a list of workspace ids, not '{k}'")))
            }
            Some(LinkTargets::Ids(ids)) => {
                let ids: Vec<String> = ids.into_iter().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                if ids.is_empty() || ids.len() > 200 {
                    return Err(HireError::usage("hire limits: linkTargets lists 1–200 workspace ids"));
                }
                LinkTargets::Ids(ids)
            }
        };
        Ok(Self {
            max_members_per_day: bound("maxMembersPerDay", input.max_members_per_day, d.max_members_per_day, CAP_MAX_PER_DAY)?,
            max_live_members: bound("maxLiveMembers", input.max_live_members, d.max_live_members, CAP_MAX_LIVE)?,
            max_members_per_team: bound("maxMembersPerTeam", input.max_members_per_team, d.max_members_per_team, CAP_MAX_PER_TEAM)?,
            db,
            link: input.link.unwrap_or(false),
            link_targets,
            retire: input.retire.unwrap_or(false),
            reach: input.reach.unwrap_or(Reach::None),
            folder: super::root::validate_sub_folder(input.folder.as_deref().unwrap_or(""))?,
            apps: Self::apps_from_request(input.apps.unwrap_or_default())?,
        })
    }

    fn apps_from_request(apps: Vec<AppCeilingIn>) -> Result<Vec<AppCeiling>, HireError> {
        if apps.len() > 20 {
            return Err(HireError::usage("hire limits: at most 20 apps"));
        }
        let mut out: Vec<AppCeiling> = Vec::new();
        for a in apps {
            let id = super::apps::resolve_app(&a.app)?
                .ok_or_else(|| HireError::usage(format!("hire limits: no published App {:?}", a.app)))?;
            if out.iter().any(|x| x.app == id) {
                return Err(HireError::usage(format!("hire limits: App {:?} twice", a.app)));
            }
            let rooms = match a.rooms {
                Some(r) => super::apps::resolve_rooms(&r)?,
                None => vec![super::apps::SELF_ROOM.to_string()],
            };
            let roles: Vec<String> = a
                .roles
                .unwrap_or_default()
                .into_iter()
                .map(|r| r.trim().to_string())
                .filter(|r| !r.is_empty())
                .collect();
            out.push(AppCeiling { app: id, rooms, roles });
        }
        Ok(out)
    }

    /// Does this key's ceiling allow an App login on `app` with these
    /// (resolved) rooms, or this role?
    pub fn allows_app_login(&self, app: &str, rooms: &[String], role: Option<&str>) -> bool {
        let Some(c) = self.apps.iter().find(|c| c.app == app) else { return false };
        match role {
            Some(r) => c.roles.iter().any(|x| x.eq_ignore_ascii_case(r.trim())),
            None => !rooms.is_empty() && rooms.iter().all(|room| c.rooms.contains(room)),
        }
    }

    /// Parse a stored envelope. `None` on anything unreadable (fail closed).
    pub fn parse_stored(raw: Option<&str>) -> Option<Self> {
        let raw = raw?.trim();
        if raw.is_empty() {
            return None;
        }
        let l: Self = serde_json::from_str(raw).ok()?;
        // Re-check the bounds a hand-edited row might break.
        if l.max_members_per_day == 0
            || l.max_live_members == 0
            || l.max_members_per_team == 0
            || !super::recipe::DB_ACCESS.contains(&l.db.as_str())
            || super::root::validate_sub_folder(&l.folder).is_err()
        {
            return None;
        }
        Some(l)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The `dbAccess` ceiling for this key (`off` when the key lost its DB door).
    pub fn db_ceiling(&self, key_has_db: bool) -> &'static str {
        if !key_has_db {
            return "off";
        }
        match self.db.as_str() {
            "write" => "write",
            "read" => "read",
            _ => "off",
        }
    }

    /// Does `reach` admit `door` for a workspace this key created?
    pub fn reach_allows(&self, door: ReachDoor) -> bool {
        match (self.reach, door) {
            (Reach::None, _) => false,
            (Reach::Message, ReachDoor::Message) => true,
            (Reach::Message, _) => false,
            (Reach::HostSessions, ReachDoor::Message | ReachDoor::HostSessions) => true,
            (Reach::HostSessions, ReachDoor::Other) => false,
        }
    }

    /// May this key link to workspace `id` (besides ones it created)?
    pub fn names_link_target(&self, id: &str) -> bool {
        match &self.link_targets {
            LinkTargets::Ids(ids) => ids.iter().any(|x| x == id),
            LinkTargets::Keyword(_) => false,
        }
    }
}

/// The `/v1` door a request comes through, for `reach`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReachDoor {
    Message,
    HostSessions,
    /// Sandboxes, DB, wiki/context writes: never reached through `reach`.
    Other,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        let l = HireLimits::from_request(&serde_json::json!({}), false).unwrap();
        assert_eq!(l, HireLimits::default());
        let back = HireLimits::parse_stored(Some(&l.to_json())).unwrap();
        assert_eq!(back, l);
        assert_eq!(HireLimits::from_request(&serde_json::Value::Bool(true), false).unwrap(), l);
    }

    #[test]
    fn stored_garbage_fails_closed() {
        assert!(HireLimits::parse_stored(None).is_none());
        assert!(HireLimits::parse_stored(Some("")).is_none());
        assert!(HireLimits::parse_stored(Some("{")).is_none());
        let mut v: serde_json::Value = serde_json::from_str(&HireLimits::default().to_json()).unwrap();
        v["surprise"] = serde_json::json!(true);
        assert!(HireLimits::parse_stored(Some(&v.to_string())).is_none(), "unknown field refuses");
        let mut z: serde_json::Value = serde_json::from_str(&HireLimits::default().to_json()).unwrap();
        z["maxLiveMembers"] = serde_json::json!(0);
        assert!(HireLimits::parse_stored(Some(&z.to_string())).is_none());
    }

    #[test]
    fn mint_rules() {
        assert!(HireLimits::from_request(&serde_json::json!({"db": "write"}), false).is_err(), "db needs the key's db door");
        assert_eq!(HireLimits::from_request(&serde_json::json!({"db": "read"}), true).unwrap().db, "read");
        assert!(HireLimits::from_request(&serde_json::json!({"maxLiveMembers": 0}), false).is_err());
        assert!(HireLimits::from_request(&serde_json::json!({"folder": "../x"}), false).is_err());
        assert!(HireLimits::from_request(&serde_json::json!({"linkTargets": "everything"}), false).is_err());
        assert!(HireLimits::from_request(&serde_json::json!({"unknown": 1}), false).is_err());
        let l = HireLimits::from_request(&serde_json::json!({"linkTargets": ["ws-1"], "link": true, "reach": "message"}), false).unwrap();
        assert!(l.names_link_target("ws-1"));
        assert!(!l.names_link_target("ws-2"));
        assert!(l.reach_allows(ReachDoor::Message));
        assert!(!l.reach_allows(ReachDoor::HostSessions));
    }

    #[test]
    fn app_ceiling_is_whole_rooms_or_named_roles() {
        let l = HireLimits {
            apps: vec![AppCeiling { app: "svc-1".into(), rooms: vec!["self".into(), "ws-a".into()], roles: vec!["Reader".into()] }],
            ..HireLimits::default()
        };
        assert!(l.allows_app_login("svc-1", &["self".into()], None));
        assert!(l.allows_app_login("svc-1", &["self".into(), "ws-a".into()], None));
        assert!(!l.allows_app_login("svc-1", &["ws-b".into()], None), "a room the owner didn't name");
        assert!(!l.allows_app_login("svc-1", &[], None), "no rooms is no login");
        assert!(l.allows_app_login("svc-1", &[], Some("reader")));
        assert!(!l.allows_app_login("svc-1", &[], Some("admin")));
        assert!(!l.allows_app_login("svc-2", &["self".into()], None), "another App");
        assert!(!HireLimits::default().allows_app_login("svc-1", &["self".into()], None), "no apps = no logins");
        let unknown_app = HireLimits::from_request(&serde_json::json!({ "apps": [{ "app": "no-such-service" }] }), false);
        assert!(unknown_app.is_err(), "an App must exist at mint");
    }

    #[test]
    fn db_ceiling_follows_the_key_door() {
        let l = HireLimits { db: "write".into(), ..HireLimits::default() };
        assert_eq!(l.db_ceiling(true), "write");
        assert_eq!(l.db_ceiling(false), "off");
    }
}
