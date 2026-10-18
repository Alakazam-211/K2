//! front443 S5: Stalwart's HTTPS listener behind the daemon's :443 front
//! (opt-in, `k2 front enable --mail`; Rosson 2026-10-17: "if it improves
//! deliverability and usability without causing potential security
//! issues").
//!
//! On a `tls-alpn` box Stalwart binds `[::]:443` and gets its certificate
//! by ACME TLS-ALPN-01. Behind the front its `https` listener moves to
//! `127.0.0.1:8443` with `overrideProxyTrustedNetworks = {127.0.0.1, ::1}`;
//! the front passes the mail names through untouched after a PROXY v2
//! header, so TLS-ALPN-01 keeps working (the validator's ClientHello
//! reaches Stalwart byte for byte) and Stalwart sees the real client IP.
//!
//! Checked against the pinned Stalwart 0.16.25 source (`3f657330`,
//! `crates/common/src/network/listen.rs`, `config/server/listener.rs`,
//! `network/security.rs`):
//! - `overrideProxyTrustedNetworks` is per listener. Only connections FROM
//!   those networks are parsed for a PROXY header (`proxy-header` 0.1.3,
//!   v1 and v2); everything else is served as is. A trusted connection
//!   without a valid header is dropped before any session exists
//!   (`ProxyError`), so it can never authenticate, fail, or be banned.
//! - The session's remote IP is the PROXY source (v4-mapped unmapped), and
//!   the blocked-IP check and the auth fail2ban use that IP. With the
//!   front always writing the real client, bans land on real clients.
//! - The listener override (unlike the global `proxyTrustedNetworks`)
//!   does NOT add loopback to the allowed (never-banned) list. So the
//!   front refuses clients whose own address is loopback on this route:
//!   no failed login that came THROUGH THE FRONT is attributed to
//!   127.0.0.1, whose ban would also lock K2 out of Stalwart's management
//!   listener (127.0.0.1:8180).
//! - What this does not stop: a process ON the box can connect to
//!   127.0.0.1:8443 itself and send a PROXY header naming any source
//!   (loopback trust has no uid check). That is no new door for loopback
//!   lockout — the same process can already fail logins on IMAP/SMTP from
//!   127.0.0.1 today — but it can make Stalwart ban an outside address it
//!   names. Narrowing it needs an nft `meta skuid` rule or a unix socket
//!   (later work).
//! - Only the `https` listener changes; SMTP/IMAP/submission keep their
//!   public binds and see real IPs as before.
//!
//! Nothing else on the box talks to 127.0.0.1:8443 while Stalwart is
//! behind the front: Caddy's mail site is not rendered then
//! (`skin_door::apply`, `front443::mail_behind_front`).

use serde_json::{json, Value};

use super::imap_listeners::ListenerRow;

/// Stalwart's HTTPS listener behind the front.
pub const FRONT_BIND: &str = "127.0.0.1:8443";
/// Where it binds on a `tls-alpn` box without the front.
pub const PUBLIC_BIND: &str = "[::]:443";
/// The front connects from loopback only.
pub const TRUSTED: [&str; 2] = ["127.0.0.1", "::1"];

fn trusted_map() -> Value {
    let mut m = serde_json::Map::new();
    for t in TRUSTED {
        m.insert(t.to_string(), Value::Bool(true));
    }
    Value::Object(m)
}

/// The `https` listener body `listeners_apply` sets for a port plan:
/// behind the front on a `tls-alpn` box, the loopback bind plus the
/// trusted PROXY networks; otherwise the plan's bind alone (unchanged).
pub fn https_listener_body(port_plan: &str, behind_front: bool) -> Value {
    if behind_front && port_plan == "tls-alpn" {
        json!({ "bind": { FRONT_BIND: true }, "overrideProxyTrustedNetworks": trusted_map() })
    } else {
        json!({ "bind": { super::preflight::https_listener_bind(port_plan): true } })
    }
}

/// The one `x:NetworkListener/set` that moves the `https` listener behind
/// the front (`on`) or back to `[::]:443` with no PROXY trust (`off`).
pub fn https_update_args(id: &str, on: bool) -> Value {
    let body = if on {
        https_listener_body("tls-alpn", true)
    } else {
        json!({ "bind": { PUBLIC_BIND: true }, "overrideProxyTrustedNetworks": {} })
    };
    let mut update = serde_json::Map::new();
    update.insert(id.to_string(), body);
    json!({ "update": update })
}

/// The `https` listener among Stalwart's stored listeners.
pub fn https_row(rows: &[ListenerRow]) -> Option<&ListenerRow> {
    rows.iter().find(|r| r.name == "https")
}

