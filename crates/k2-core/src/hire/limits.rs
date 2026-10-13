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
        })
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
    fn db_ceiling_follows_the_key_door() {
        let l = HireLimits { db: "write".into(), ..HireLimits::default() };
        assert_eq!(l.db_ceiling(true), "write");
        assert_eq!(l.db_ceiling(false), "off");
    }
}
