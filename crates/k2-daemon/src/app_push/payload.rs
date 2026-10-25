//! What a custom App's notification is, checked (CP26–CP27), and what Apple
//! and Google receive (§8.3, CP29).
//!
//! K2 adds its own stamps: `k2App` (the App's service id), `k2Send` (the
//! send id), `k2Kind` (`app` | `agent`), `k2Topic` and `k2Who` (an opaque
//! per-(App, guest) key). No guest username, principal id, room name or K2
//! host name ever rides a push: Apple and Google must not see who.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value as J};

/// Apple's limit for an alert payload, and Google's for a message payload.
pub const MAX_PAYLOAD_BYTES: usize = 4096;
pub const MAX_TITLE_BYTES: usize = 200;
pub const MAX_BODY_BYTES: usize = 1024;
pub const MAX_DATA_KEYS: usize = 32;
pub const MAX_DATA_KEY_LEN: usize = 64;
pub const MAX_DATA_VALUE_BYTES: usize = 1024;
pub const MAX_DATA_JSON_BYTES: usize = 2048;
pub const MAX_COLLAPSE_BYTES: usize = 64;
pub const MAX_THREAD_BYTES: usize = 256;
pub const MAX_SOUND_BYTES: usize = 64;
pub const MAX_BADGE: u32 = 99_999;
pub const MAX_TTL_SECS: u32 = 2_419_200;
pub const DEFAULT_TTL_SECS: u32 = 86_400;
pub const MAX_CHANNEL_BYTES: usize = 64;

/// `k2Kind`.
pub const KIND_APP: &str = "app";
pub const KIND_AGENT: &str = "agent";

/// One notification, checked. `title`/`body` are `None` only when silent.
#[derive(Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: Option<String>,
    pub body: Option<String>,
    pub data: BTreeMap<String, String>,
    pub topic: Option<String>,
    pub collapse_id: Option<String>,
    pub thread_id: Option<String>,
    /// `None` = no sound field (`"none"` maps here too).
    pub sound: Option<String>,
    pub badge: Option<u32>,
    pub silent: bool,
    pub high_priority: bool,
    pub ttl_secs: u32,
    pub mutable: bool,
    pub android_channel: Option<String>,
}

impl std::fmt::Debug for Notification {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the text or the data values (CP41).
        f.debug_struct("Notification")
            .field("topic", &self.topic)
            .field("silent", &self.silent)
            .field("data_keys", &self.data.len())
            .finish()
    }
}

/// K2's per-device stamps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stamps {
    pub app: String,
    pub send_id: String,
    pub kind: &'static str,
    pub who: String,
}

/// A refusal of the request (`usage` or `too_large`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    Usage(String),
    TooLarge { limit: usize },
}

fn usage<T>(m: impl Into<String>) -> Result<T, Refused> {
    Err(Refused::Usage(m.into()))
}

fn opt_str<'a>(v: &'a J, k: &str) -> Result<Option<&'a str>, Refused> {
    match v.get(k) {
        None | Some(J::Null) => Ok(None),
        Some(J::String(s)) => Ok(Some(s.as_str())),
        Some(_) => usage(format!("{k} must be a string")),
    }
}

fn no_nul(k: &str, s: &str) -> Result<(), Refused> {
    if s.contains('\0') {
        return usage(format!("{k} can't contain a NUL character"));
    }
    Ok(())
}

/// Reserved `data` keys: K2's own stamps and the vendors' (CP26).
pub fn reserved_data_key(k: &str) -> bool {
    let l = k.to_ascii_lowercase();
    l == "aps"
        || l.starts_with("k2")
        || matches!(l.as_str(), "from" | "notification" | "message_type" | "collapse_key")
        || l.starts_with("google.")
        || l.starts_with("gcm.")
}

/// The request fields K2 reads (everything else is refused: CP3, CP24).
pub const FIELDS: &[&str] = &[
    "to",
    "title",
    "body",
    "data",
    "topic",
    "collapseId",
    "threadId",
    "sound",
    "badge",
    "silent",
    "priority",
    "ttlSecs",
    "mutable",
    "androidChannel",
];

