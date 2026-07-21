//! frpc TOML renderer.
//!
//! Emits a `frpc.toml` for `fatedier/frp` **v0.61** (TOML config schema)
//! that dials the K2 Connect frps server and requests an HTTP proxy on a
//! `<subdomain>.k2.dev` vhost. The server-plugin contract (see the
//! proprietary `k2-connect` control plane) requires:
//!
//!   * the K2SO bearer token in the client login metas — frp v0.61
//!     spells this `metadatas.<key>` at the *client* (top) level, which
//!     frps forwards to the plugin as `login.metas`. The plugin reads
//!     key `"token"` (`frp_plugin.rs::TOKEN_META_KEY`).
//!   * an `http` proxy whose `subdomain` is the requested label. The
//!     plugin *forces* it to the user's canonical namespace on NewProxy,
//!     so we just send the requested value.
//!   * `localPort` = the daemon port to expose.
//!
//! We render by hand (not via a TOML serializer) so the output is exact,
//! diff-stable, and reviewable against frp's documented schema. All
//! string values are emitted with basic TOML escaping.

use super::config::{RelayEndpoint, TunnelConfig};

/// Render the frpc TOML for `cfg`. `local_port` is resolved by the
/// caller (the connector fills in the live daemon port when the config
/// leaves it `None`); pass the concrete port here.
///
/// `e2e` selects the proxy `type` (PRD `k2-connect-e2e-encryption.md` §4
/// Option A):
///   * `false` (today's behaviour) — `type = "http"`; the relay's Caddy
///     terminates TLS and forwards cleartext to `localPort` (the daemon's
///     HTTP port).
///   * `true` — `type = "https"`; frps does SNI passthrough on
///     `vhostHTTPSPort` (443) and forwards the **encrypted** stream to
///     `localPort` (the daemon's rustls HTTPS listener port), which
///     terminates TLS itself. The relay never sees plaintext.
///
/// The `proxy_name` is the frp proxy identifier — deterministic per
/// subdomain so a restart reuses the same name. Defaults to
/// `k2so-<subdomain>` (or `k2so-daemon` when no subdomain is set).
pub fn render_frpc_toml(cfg: &TunnelConfig, local_port: u16, e2e: bool) -> String {
    let sub = cfg.subdomain.trim();
    let proxy_name = if sub.is_empty() {
        "k2so-daemon".to_string()
    } else {
        format!("k2so-{sub}")
    };

    let mut out = String::new();
    out.push_str("# K2SO tunnel connector — auto-generated frpc config.\n");
    out.push_str("# Exposes the local K2SO daemon at https://<subdomain>.k2.dev\n");
    out.push_str("# via the hosted K2 Connect frps backbone. DO NOT EDIT BY HAND —\n");
    out.push_str("# regenerated on every `k2so tunnel start`. Contains a bearer\n");
    out.push_str("# token; treat as a secret (file is written 0600).\n\n");

    // ── Client (common) section ──────────────────────────────────────
    out.push_str(&format!("serverAddr = {}\n", toml_str(&cfg.server_addr)));
    out.push_str(&format!("serverPort = {}\n", cfg.server_port));
    out.push('\n');

    // Login metas — frp v0.61: top-level `metadatas.<key>`. frps forwards
    // these as `login.metas` to the server-plugin, which validates
    // `metas["token"]` and (when present) `metas["device_id"]` for lease
    // identity / multi-device accounting.
    out.push_str("# Login metadata forwarded to the K2 Connect control plane.\n");
    out.push_str("# The bearer token authorizes the tunnel and selects the user.\n");
    out.push_str("[metadatas]\n");
    out.push_str(&format!("token = {}\n", toml_str(&cfg.token)));
    if let Some(dev) = cfg.device_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("device_id = {}\n", toml_str(dev)));
    }
    out.push('\n');

    // ── Proxy section (array of tables) ───────────────────────────────
    // E2E flips the proxy type: `https` makes frps route by SNI on the
    // vhostHTTPSPort and forward ciphertext to the daemon's rustls
    // listener (TLS terminates on the daemon); `http` is the legacy path
    // where the relay's Caddy terminates TLS. Same subdomain either way.
    out.push_str("[[proxies]]\n");
    out.push_str(&format!("name = {}\n", toml_str(&proxy_name)));
    if e2e {
        out.push_str("type = \"https\"\n");
    } else {
        out.push_str("type = \"http\"\n");
    }
    out.push_str("localIP = \"127.0.0.1\"\n");
    out.push_str(&format!("localPort = {local_port}\n"));
    if !sub.is_empty() {
        out.push_str(&format!("subdomain = {}\n", toml_str(sub)));
    }

    out
}

