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

use super::config::TunnelConfig;

/// Render the frpc TOML for `cfg`. `local_port` is resolved by the
/// caller (the connector fills in the live daemon port when the config
/// leaves it `None`); pass the concrete port here.
///
/// The `proxy_name` is the frp proxy identifier — deterministic per
/// subdomain so a restart reuses the same name. Defaults to
/// `k2so-<subdomain>` (or `k2so-daemon` when no subdomain is set).
pub fn render_frpc_toml(cfg: &TunnelConfig, local_port: u16) -> String {
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
    // `metas["token"]`.
    out.push_str("# Login metadata forwarded to the K2 Connect control plane.\n");
    out.push_str("# The bearer token authorizes the tunnel and selects the user.\n");
    out.push_str("[metadatas]\n");
    out.push_str(&format!("token = {}\n", toml_str(&cfg.token)));
    out.push('\n');

    // ── Proxy section (array of tables) ───────────────────────────────
    out.push_str("[[proxies]]\n");
    out.push_str(&format!("name = {}\n", toml_str(&proxy_name)));
    out.push_str("type = \"http\"\n");
    out.push_str("localIP = \"127.0.0.1\"\n");
    out.push_str(&format!("localPort = {local_port}\n"));
    if !sub.is_empty() {
        out.push_str(&format!("subdomain = {}\n", toml_str(sub)));
    }

    out
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
        let toml = render_frpc_toml(&sample(), 57839);
        assert!(
            toml.contains("serverAddr = \"178.156.232.105\""),
            "missing serverAddr\n{toml}"
        );
        assert!(toml.contains("serverPort = 7000"), "missing serverPort\n{toml}");
    }

    #[test]
    fn renders_token_in_login_metadatas() {
        let toml = render_frpc_toml(&sample(), 57839);
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
    fn renders_http_proxy_with_subdomain_and_local_port() {
        let toml = render_frpc_toml(&sample(), 57839);
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
        let toml = render_frpc_toml(&cfg, 8123);
        assert!(toml.contains("localPort = 8123"), "{toml}");
    }

    #[test]
    fn empty_subdomain_omits_subdomain_key_and_names_daemon() {
        let mut cfg = sample();
        cfg.subdomain = String::new();
        let toml = render_frpc_toml(&cfg, 57839);
        assert!(
            !toml.contains("subdomain ="),
            "blank subdomain must not emit a subdomain key (server derives it)\n{toml}"
        );
        assert!(toml.contains("name = \"k2so-daemon\""), "{toml}");
    }

    #[test]
    fn token_with_quotes_is_escaped_not_injected() {
        let mut cfg = sample();
        cfg.token = "ab\"c\nname = \"evil".to_string();
        let toml = render_frpc_toml(&cfg, 1);
        // The malicious newline + key must be escaped inside the string,
        // never emitted as a real second key.
        assert!(
            toml.contains(r#"token = "ab\"c\nname = \"evil""#),
            "token escaping failed — TOML injection possible\n{toml}"
        );
    }
}