/// Check one send request's notification (not `to`). Errors name the field;
/// never echo text or data values.
pub fn parse(v: &J) -> Result<Notification, Refused> {
    let Some(o) = v.as_object() else { return usage("send a JSON object") };
    if let Some(k) = o.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        let shown: String = k.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').take(32).collect();
        return usage(format!("unknown field {shown} (the App comes from your credential, never the body)"));
    }
    let silent = match v.get("silent") {
        None | Some(J::Null) => false,
        Some(J::Bool(b)) => *b,
        Some(_) => return usage("silent must be true or false"),
    };
    let title = opt_str(v, "title")?.map(str::trim).filter(|s| !s.is_empty());
    let body = opt_str(v, "body")?.map(str::trim).filter(|s| !s.is_empty());
    if silent {
        if title.is_some() || body.is_some() {
            return usage("a silent push carries no title or body (it wakes the app; put what it needs in data)");
        }
    } else {
        let (Some(t), Some(b)) = (title, body) else {
            return usage("title and body are required (or send silent: true)");
        };
        if t.len() > MAX_TITLE_BYTES {
            return usage(format!("title is at most {MAX_TITLE_BYTES} bytes"));
        }
        if b.len() > MAX_BODY_BYTES {
            return usage(format!("body is at most {MAX_BODY_BYTES} bytes"));
        }
        no_nul("title", t)?;
        no_nul("body", b)?;
    }
    let mut data = BTreeMap::new();
    match v.get("data") {
        None | Some(J::Null) => {}
        Some(J::Object(m)) => {
            if m.len() > MAX_DATA_KEYS {
                return usage(format!("data holds at most {MAX_DATA_KEYS} keys"));
            }
            for (k, x) in m {
                if k.is_empty() || k.len() > MAX_DATA_KEY_LEN || !k.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-')) {
                    return usage(format!("a data key is 1 to {MAX_DATA_KEY_LEN} of A-Z, a-z, 0-9, '_', '.' and '-'"));
                }
                if reserved_data_key(k) {
                    let shown: String = k.chars().take(MAX_DATA_KEY_LEN).collect();
                    return usage(format!("data key {shown} is reserved (aps, k2*, from, notification, message_type, collapse_key, google.*, gcm.*)"));
                }
                let Some(s) = x.as_str() else { return usage("data values are strings (send numbers as text)") };
                if s.len() > MAX_DATA_VALUE_BYTES {
                    return usage(format!("a data value is at most {MAX_DATA_VALUE_BYTES} bytes"));
                }
                data.insert(k.clone(), s.to_string());
            }
            let size = serde_json::to_string(&data).map(|s| s.len()).unwrap_or(usize::MAX);
            if size > MAX_DATA_JSON_BYTES {
                return usage(format!("data is at most {MAX_DATA_JSON_BYTES} bytes as JSON"));
            }
        }
        Some(_) => return usage("data is an object of strings"),
    }
    let topic = opt_str(v, "topic")?.map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(t) = &topic {
        if !k2_core::skin::app_push::valid_topic(t) {
            return usage("topic is 1 to 32 of a-z, 0-9, '.', '_' and '-'");
        }
    }
    let bounded = |k: &str, max: usize| -> Result<Option<String>, Refused> {
        let s = opt_str(v, k)?.map(str::trim).filter(|s| !s.is_empty());
        if let Some(s) = s {
            if s.len() > max {
                return usage(format!("{k} is at most {max} bytes"));
            }
            no_nul(k, s)?;
        }
        Ok(s.map(str::to_string))
    };
    let collapse_id = bounded("collapseId", MAX_COLLAPSE_BYTES)?;
    let thread_id = bounded("threadId", MAX_THREAD_BYTES)?;
    let sound = match opt_str(v, "sound")?.map(str::trim) {
        None | Some("") | Some("none") => None,
        Some(s) if s.len() <= MAX_SOUND_BYTES && s.bytes().all(|b| (0x20..0x7f).contains(&b)) => Some(s.to_string()),
        Some(_) => return usage(format!("sound is at most {MAX_SOUND_BYTES} printable ASCII characters, or none")),
    };
    let badge = match v.get("badge") {
        None | Some(J::Null) => None,
        Some(J::Number(n)) => match n.as_u64() {
            Some(b) if b <= MAX_BADGE as u64 => Some(b as u32),
            _ => return usage(format!("badge is 0 to {MAX_BADGE}")),
        },
        Some(_) => return usage(format!("badge is 0 to {MAX_BADGE}")),
    };
    let high_priority = match opt_str(v, "priority")? {
        None => !silent,
        Some("high") => true,
        Some("normal") => false,
        Some(_) => return usage("priority is high or normal"),
    };
    if silent && high_priority {
        return usage("a silent push is normal priority (Apple drops high-priority background pushes)");
    }
    let ttl_secs = match v.get("ttlSecs") {
        None | Some(J::Null) => DEFAULT_TTL_SECS,
        Some(J::Number(n)) => match n.as_u64() {
            Some(t) if t <= MAX_TTL_SECS as u64 => t as u32,
            _ => return usage(format!("ttlSecs is 0 to {MAX_TTL_SECS} (28 days)")),
        },
        Some(_) => return usage(format!("ttlSecs is 0 to {MAX_TTL_SECS} (28 days)")),
    };
    let mutable = match v.get("mutable") {
        None | Some(J::Null) => false,
        Some(J::Bool(b)) => *b,
        Some(_) => return usage("mutable must be true or false"),
    };
    let android_channel = bounded("androidChannel", MAX_CHANNEL_BYTES)?;
    if let Some(c) = &android_channel {
        if !c.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-')) {
            return usage("androidChannel is A-Z, a-z, 0-9, '_', '.' and '-'");
        }
    }
    let n = Notification {
        title: title.map(str::to_string),
        body: body.map(str::to_string),
        data,
        topic,
        collapse_id,
        thread_id,
        sound,
        badge,
        silent,
        high_priority,
        ttl_secs,
        mutable,
        android_channel,
    };
    fits(&n)?;
    Ok(n)
}