/// Render the frpc TOML for `cfg` dialing a SPECIFIC relay of the
/// fallback list (multi-relay failover). The relays are peers behind one
/// frps auth token and one wildcard cert, so **only `serverAddr` /
/// `serverPort` differ** between them — token, subdomain, proxy name,
/// type, and localPort are byte-identical to [`render_frpc_toml`] for the
/// same `cfg`. For a legacy single-endpoint config,
/// `relay_list()[0]` IS the stored `server_addr`/`server_port`, so this
/// renders byte-identical output to the pre-failover path.
pub fn render_frpc_toml_for_relay(
    cfg: &TunnelConfig,
    relay: &RelayEndpoint,
    local_port: u16,
    e2e: bool,
) -> String {
    let mut c = cfg.clone();
    c.server_addr = relay.host.clone();
    c.server_port = relay.port;
    render_frpc_toml(&c, local_port, e2e)
}

/// Minimal TOML basic-string escaping. frp config values (host, token,
/// subdomain) are ASCII in practice, but we escape backslash, double
/// quote, and control chars so a pathological token can't break the
/// file or inject a second key.
fn toml_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> TunnelConfig {
        TunnelConfig {
            server_addr: "178.156.232.105".to_string(),
            server_port: 7000,
            token: "tok_secret".to_string(),
            subdomain: "rosson".to_string(),
            local_port: Some(57839),
            ..Default::default()
        }
    }

    #[test]
    fn renders_server_addr_and_port() {
        let toml = render_frpc_toml(&sample(), 57839, false);
        assert!(
            toml.contains("serverAddr = \"178.156.232.105\""),
            "missing serverAddr\n{toml}"
        );
        assert!(toml.contains("serverPort = 7000"), "missing serverPort\n{toml}");
    }

    #[test]
    fn renders_token_in_login_metadatas() {
        let toml = render_frpc_toml(&sample(), 57839, false);
        assert!(
            toml.contains("[metadatas]"),
            "token must live in the client-level [metadatas] table\n{toml}"
        );
        assert!(
            toml.contains("token = \"tok_secret\""),
            "token meta missing or wrong key\n{toml}"
        );
    }

    #[test]
    fn renders_device_id_in_login_metadatas() {
        let mut cfg = sample();
        cfg.device_id = Some("dev-abc-123".to_string());
        let toml = render_frpc_toml(&cfg, 57839, false);
        assert!(
            toml.contains("[metadatas]"),
            "device_id must live in the client-level [metadatas] table\n{toml}"
        );
        assert!(
            toml.contains("device_id = \"dev-abc-123\""),
            "device_id meta missing or wrong value\n{toml}"
        );
        // Escaping: quotes in device_id must not break the TOML string.
        cfg.device_id = Some("dev\"evil".to_string());
        let escaped = render_frpc_toml(&cfg, 57839, false);
        assert!(
            escaped.contains(r#"device_id = "dev\"evil""#),
            "device_id must be TOML-escaped\n{escaped}"
        );
        // Relay path clones cfg — device_id must survive rotation renders.
        let relay = RelayEndpoint {
            host: "5.6.7.8".to_string(),
            port: 7001,
        };
        cfg.device_id = Some("dev-abc-123".to_string());
        let rotated = render_frpc_toml_for_relay(&cfg, &relay, 57839, false);
        assert!(
            rotated.contains("device_id = \"dev-abc-123\""),
            "relay render must inherit device_id via cfg clone\n{rotated}"
        );
    }

    #[test]
    fn absent_or_empty_device_id_omits_key() {
        // Default sample has no device_id.
        let plain = render_frpc_toml(&sample(), 57839, false);
        assert!(
            !plain.contains("device_id"),
            "absent device_id must omit the key entirely\n{plain}"
        );
        let mut cfg = sample();
        cfg.device_id = Some(String::new());
        let empty = render_frpc_toml(&cfg, 57839, false);
        assert!(
            !empty.contains("device_id"),
            "empty device_id must omit the key\n{empty}"
        );
        cfg.device_id = Some("   ".to_string());
        let blank = render_frpc_toml(&cfg, 57839, false);
        assert!(
            !blank.contains("device_id"),
            "whitespace-only device_id must omit the key\n{blank}"
        );
    }

    #[test]
    fn renders_http_proxy_with_subdomain_and_local_port() {
        let toml = render_frpc_toml(&sample(), 57839, false);
        assert!(toml.contains("[[proxies]]"), "no proxy table\n{toml}");
        assert!(toml.contains("type = \"http\""), "proxy must be http\n{toml}");
        assert!(
            toml.contains("subdomain = \"rosson\""),
            "requested subdomain missing\n{toml}"
        );
        assert!(
            toml.contains("localPort = 57839"),
            "localPort must be the exposed daemon port\n{toml}"
        );
        assert!(
            toml.contains("name = \"k2so-rosson\""),
            "proxy name must be deterministic per subdomain\n{toml}"
        );
    }

    #[test]
    fn local_port_argument_overrides_config_field() {
        // The connector resolves the live daemon port and passes it
        // explicitly; the renderer must honor the argument, not the
        // (possibly stale/None) config field.
        let mut cfg = sample();
        cfg.local_port = None;
        let toml = render_frpc_toml(&cfg, 8123, false);
        assert!(toml.contains("localPort = 8123"), "{toml}");
    }

    #[test]
    fn empty_subdomain_omits_subdomain_key_and_names_daemon() {
        let mut cfg = sample();
        cfg.subdomain = String::new();
        let toml = render_frpc_toml(&cfg, 57839, false);
        assert!(
            !toml.contains("subdomain ="),
            "blank subdomain must not emit a subdomain key (server derives it)\n{toml}"
        );
        assert!(toml.contains("name = \"k2so-daemon\""), "{toml}");
    }

    #[test]
    fn e2e_flag_flips_proxy_type_http_to_https() {
        // OFF (legacy / zero-regression): http, never https.
        let off = render_frpc_toml(&sample(), 57839, false);
        assert!(
            off.contains("type = \"http\""),
            "e2e=false must render an http proxy\n{off}"
        );
        assert!(
            !off.contains("type = \"https\""),
            "e2e=false must NOT render https\n{off}"
        );

        // ON (Option A): https, never the plain http type.
        let on = render_frpc_toml(&sample(), 57839, true);
        assert!(
            on.contains("type = \"https\""),
            "e2e=true must render an https proxy\n{on}"
        );
        assert!(
            !on.contains("type = \"http\"\n"),
            "e2e=true must NOT render the plain http type\n{on}"
        );
        // Everything else is unchanged on the flip: same subdomain, same
        // localPort, same proxy name — only the type differs.
        assert!(on.contains("subdomain = \"rosson\""), "{on}");
        assert!(on.contains("localPort = 57839"), "{on}");
        assert!(on.contains("name = \"k2so-rosson\""), "{on}");
    }

    #[test]
    fn relay_render_swaps_only_server_addr_and_port() {
        // Failover invariant: rotating relays changes serverAddr/serverPort
        // and NOTHING else — same token, same subdomain, same proxy name.
        let cfg = sample();
        let relay = RelayEndpoint { host: "5.6.7.8".to_string(), port: 7001 };
        let rotated = render_frpc_toml_for_relay(&cfg, &relay, 57839, false);
        assert!(
            rotated.contains("serverAddr = \"5.6.7.8\""),
            "must dial the given relay\n{rotated}"
        );
        assert!(rotated.contains("serverPort = 7001"), "{rotated}");

        // Every non-server line is byte-identical to the plain render.
        let baseline = render_frpc_toml(&cfg, 57839, false);
        let strip = |s: &str| {
            s.lines()
                .filter(|l| !l.starts_with("serverAddr") && !l.starts_with("serverPort"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        assert_eq!(
            strip(&rotated),
            strip(&baseline),
            "only serverAddr/serverPort may differ between relays"
        );
    }

    #[test]
    fn relay_render_of_legacy_primary_is_byte_identical_to_plain_render() {
        // Single-relay (legacy) configs go through the relay-aware path in
        // the connector now — the output must be EXACTLY what the old path
        // produced.
        let cfg = sample();
        let primary = cfg.relay_list()[0].clone();
        assert_eq!(
            render_frpc_toml_for_relay(&cfg, &primary, 57839, false),
            render_frpc_toml(&cfg, 57839, false),
        );
        assert_eq!(
            render_frpc_toml_for_relay(&cfg, &primary, 57839, true),
            render_frpc_toml(&cfg, 57839, true),
        );
    }

    #[test]
    fn token_with_quotes_is_escaped_not_injected() {
        let mut cfg = sample();
        cfg.token = "ab\"c\nname = \"evil".to_string();
        let toml = render_frpc_toml(&cfg, 1, false);
        // The malicious newline + key must be escaped inside the string,
        // never emitted as a real second key.
        assert!(
            toml.contains(r#"token = "ab\"c\nname = \"evil""#),
            "token escaping failed — TOML injection possible\n{toml}"
        );
    }
}