/// May hosted mail move behind the front? Only a box whose Stalwart is set
/// up and on the `tls-alpn` plan (its own ACME on :443): http-01 / dns-01
/// boxes already sit behind Caddy on 127.0.0.1:8443.
pub fn check_can_move() -> Result<(), String> {
    if !super::supervisor::mail_supported() {
        return Err("hosted mail runs on Linux boxes only".into());
    }
    let status = super::supervisor::current_status();
    if !matches!(status.as_deref(), Some("running" | "degraded" | "stopped")) {
        return Err(format!(
            "hosted mail is {} — enable it first (a fresh enable on a front box goes behind the front by itself)",
            status.as_deref().unwrap_or("not installed")
        ));
    }
    let plan = super::supervisor::row_field("port_plan");
    if plan.as_deref() != Some("tls-alpn") {
        return Err(format!(
            "hosted mail is on the {} plan: Stalwart already sits on 127.0.0.1:8443 behind Caddy, which the front \
             reaches as Skin Direct's mail site — nothing to move",
            plan.as_deref().unwrap_or("unset")
        ));
    }
    Ok(())
}

/// Move Stalwart's `https` listener (one registry update + one Stalwart
/// restart, through the mail helper's or plain sudo's door). The restart
/// is required: the front's socket can only take :443 once Stalwart let
/// go of it (and the reverse on the way back).
pub fn move_behind_front(on: bool) -> Result<Value, String> {
    use super::imap_listeners::{restart_once, LiveDoors, RestartReport};
    let client = super::supervisor::mgmt_client_from_row()?;
    let rows = client.listener_rows()?;
    let row = https_row(&rows).ok_or("Stalwart has no `https` listener")?;
    let want = if on { FRONT_BIND } else { PUBLIC_BIND };
    let already = row.binds.len() == 1 && row.binds[0] == want;
    if !already || on {
        // `on` always re-asserts the trusted networks with the bind.
        client.listener_update(https_update_args(&row.id, on))?;
    }
    match restart_once(&mut LiveDoors { why: "front443" })? {
        RestartReport::Restarted(_) => {}
        RestartReport::NotRestarted { helper } => {
            return Err(format!(
                "the https listener is stored at {want} but Stalwart could not be restarted (mail helper {}): \
                 run as root `systemctl restart stalwart`, then try again",
                helper.as_str()
            ))
        }
    }
    Ok(json!({ "https": want, "proxyV2": on, "restarted": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn behind_the_front_https_is_loopback_with_proxy_trust_only_on_tls_alpn() {
        let b = https_listener_body("tls-alpn", true);
        assert_eq!(b["bind"], json!({"127.0.0.1:8443": true}));
        assert_eq!(b["overrideProxyTrustedNetworks"], json!({"127.0.0.1": true, "::1": true}));
        // Without the front, or on http-01 / dns-01: exactly what it was.
        assert_eq!(https_listener_body("tls-alpn", false), json!({"bind": {"[::]:443": true}}));
        for plan in ["http-01", "dns-01"] {
            for front in [false, true] {
                let b = https_listener_body(plan, front);
                assert_eq!(b, json!({"bind": {"127.0.0.1:8443": true}}), "{plan} {front}");
                assert!(b.get("overrideProxyTrustedNetworks").is_none(), "Caddy's 8443 hop sends no PROXY header");
            }
        }
    }

    #[test]
    fn the_move_is_one_update_each_way_and_back_clears_the_trust() {
        let on = https_update_args("L-https", true);
        assert_eq!(on["update"]["L-https"]["bind"], json!({"127.0.0.1:8443": true}));
        assert_eq!(on["update"]["L-https"]["overrideProxyTrustedNetworks"]["127.0.0.1"], true);
        assert!(on.get("create").is_none() && on.get("destroy").is_none(), "only the https listener changes");
        let off = https_update_args("L-https", false);
        assert_eq!(off["update"]["L-https"]["bind"], json!({"[::]:443": true}));
        assert_eq!(off["update"]["L-https"]["overrideProxyTrustedNetworks"], json!({}), "no PROXY trust left on a public bind");
        // Never a trusted network outside loopback.
        for t in TRUSTED {
            assert!(t.parse::<std::net::IpAddr>().unwrap().is_loopback(), "{t}");
        }
    }

    #[test]
    fn the_https_row_is_found_by_name() {
        let row = |id: &str, name: &str| ListenerRow {
            id: id.into(),
            name: name.into(),
            binds: vec![],
            protocol: Some("http".into()),
            use_tls: Some(true),
            tls_implicit: Some(true),
        };
        let rows = vec![row("a", "http"), row("b", "https"), row("c", "imaps")];
        assert_eq!(https_row(&rows).map(|r| r.id.as_str()), Some("b"));
        assert!(https_row(&rows[..1]).is_none());
    }
}