/// The built payloads must fit Apple's and Google's 4,096 bytes (A7). A
/// placeholder of the real stamps' length stands in for them.
pub fn fits(n: &Notification) -> Result<(), Refused> {
    let s = Stamps { app: "x".repeat(40), send_id: "x".repeat(SEND_ID_LEN), kind: KIND_AGENT, who: "x".repeat(32) };
    if apns_body(n, &s).len() > MAX_PAYLOAD_BYTES || fcm_payload_bytes(n, &s) > MAX_PAYLOAD_BYTES {
        return Err(Refused::TooLarge { limit: MAX_PAYLOAD_BYTES });
    }
    Ok(())
}

/// A send id: `snd_` + 32 hex.
pub const SEND_ID_LEN: usize = 36;

pub fn new_send_id() -> String {
    format!("snd_{}", uuid::Uuid::new_v4().simple())
}

/// The flat data map every push carries: the App's own keys plus K2's.
pub fn data_map(n: &Notification, s: &Stamps) -> BTreeMap<String, String> {
    let mut d = n.data.clone();
    d.insert("k2App".into(), s.app.clone());
    d.insert("k2Send".into(), s.send_id.clone());
    d.insert("k2Kind".into(), s.kind.to_string());
    d.insert("k2Who".into(), s.who.clone());
    if let Some(t) = &n.topic {
        d.insert("k2Topic".into(), t.clone());
    }
    d
}

fn aps(n: &Notification) -> J {
    let mut a = Map::new();
    if n.silent {
        a.insert("content-available".into(), json!(1));
    } else {
        a.insert("alert".into(), json!({ "title": n.title, "body": n.body }));
        if let Some(s) = &n.sound {
            a.insert("sound".into(), json!(s));
        }
        if n.mutable {
            a.insert("mutable-content".into(), json!(1));
        }
    }
    if let Some(b) = n.badge {
        a.insert("badge".into(), json!(b));
    }
    if let Some(t) = &n.thread_id {
        a.insert("thread-id".into(), json!(t));
    }
    J::Object(a)
}

