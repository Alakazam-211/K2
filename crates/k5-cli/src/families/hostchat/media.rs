//! `k5 hostchat media …` (`_hostchat_media`, `cli/src/hostchat-voice.sh`):
//! the voice engine on this box: preflight, status, install (and its
//! wait), url, renew, rebind, tier, probe and uninstall.

use super::main_verbs::{abs, field, show, wait_enable};
use super::print::{g, load};
use super::{cj, usage, Ctx, Plan};
use crate::exit::Exit;
use crate::families::voice::sew::{self, Field};
use crate::families::{raised, Shown};
use crate::out;
use crate::prompt::{self, AtEof, Echo};
use k2_client::brand::{CLI_NAME, PRODUCT_NAME};

/// A media verb and its flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Media {
    pub sub: String,
    pub json: bool,
    pub url: String,
    pub node_ip: String,
    pub tarball: String,
    pub wait: bool,
    pub udp: String,
    pub tcp: String,
    pub tls: String,
    pub yes: bool,
    pub pos: Vec<String>,
}

/// The words after `media`.
pub fn parse(words: &[&str]) -> Plan {
    let c = CLI_NAME;
    let (sub, rest): (&str, &[&str]) = match words.split_first() {
        Some((&s, r)) => (s, r),
        None => ("", &[]),
    };
    let mut m = Media {
        sub: sub.to_string(),
        wait: true,
        ..Media::default()
    };
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i];
        let slot = match w {
            "--json" => {
                m.json = true;
                None
            }
            "--yes" | "-y" => {
                if sub != "uninstall" {
                    return Plan::Bad(usage(&format!(
                        "{w} only applies to {c} hostchat media uninstall"
                    )));
                }
                m.yes = true;
                None
            }
            "--url" | "--voice-url" | "--public-url" => Some(&mut m.url),
            "--node-ip" => Some(&mut m.node_ip),
            "--tarball" | "--media-tarball" => Some(&mut m.tarball),
            "--no-wait" => {
                m.wait = false;
                None
            }
            "--udp" => Some(&mut m.udp),
            "--tcp" => Some(&mut m.tcp),
            "--turn-tls" => Some(&mut m.tls),
            "-h" | "--help" => return Plan::Help,
            w if w.starts_with('-') => {
                return Plan::Bad(usage(&format!("unknown flag {w} ({c} hostchat --help)")))
            }
            w => {
                m.pos.push(w.to_string());
                i += 1;
                continue;
            }
        };
        match slot {
            None => i += 1,
            Some(slot) => {
                let Some(v) = rest.get(i + 1).filter(|v| !v.is_empty()) else {
                    return Plan::Shell;
                };
                *slot = v.to_string();
                i += 2;
            }
        }
    }
    m.tarball = abs(&m.tarball);
    match sub {
        "preflight" | "status" | "install" | "url" | "renew" | "rebind" | "tier" | "probe"
        | "uninstall" => Plan::Media(m),
        "" | "help" => Plan::Help,
        _ => Plan::Bad(format!(
            "{{\"error\":\"usage\",\"hint\":\"unknown media command '{sub}' ({c} hostchat --help)\"}}"
        )),
    }
}

fn s(t: &str) -> Field {
    Field::S(t.to_string())
}

pub fn run(ctx: &mut Ctx, m: &Media) -> Exit {
    match verb(ctx, m) {
        Ok(e) | Err(e) => e,
    }
}

fn bad(hint: &str) -> Result<Exit, Exit> {
    out::stderr_line(&usage(hint));
    Err(Exit::Usage)
}

fn verb(ctx: &mut Ctx, m: &Media) -> Result<Exit, Exit> {
    let c = CLI_NAME;
    let json = m.json;
    let post = |ctx: &mut Ctx, route: &str, payload: &str, verb: &str| -> Result<Exit, Exit> {
        let body = ctx.call(true, route, payload, false)?;
        Ok(show(&body, verb, json))
    };
    let pos0 = m.pos.first().map_or("", String::as_str);
    match m.sub.as_str() {
        "preflight" => {
            let body = ctx.call(false, "/cli/sew/media/preflight", "", false)?;
            Ok(show(&body, "media-preflight", json))
        }
        "status" => {
            let body = ctx.call(false, "/cli/sew/status", "health=1", false)?;
            if !json {
                return Ok(show(&body, "status", false));
            }
            Ok(match load(&body, 1).and_then(|v| g(&v, "voice", 1)) {
                Ok(v) => Shown::out(format!("{}\n", sew::dumps_default(&v))).print(),
                Err(e) => raised(String::new(), String::new(), &e).print(),
            })
        }
        "install" => {
            let payload = cj(&[
                ("publicUrl", s(&m.url)),
                ("nodeIp", s(&m.node_ip)),
                ("mediaTarballPath", s(&m.tarball)),
            ]);
            let body = ctx.call(true, "/cli/sew/media/install", &payload, false)?;
            if m.wait && field(&body, "voice").unwrap_or_default() == "installing" {
                return Ok(wait_enable(ctx, true, json));
            }
            Ok(show(&body, "media-install", json))
        }
        "url" => {
            let u = if pos0.is_empty() {
                m.url.as_str()
            } else {
                pos0
            };
            if u.is_empty() {
                return bad(&format!("{c} hostchat media url wss://HOST"));
            }
            post(
                ctx,
                "/cli/sew/media/url",
                &cj(&[("publicUrl", s(u))]),
                "media-url",
            )
        }
        "renew" => post(ctx, "/cli/sew/media/renew", "{}", "media-renew"),
        "rebind" => post(ctx, "/cli/sew/media/rebind", "{}", "media-rebind"),
        "tier" => {
            if !matches!(pos0, "tunnel" | "direct") {
                return bad(&format!("{c} hostchat media tier tunnel|direct (tunnel: relay-only TURN through {PRODUCT_NAME} Connect, no public port; direct: public 5349/7882, the box IP in DNS)"));
            }
            post(
                ctx,
                "/cli/sew/media/tier",
                &cj(&[("tier", s(pos0))]),
                "media-tier",
            )
        }
        "probe" => {
            if m.udp.is_empty() || m.tcp.is_empty() || m.tls.is_empty() {
                return bad(&format!(
                    "{c} hostchat media probe --udp ok|fail --tcp ok|fail --turn-tls ok|fail"
                ));
            }
            let payload = cj(&[
                ("udp", s(&m.udp)),
                ("tcp", s(&m.tcp)),
                ("turnTls", s(&m.tls)),
            ]);
            post(ctx, "/cli/sew/media/probe", &payload, "media-probe")
        }
        _ => {
            if !m.yes {
                if !prompt::stdin_is_tty() {
                    out::stderr_line(&format!("{{\"error\":\"confirm_required\",\"hint\":\"Uninstall voice? Live calls end. There is no terminal to ask on: pass --yes ({c} hostchat media uninstall --yes)\"}}"));
                    return Err(Exit::Usage);
                }
                prompt::prompt("Uninstall voice? Live calls end. [y/N] ");
                let ans = prompt::read_line(Echo::Shown, AtEof::Drop);
                if !matches!(ans.expose(), b"y" | b"Y" | b"yes" | b"YES" | b"Yes") {
                    out::stderr_line("Voice not removed.");
                    return Err(Exit::Error);
                }
            }
            post(
                ctx,
                "/cli/sew/media/uninstall",
                r#"{"confirm": true}"#,
                "media-uninstall",
            )
        }
    }
}
