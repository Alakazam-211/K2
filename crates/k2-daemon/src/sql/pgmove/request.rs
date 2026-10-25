//! `POST /cli/db/upgrade`: the strict body (hostmail-style, V8). The
//! permission gate is PG-A's `versions_access::decide` (§12); `k2::acts_for`
//! maps a request onto it.
//!
//! Body: exactly one scope (`db`, `app` or `all`), then flags. Unknown
//! keys and wrong types are a 400, so a typo never starts a real move.

use serde_json::Value;

use super::Major;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Db(String),
    App(String),
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Move,
    Check,
    Rollback,
    ForgetOld,
    AcknowledgeFailed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpgradeRequest {
    pub scope: Option<Scope>,
    pub act: Act,
    pub to: Option<Major>,
    pub yes: bool,
    pub now: bool,
    pub allow_adopted: bool,
    pub force_now: bool,
    /// `--app` when K2 can't tell the App's database (PA2): move every
    /// database of its workspace.
    pub all_workspace_dbs: bool,
}

fn db_name_ok(s: &str) -> bool {
    k2_root_proto::verbs::DbName::parse(s).is_some() && s != "postgres"
}

fn app_id_ok(s: &str) -> bool {
    k2_root_proto::apps::AppId::parse(s).is_some()
}

pub fn parse(body: &[u8]) -> Result<UpgradeRequest, String> {
    let trimmed = std::str::from_utf8(body).map(str::trim).map_err(|_| "body is not UTF-8".to_string())?;
    let v: Value = if trimmed.is_empty() {
        return Err("body must name a database: {\"db\": \"<name>\"}, {\"app\": \"<id>\"} or {\"all\": true}".into());
    } else {
        serde_json::from_str(trimmed).map_err(|e| format!("invalid JSON body: {e}"))?
    };
    let obj = v.as_object().ok_or("body must be a JSON object")?;
    let mut r = UpgradeRequest { scope: None, act: Act::Move, to: None, yes: false, now: false, allow_adopted: false, force_now: false, all_workspace_dbs: false };
    let mut acts = Vec::new();
    let mut scopes = 0;
    for (k, val) in obj {
        let flag = || val.as_bool().ok_or_else(|| format!("'{k}' must be true or false"));
        match k.as_str() {
            "db" => {
                let s = val.as_str().filter(|s| db_name_ok(s)).ok_or("'db' must be a K2 database name ([a-z0-9_], ≤ 63)")?;
                r.scope = Some(Scope::Db(s.into()));
                scopes += 1;
            }
            "app" => {
                let s = val.as_str().filter(|s| app_id_ok(s)).ok_or("'app' must be an App id")?;
                r.scope = Some(Scope::App(s.into()));
                scopes += 1;
            }
            "all" => {
                if flag()? {
                    r.scope = Some(Scope::All);
                    scopes += 1;
                }
            }
            "to" => {
                let m = val.as_u64().ok_or("'to' must be a major number")?;
                r.to = Some(u32::try_from(m).map_err(|_| "'to' is out of range")?);
            }
            "check" => {
                if flag()? {
                    acts.push(Act::Check)
                }
            }
            "rollback" => {
                if flag()? {
                    acts.push(Act::Rollback)
                }
            }
            "forgetOld" => {
                if flag()? {
                    acts.push(Act::ForgetOld)
                }
            }
            "acknowledgeFailed" => {
                if flag()? {
                    acts.push(Act::AcknowledgeFailed)
                }
            }
            "yes" => r.yes = flag()?,
            "now" => r.now = flag()?,
            "allowAdopted" => r.allow_adopted = flag()?,
            "forceNow" => r.force_now = flag()?,
            "allWorkspaceDbs" => r.all_workspace_dbs = flag()?,
            other => return Err(format!("unknown field '{other}'")),
        }
    }
    if acts.len() > 1 {
        return Err("check, rollback, forgetOld and acknowledgeFailed are separate calls".into());
    }
    r.act = acts.first().copied().unwrap_or(Act::Move);
    if scopes > 1 {
        return Err("one scope: db, app or all".into());
    }
    match (r.act, &r.scope) {
        (Act::AcknowledgeFailed, None) => {}
        (Act::AcknowledgeFailed, Some(_)) => return Err("acknowledgeFailed takes no scope".into()),
        (_, None) => return Err("name a database: db, app or all".into()),
        (Act::Rollback | Act::ForgetOld, Some(Scope::App(_) | Scope::All)) => {
            return Err("rollback and forgetOld take one database (db)".into())
        }
        _ => {}
    }
    if r.act == Act::ForgetOld && r.to.is_some() {
        return Err("forgetOld takes no 'to'".into());
    }
    if r.force_now && r.act != Act::ForgetOld {
        return Err("forceNow is a forgetOld flag".into());
    }
    if r.yes && r.act != Act::Rollback {
        return Err("yes is a rollback flag".into());
    }
    if r.all_workspace_dbs && !matches!(r.scope, Some(Scope::App(_))) {
        return Err("allWorkspaceDbs goes with app".into());
    }
    Ok(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<UpgradeRequest, String> {
        parse(s.as_bytes())
    }

    #[test]
    fn strict_body() {
        let r = p(r#"{"db":"ws_jobs","to":17}"#).unwrap();
        assert_eq!(r.scope, Some(Scope::Db("ws_jobs".into())));
        assert_eq!((r.act, r.to), (Act::Move, Some(17)));
        assert_eq!(p(r#"{"db":"ws_jobs","check":true}"#).unwrap().act, Act::Check);
        assert_eq!(p(r#"{"db":"ws_jobs","rollback":true,"yes":true}"#).unwrap().act, Act::Rollback);
        assert_eq!(p(r#"{"app":"jobs"}"#).unwrap().scope, Some(Scope::App("jobs".into())));
        assert_eq!(p(r#"{"all":true,"check":true}"#).unwrap().scope, Some(Scope::All));
        assert_eq!(p(r#"{"acknowledgeFailed":true}"#).unwrap().act, Act::AcknowledgeFailed);
        assert!(p(r#"{"app":"jobs","allWorkspaceDbs":true}"#).unwrap().all_workspace_dbs);
        for bad in [
            "",
            "[]",
            r#"{"db":"ws_jobs","dry_run":true}"#,
            r#"{"db":"ws_jobs","check":"yes"}"#,
            r#"{"db":"WS_JOBS"}"#,
            r#"{"db":"postgres"}"#,
            r#"{"db":"a;b"}"#,
            r#"{"db":"ws_jobs","app":"jobs"}"#,
            r#"{"db":"ws_jobs","check":true,"rollback":true}"#,
            r#"{"app":"jobs","rollback":true,"yes":true}"#,
            r#"{"all":true,"forgetOld":true}"#,
            r#"{"db":"ws_jobs","yes":true}"#,
            r#"{"db":"ws_jobs","forceNow":true}"#,
            r#"{"db":"ws_jobs","to":"17"}"#,
            r#"{"db":"ws_jobs","to":-1}"#,
            r#"{"check":true}"#,
            r#"{"db":"ws_jobs","acknowledgeFailed":true}"#,
            r#"{"db":"ws_jobs","allWorkspaceDbs":true}"#,
            r#"{"app":"jobs","allWorkspaceDbs":"yes"}"#,
        ] {
            assert!(p(bad).is_err(), "{bad}");
        }
    }
}