/// The APNs request body.
pub fn apns_body(n: &Notification, s: &Stamps) -> Vec<u8> {
    let mut top = Map::new();
    top.insert("aps".into(), aps(n));
    for (k, v) in data_map(n, s) {
        top.insert(k, json!(v));
    }
    serde_json::to_vec(&J::Object(top)).unwrap_or_default()
}

/// The APNs request headers (besides `authorization`), as `(name, value)`.
pub fn apns_headers(n: &Notification, bundle_id: &str, now_secs: i64) -> Vec<(String, String)> {
    let mut h = vec![
        ("apns-topic".to_string(), bundle_id.to_string()),
        ("apns-push-type".to_string(), if n.silent { "background" } else { "alert" }.to_string()),
        ("apns-priority".to_string(), if n.high_priority { "10" } else { "5" }.to_string()),
        ("apns-expiration".to_string(), if n.ttl_secs == 0 { "0".to_string() } else { (now_secs + n.ttl_secs as i64).to_string() }),
    ];
    if let Some(c) = &n.collapse_id {
        h.push(("apns-collapse-id".to_string(), c.clone()));
    }
    h
}

fn fcm_core(n: &Notification, s: &Stamps) -> (Option<J>, J) {
    let notification = (!n.silent).then(|| json!({ "title": n.title, "body": n.body }));
    (notification, json!(data_map(n, s)))
}

fn fcm_payload_bytes(n: &Notification, s: &Stamps) -> usize {
    let (notif, data) = fcm_core(n, s);
    serde_json::to_vec(&json!({ "notification": notif, "data": data })).map(|v| v.len()).unwrap_or(usize::MAX)
}

/// The FCM HTTP v1 request body for one token.
pub fn fcm_body(n: &Notification, s: &Stamps, token: &str, now_secs: i64) -> Vec<u8> {
    let (notification, data) = fcm_core(n, s);
    let mut android = Map::new();
    android.insert("priority".into(), json!(if n.high_priority { "HIGH" } else { "NORMAL" }));
    android.insert("ttl".into(), json!(format!("{}s", n.ttl_secs)));
    if let Some(c) = &n.collapse_id {
        android.insert("collapse_key".into(), json!(c));
    }
    if !n.silent {
        let mut an = Map::new();
        if let Some(c) = &n.android_channel {
            an.insert("channel_id".into(), json!(c));
        }
        if let Some(snd) = &n.sound {
            an.insert("sound".into(), json!(snd));
        }
        if let Some(c) = &n.collapse_id {
            an.insert("tag".into(), json!(c));
        }
        if let Some(b) = n.badge {
            an.insert("notification_count".into(), json!(b));
        }
        if !an.is_empty() {
            android.insert("notification".into(), J::Object(an));
        }
    }
    let mut apns_headers = Map::new();
    for (k, v) in apns_headers_for_fcm(n, now_secs) {
        apns_headers.insert(k, json!(v));
    }
    let mut msg = Map::new();
    msg.insert("token".into(), json!(token));
    if let Some(nn) = notification {
        msg.insert("notification".into(), nn);
    }
    msg.insert("data".into(), data);
    msg.insert("android".into(), J::Object(android));
    msg.insert("apns".into(), json!({ "headers": apns_headers, "payload": { "aps": aps(n) } }));
    serde_json::to_vec(&json!({ "message": msg })).unwrap_or_default()
}

fn apns_headers_for_fcm(n: &Notification, now_secs: i64) -> Vec<(String, String)> {
    // FCM sets apns-topic from the iOS app it forwards to.
    apns_headers(n, "", now_secs).into_iter().filter(|(k, _)| k != "apns-topic").collect()
}

#[cfg(test)]
#[path = "payload_tests.rs"]
mod tests;
