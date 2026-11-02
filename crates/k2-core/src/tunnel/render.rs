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
/// The proxy is named deterministically per subdomain, so a restart reuses
/// the same name: `k2so-<subdomain>`, or `k2so-daemon` when none is set.
/// `K2_TUNNEL_PROXY_NAME=confirmed` (opt-in, 0.46.8 relay name binding) names
/// it `k2so-<label>` from the SERVER-CONFIRMED label (`cfg.confirmed_label`)
/// instead; then with no confirmed label NO proxy is rendered and the
/// connector logs why. `subdomain` goes out as the requested label either way.
pub fn render_frpc_toml(cfg: &TunnelConfig, local_port: u16, e2e: bool) -> String {
    render_frpc_toml_with(cfg, local_port, e2e, &custom_for_render())
}

/// The Path B names a render carries. k2-core's own unit tests render none
/// (they run in parallel with the list's own test, which changes it); the
/// list is exercised through [`render_frpc_toml_with`].
fn custom_for_render() -> Vec<String> {
    if cfg!(test) {
        return Vec::new();
    }
    super::custom_names::current()
}

/// [`render_frpc_toml`] with an explicit Path B name list (0.46.8, PB-2):
/// with E2E on, a subdomain and at least one name, a second `https` proxy
/// `k2-custom-<label>` carries `customDomains = [the names]` to the same
/// `localPort` (the E2E listener, which serves those names), PROXY v2 on
/// like the main proxy. frps registers only the names the control plane
/// verified for this label. No names (or E2E off) renders no second proxy,
/// byte-identical to before 0.46.8.
pub fn render_frpc_toml_with(cfg: &TunnelConfig, local_port: u16, e2e: bool, custom: &[String]) -> String {
    let sub = cfg.subdomain.trim();
    let proxy_name = main_proxy_name(cfg);

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
    let Some(proxy_name) = proxy_name else {
        out.push_str("# No proxy: the control plane has confirmed no label for this token yet.\n");
        return out;
    };
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
    if e2e {
        // 0.46.3 client IP: frpc writes one PROXY v2 header (the address
        // frps saw) before each connection's TLS bytes. Only the daemon's
        // E2E listener (this `localPort`) reads it; see k2-daemon
        // `client_ip.rs`. An old relay reports its own loopback, which the
        // daemon treats as no client IP. The cleartext `http` path has no
        // TLS to put it in front of, so it stays off there.
        out.push_str("transport.proxyProtocolVersion = \"v2\"\n");
    }

    // The relay registers these names only on `k2-custom-<label>` from that
    // label: the confirmed one, as for the main proxy.
    let custom_label = proxy_label(cfg).filter(|_| e2e && !custom.is_empty());
    if let Some(label) = custom_label {
        out.push('\n');
        out.push_str("# Path B (0.46.8): this box's own domains through K2's relays. The relay\n");
        out.push_str("# passes each TLS stream through by name; the daemon ends TLS with the\n");
        out.push_str("# name's own certificate.\n");
        out.push_str("[[proxies]]\n");
        out.push_str(&format!("name = {}\n", toml_str(&format!("{}{label}", super::custom_names::PROXY_PREFIX))));
        out.push_str("type = \"https\"\n");
        out.push_str("localIP = \"127.0.0.1\"\n");
        out.push_str(&format!("localPort = {local_port}\n"));
        let list: Vec<String> = custom.iter().map(|n| toml_str(n)).collect();
        out.push_str(&format!("customDomains = [{}]\n", list.join(", ")));
        out.push_str("transport.proxyProtocolVersion = \"v2\"\n");
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
    render_frpc_toml_for_relay_with(cfg, relay, local_port, e2e, &custom_for_render())
}

/// [`render_frpc_toml_for_relay`] with an explicit Path B name list: the
/// custom proxy is in the shared body, so every relay's render carries it
/// (vs-live PB-A4).
pub fn render_frpc_toml_for_relay_with(
    cfg: &TunnelConfig,
    relay: &RelayEndpoint,
    local_port: u16,
    e2e: bool,
    custom: &[String],
) -> String {
    let mut c = cfg.clone();
    c.server_addr = relay.host.clone();
    c.server_port = relay.port;
    render_frpc_toml_with(&c, local_port, e2e, custom)
}

/// The label that names the main tunnel's proxies: the requested `subdomain`
/// (the default), or under `K2_TUNNEL_PROXY_NAME=confirmed` the
/// server-confirmed one. `None` = no subdomain to name a custom proxy from
/// (and, opt-in, no main proxy either). The voice lane doesn't use this:
/// [`confirmed_label_of`].
pub fn proxy_label(cfg: &TunnelConfig) -> Option<String> {
    proxy_label_in(cfg, super::confirmed_label::config_naming())
}

fn proxy_label_in(cfg: &TunnelConfig, config_naming: bool) -> Option<String> {
    if config_naming {
        let sub = cfg.subdomain.trim();
        return (!sub.is_empty()).then(|| sub.to_string());
    }
    confirmed_label_of(cfg)
}

/// `cfg.confirmed_label`, when it is a valid k2.dev label.
pub fn confirmed_label_of(cfg: &TunnelConfig) -> Option<String> {
    cfg.confirmed_label.as_deref().map(str::trim).filter(|l| super::confirmed_label::label_ok(l)).map(str::to_string)
}

/// The main frpc's proxy name: `k2so-<subdomain>` or `k2so-daemon` (the
/// default). `K2_TUNNEL_PROXY_NAME=confirmed`: `k2so-<confirmed label>`, and
/// `None` = no proxy.
pub fn main_proxy_name(cfg: &TunnelConfig) -> Option<String> {
    main_proxy_name_in(cfg, super::confirmed_label::config_naming())
}

fn main_proxy_name_in(cfg: &TunnelConfig, config_naming: bool) -> Option<String> {
    match proxy_label_in(cfg, config_naming) {
        Some(l) => Some(format!("k2so-{l}")),
        None if config_naming => Some("k2so-daemon".to_string()),
        None => None,
    }
}

/// The voice lane's frp proxy name for `label` (§6.6 M4, exact).
pub fn voice_proxy_name(label: &str) -> String {
    format!("k2voice-{label}")
}

/// The TURN host the voice lane carries for `label` (§6.6 M1/M4): a nested
/// name under the box's label, so the relay serves it for Pro labels only.
pub fn voice_turn_host(label: &str) -> String {
    format!("turn.{label}.k2.dev")
}

/// The local TURN-TLS listener frpc-voice dials: sew-media's `passthrough`
/// listener on loopback (§6.6 M4/M5).
pub const VOICE_LOCAL_PORT: u16 = 5349;

/// Render `frpc-voice.toml` (voice rooms PRD §6.6 M4, S9b): a second frpc
/// that dials the SAME relay as the main frpc (`relay`, the selector's
/// current pick) with the SAME token and device id, logs in with
/// `metadatas.role = "voice"` (the relay never takes its login or close as
/// the box's liveness) and carries exactly one `https` proxy, named
/// `k2voice-<label>`, for `turn.<label>.k2.dev` → `127.0.0.1:5349` with a
/// PROXY v2 header. No `transport.tcpMux` line: frp's default (on) is what
/// frps requires; isolation comes from the separate process and TCP
/// connection. `<label>` is the server-confirmed label
/// ([`confirmed_label_of`]) however the main tunnel is named, never the
/// requested `subdomain`. `None` without one (no proxy, so no lane).
pub fn render_voice_frpc_toml(cfg: &TunnelConfig, relay: &RelayEndpoint) -> Option<String> {
    let label = confirmed_label_of(cfg)?;
    let label = label.as_str();
    let mut out = String::new();
    out.push_str("# K2 voice lane (frpc-voice): auto-generated frpc config.\n");
    out.push_str("# Carries only turn.<label>.k2.dev to sew-media's TURN-TLS on\n");
    out.push_str("# 127.0.0.1:5349. DO NOT EDIT BY HAND: regenerated by the daemon.\n");
    out.push_str("# Contains a bearer token; treat as a secret (file is written 0600).\n\n");
    out.push_str(&format!("serverAddr = {}\n", toml_str(&relay.host)));
    out.push_str(&format!("serverPort = {}\n", relay.port));
    out.push('\n');
    out.push_str("[metadatas]\n");
    out.push_str(&format!("token = {}\n", toml_str(&cfg.token)));
    if let Some(dev) = cfg.device_id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        out.push_str(&format!("device_id = {}\n", toml_str(dev)));
    }
    out.push_str("role = \"voice\"\n");
    out.push('\n');
    out.push_str("[[proxies]]\n");
    out.push_str(&format!("name = {}\n", toml_str(&voice_proxy_name(label))));
    out.push_str("type = \"https\"\n");
    out.push_str(&format!("customDomains = [{}]\n", toml_str(&voice_turn_host(label))));
    out.push_str("localIP = \"127.0.0.1\"\n");
    out.push_str(&format!("localPort = {VOICE_LOCAL_PORT}\n"));
    out.push_str("transport.proxyProtocolVersion = \"v2\"\n");
    Some(out)
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
            confirmed_label: Some("rosson".to_string()),
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
    fn empty_subdomain_omits_subdomain_key_and_keeps_the_confirmed_name() {
        let _naming = crate::test_env::EnvVar::set(super::super::confirmed_label::PROXY_NAME_ENV, "confirmed");
        let mut cfg = sample();
        cfg.subdomain = String::new();
        let toml = render_frpc_toml(&cfg, 57839, false);
        assert!(
            !toml.contains("subdomain ="),
            "blank subdomain must not emit a subdomain key (server derives it)\n{toml}"
        );
        assert!(toml.contains("name = \"k2so-rosson\""), "{toml}");
        assert!(!toml.contains("k2so-daemon"), "{toml}");
    }

    /// The default (0.46.8): the main tunnel's names stay what they are today,
    /// whatever the control plane confirms, and only the voice lane uses the
    /// confirmed label. A box with no confirmed label still serves.
    #[test]
    fn by_default_the_main_tunnel_keeps_todays_names_and_only_voice_is_confirmed() {
        let relay = RelayEndpoint { host: "relay.k2.dev".into(), port: 7000 };
        for setting in [None, Some("config"), Some("typo")] {
            let _naming = match setting {
                Some(v) => crate::test_env::EnvVar::set(super::super::confirmed_label::PROXY_NAME_ENV, v),
                None => crate::test_env::EnvVar::remove(super::super::confirmed_label::PROXY_NAME_ENV),
            };
            for confirmed in [Some("rosson"), None, Some("Not.A.Label")] {
                let mut cfg = sample();
                cfg.confirmed_label = confirmed.map(str::to_string);
                cfg.subdomain = "old-box".into();
                let names = ["a.example.com".to_string()];
                let main = render_frpc_toml_with(&cfg, 57839, true, &names);
                assert!(main.contains("name = \"k2so-old-box\""), "{setting:?} {confirmed:?}\n{main}");
                assert!(main.contains("name = \"k2-custom-old-box\""), "{main}");
                assert!(main.contains("subdomain = \"old-box\""), "{main}");
                assert_eq!(main_proxy_name(&cfg).as_deref(), Some("k2so-old-box"));
                let voice = render_voice_frpc_toml(&cfg, &relay);
                match confirmed {
                    Some("rosson") => {
                        let v = voice.expect("a confirmed label");
                        assert!(v.contains("name = \"k2voice-rosson\"") && v.contains("turn.rosson.k2.dev"), "{v}");
                    }
                    _ => assert_eq!(voice, None, "the requested name never reaches the voice lane"),
                }
                // No subdomain: `k2so-daemon`, as ever, and no custom proxy.
                cfg.subdomain = String::new();
                let toml = render_frpc_toml_with(&cfg, 57839, true, &names);
                assert!(toml.contains("[[proxies]]\nname = \"k2so-daemon\""), "{toml}");
                assert!(!toml.contains("k2-custom-"), "{toml}");
                assert!(!toml.contains("subdomain ="), "{toml}");
            }
        }
    }

    /// Relay name binding (opt-in, `K2_TUNNEL_PROXY_NAME=confirmed`): the proxy
    /// names come from the label the control plane confirmed, whatever the
    /// requested `subdomain` says.
    #[test]
    fn stale_or_odd_subdomains_still_name_the_confirmed_label() {
        let _naming = crate::test_env::EnvVar::set(super::super::confirmed_label::PROXY_NAME_ENV, "confirmed");
        let relay = RelayEndpoint { host: "relay.k2.dev".into(), port: 7000 };
        for requested in ["old-box", "rosson.k2.dev", "staging.rosson", "Rosson", "", "  rosson  "] {
            let mut cfg = sample();
            cfg.subdomain = requested.to_string();
            let main = render_frpc_toml(&cfg, 57839, true);
            assert!(main.contains("name = \"k2so-rosson\""), "{requested:?}\n{main}");
            assert_eq!(main.matches("name = ").count(), 1, "{main}");
            let names = ["a.example.com".to_string()];
            let with_custom = render_frpc_toml_with(&cfg, 57839, true, &names);
            assert!(with_custom.contains("name = \"k2-custom-rosson\""), "{requested:?}\n{with_custom}");
            let voice = render_voice_frpc_toml(&cfg, &relay).expect("a confirmed label");
            assert!(voice.contains("name = \"k2voice-rosson\""), "{requested:?}\n{voice}");
            assert!(voice.contains("customDomains = [\"turn.rosson.k2.dev\"]"), "{voice}");
            assert_eq!(voice, render_voice_frpc_toml(&sample(), &relay).unwrap(), "the request never reaches the voice lane: {requested:?}");
        }
    }

    /// Opt-in, no confirmed label: no tunnel proxy at all (never
    /// `k2so-daemon`), and no voice lane. An invalid confirmation counts as
    /// none.
    #[test]
    fn no_confirmed_label_renders_no_proxy() {
        let _naming = crate::test_env::EnvVar::set(super::super::confirmed_label::PROXY_NAME_ENV, "confirmed");
        let relay = RelayEndpoint { host: "relay.k2.dev".into(), port: 7000 };
        for confirmed in [None, Some(""), Some("Rosson"), Some("rosson.k2.dev")] {
            let mut cfg = sample();
            cfg.confirmed_label = confirmed.map(str::to_string);
            for e2e in [false, true] {
                let toml = render_frpc_toml(&cfg, 57839, e2e);
                assert!(!toml.contains("[[proxies]]"), "{confirmed:?}\n{toml}");
                assert!(!toml.contains("k2so-"), "{toml}");
                assert!(toml.contains("[metadatas]\ntoken = \"tok_secret\""), "frpc still logs in\n{toml}");
                let with_custom = render_frpc_toml_with(&cfg, 57839, e2e, &["a.example.com".to_string()]);
                assert!(!with_custom.contains("[[proxies]]"), "no label, no custom proxy either\n{with_custom}");
            }
            assert_eq!(render_voice_frpc_toml(&cfg, &relay), None);
        }
    }

    /// The two namings, side by side.
    #[test]
    fn config_naming_uses_the_subdomain_and_confirmed_naming_the_label() {
        let mut cfg = sample();
        cfg.confirmed_label = Some("rosson".into());
        cfg.subdomain = "old-box".into();
        assert_eq!(main_proxy_name_in(&cfg, true).as_deref(), Some("k2so-old-box"));
        assert_eq!(main_proxy_name_in(&cfg, false).as_deref(), Some("k2so-rosson"));
        cfg.subdomain = String::new();
        assert_eq!(main_proxy_name_in(&cfg, true).as_deref(), Some("k2so-daemon"));
        cfg.confirmed_label = None;
        assert_eq!(main_proxy_name_in(&cfg, false), None);
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
        // 0.46.3: only the E2E proxy carries frpc's PROXY v2 header.
        assert!(
            on.contains("transport.proxyProtocolVersion = \"v2\"\n"),
            "e2e=true must ask frpc for a PROXY v2 header\n{on}"
        );
        assert!(
            !off.contains("proxyProtocolVersion"),
            "the cleartext http proxy must not get a PROXY header\n{off}"
        );
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

    /// §6.6 M4: the voice lane's whole config, pinned line for line.
    #[test]
    fn voice_lane_renders_exactly_the_m4_shape() {
        let mut cfg = sample();
        cfg.device_id = Some("dev-abc-123".to_string());
        let relay = RelayEndpoint { host: "relay.k2.dev".to_string(), port: 7000 };
        let toml = render_voice_frpc_toml(&cfg, &relay).expect("a label renders");
        let body: Vec<&str> = toml.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).collect();
        assert_eq!(
            body,
            vec![
                "serverAddr = \"relay.k2.dev\"",
                "serverPort = 7000",
                "[metadatas]",
                "token = \"tok_secret\"",
                "device_id = \"dev-abc-123\"",
                "role = \"voice\"",
                "[[proxies]]",
                "name = \"k2voice-rosson\"",
                "type = \"https\"",
                "customDomains = [\"turn.rosson.k2.dev\"]",
                "localIP = \"127.0.0.1\"",
                "localPort = 5349",
                "transport.proxyProtocolVersion = \"v2\"",
            ],
            "{toml}"
        );
        assert!(!toml.contains("tcpMux"), "tcpMux stays frp's default (on): no line\n{toml}");
        assert!(!toml.contains("subdomain"), "only the exact TURN host, never the label's wildcard\n{toml}");
        // No device id: the key is left out, as in the main config.
        cfg.device_id = None;
        assert!(!render_voice_frpc_toml(&cfg, &relay).unwrap().contains("device_id"));
        // No confirmed label: nothing to carry.
        cfg.confirmed_label = None;
        assert_eq!(render_voice_frpc_toml(&cfg, &relay), None);
    }

    /// The voice lane changes nothing in the main frpc's config: same bytes
    /// as before 0.46.8 (no turn proxy, no role meta).
    #[test]
    fn the_main_config_carries_no_turn_proxy() {
        for e2e in [false, true] {
            let main = render_frpc_toml(&sample(), 57839, e2e);
            assert!(!main.contains("turn."), "{main}");
            assert!(!main.contains("k2voice"), "{main}");
            assert!(!main.contains("role"), "{main}");
            assert_eq!(main.matches("[[proxies]]").count(), 1, "{main}");
        }
    }

    /// The voice lane follows the main frpc's relay: rendered for the relay
    /// the main frpc dials, only serverAddr/serverPort differ.
    #[test]
    fn voice_lane_render_swaps_only_the_relay() {
        let cfg = sample();
        let a = render_voice_frpc_toml(&cfg, &RelayEndpoint { host: "1.2.3.4".into(), port: 7000 }).unwrap();
        let b = render_voice_frpc_toml(&cfg, &RelayEndpoint { host: "5.6.7.8".into(), port: 7001 }).unwrap();
        let strip = |s: &str| s.lines().filter(|l| !l.starts_with("server")).collect::<Vec<_>>().join("\n");
        assert_eq!(strip(&a), strip(&b));
        assert!(b.contains("serverAddr = \"5.6.7.8\"") && b.contains("serverPort = 7001"));
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

    /// PB-2 / PRD test 4 / PB-A4: the custom proxy lists exactly the names
    /// given, has PROXY v2 and the same localPort, rides every relay's
    /// render byte for byte, and an empty list (or E2E off) renders none.
    #[test]
    fn the_custom_proxy_carries_the_path_b_names_in_every_relay_render() {
        let cfg = sample();
        let names = vec!["a.example.com".to_string(), "shop.example.com".to_string()];
        let toml = render_frpc_toml_with(&cfg, 57839, true, &names);
        let custom = toml.split("[[proxies]]").nth(2).expect("a second proxy");
        assert!(custom.contains("name = \"k2-custom-rosson\""), "{toml}");
        assert!(custom.contains("type = \"https\""), "{toml}");
        assert!(custom.contains("localPort = 57839"), "same local port\n{toml}");
        assert!(custom.contains("customDomains = [\"a.example.com\", \"shop.example.com\"]"), "{toml}");
        assert!(custom.contains("transport.proxyProtocolVersion = \"v2\""), "{toml}");
        assert!(!custom.contains("subdomain ="), "{toml}");
        assert_eq!(toml.matches("[[proxies]]").count(), 2);
        // No names, or E2E off: exactly today's render.
        assert_eq!(render_frpc_toml_with(&cfg, 57839, true, &[]), render_frpc_toml(&cfg, 57839, true));
        assert_eq!(render_frpc_toml_with(&cfg, 57839, false, &names), render_frpc_toml(&cfg, 57839, false));
        // Every relay: only the server lines differ.
        let a = RelayEndpoint { host: "relay-a.example".into(), port: 7000 };
        let b = RelayEndpoint { host: "relay-b.example".into(), port: 7001 };
        let strip = |t: &str| {
            t.lines()
                .filter(|l| !l.starts_with("serverAddr") && !l.starts_with("serverPort"))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let ra = render_frpc_toml_for_relay_with(&cfg, &a, 57839, true, &names);
        let rb = render_frpc_toml_for_relay_with(&cfg, &b, 57839, true, &names);
        assert!(ra.contains("customDomains") && rb.contains("customDomains"));
        assert_eq!(strip(&ra), strip(&rb), "byte-identical apart from the relay");
        // A name can't break out of its string.
        let evil = render_frpc_toml_with(&cfg, 1, true, &["x\"]\nname = \"evil".to_string()]);
        assert!(evil.contains(r#"customDomains = ["x\"]\nname = \"evil"]"#), "{evil}");
    }
}
