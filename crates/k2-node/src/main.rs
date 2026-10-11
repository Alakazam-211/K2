//! k2-node: the compute node runtime (prd-k2-compute-nodes-v1 §8).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use k2_node::config;
use k2_node::node::{Node, NodeOptions, NODE_VERSION};
use k2_node::paths::{default_home, Layout, DEFAULT_CONFIG_DIR};
use k2_node_proto::frames::Control;
use k2_node_proto::pairing::EnrollString;

const USAGE: &str = r#"k2-node — K2 compute node runtime

Install (root, once; the installers are inside this binary):
  sudo k2-node setup [--private-home | --no-private-home] [--human USER]
                     [--allow-packages "libssl-dev pkg-config"]
                     [--controller URL --enroll CODE.FP --name NAME]
  sudo k2-node uninstall
  Over SSH: one sudo call, password on stdin (never in argv):
    ssh host 'sudo -S -p "" /path/to/k2-node setup --no-private-home …' < password-file
    (the password comes from stdin, e.g. a secret file you delete after)

  k2-node run [--home DIR] [--config-dir DIR] [--dev]
  k2-node enroll --controller <https://you.k2.dev> (--enroll <ABCDE-FGHJK.0123456789abcdef> | --ticket T) --name <name>
                 [--label k=v]... [--home DIR] [--force]
  k2-node request --controller URL (--enroll CODE.FP | --ticket T) --name NAME [--config-dir DIR]
                 (the machine owner: ask the running node to enroll itself)
  k2-node tools install <need>... [--home DIR]   (as the node user: node@22 python@3.12 rust@stable cargo-nextest sys:pkg-config)
  k2-node tools list [--home DIR] [--json]
  k2-node packages list | allow <pkg>... | disallow <pkg>...   (allow/disallow: root)
  k2-node status [--home DIR] [--json]
  k2-node pause [--now] [--by NAME] [--config-dir DIR]     (machine owner: root or k2nodectl)
  k2-node drain [--by NAME] [--config-dir DIR]
  k2-node resume [--by NAME] [--config-dir DIR]
  k2-node policy show [--config-dir DIR] [--home DIR]
  k2-node policy set key=value... [--config-dir DIR] [--home DIR]
  k2-node install-files --os linux|macos --binary PATH --home DIR --config-dir DIR --user NAME --group NAME --out DIR
  k2-node check-home <dir>        (installer: refuse a home that runs K2)
  k2-node version

Remote admin (prd-k2-node-admin-v1; root, at this machine):
  sudo k2-node admin level off|policy|full      raise or lower remote admin (raising only here)
  sudo k2-node admin pin                         pin the enrolled controller's key for the executor
  sudo k2-node admin init [--level L] [--auto-update follow|off]   (installer)
  sudo k2-node admin audit [--tail N] [--json]   check + print the root audit chain
  k2-node admin status                           what the root executor says
  k2-node admin serve --config-dir D --state-dir S --home H --binary B   (the root executor; started by systemd/launchd)

Agents never run this: they use `k2 compute` on their K2 server."#;

struct Args {
    flags: BTreeMap<String, Vec<String>>,
    switches: Vec<String>,
    positional: Vec<String>,
}

const SWITCHES: &[&str] = &[
    "--dev",
    "--json",
    "--now",
    "--force",
    "-h",
    "--help",
    "--private-home",
    "--no-private-home",
    "--reenroll",
    "--nextest",
];

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args { flags: BTreeMap::new(), switches: vec![], positional: vec![] };
    let mut i = 0;
    while i < args.len() {
        let s = &args[i];
        if SWITCHES.contains(&s.as_str()) {
            a.switches.push(s.clone());
        } else if let Some(name) = s.strip_prefix("--") {
            let (k, v) = match name.split_once('=') {
                Some((k, v)) => (k.to_string(), v.to_string()),
                None => {
                    i += 1;
                    let v = args.get(i).ok_or_else(|| format!("--{name} needs a value"))?.clone();
                    (name.to_string(), v)
                }
            };
            a.flags.entry(k).or_default().push(v);
        } else {
            a.positional.push(s.clone());
        }
        i += 1;
    }
    Ok(a)
}

impl Args {
    fn get(&self, k: &str) -> Option<&str> {
        self.flags.get(k).and_then(|v| v.last()).map(|s| s.as_str())
    }
    fn need(&self, k: &str) -> Result<&str, String> {
        self.get(k).ok_or_else(|| format!("--{k} is required"))
    }
    fn has(&self, s: &str) -> bool {
        self.switches.iter().any(|x| x == s)
    }
    fn layout(&self) -> Layout {
        Layout::new(
            self.get("home").map(PathBuf::from).unwrap_or_else(default_home),
            self.get("config-dir").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_DIR)),
        )
    }
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

fn passwd_home() -> Option<PathBuf> {
    unsafe {
        let pw = libc::getpwuid(libc::geteuid());
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(PathBuf::from(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned()))
    }
}

/// CN25: never run as a user who runs K2 itself.
fn k2_files_in_home() -> Vec<String> {
    passwd_home().map(|h| k2_node::paths::k2_files_in(&h)).unwrap_or_default()
}

fn who(a: &Args) -> String {
    if let Some(b) = a.get("by") {
        return b.to_string();
    }
    std::env::var("SUDO_USER").ok().filter(|s| !s.is_empty()).unwrap_or_else(k2_node::node::current_user)
}

async fn cmd_run(a: &Args) -> Result<(), String> {
    if is_root() {
        return Err("k2-node never runs as root (jobs would run as root). Run it as the node user, e.g. k2node.".into());
    }
    let found = k2_files_in_home();
    if !found.is_empty() {
        if a.has("--dev") {
            eprintln!("WARNING: --dev: running as a user who runs K2 ({}). Jobs still get a scrubbed env and their own HOME, but they can read this user's files. Dev/smoke only.", found.join(", "));
        } else {
            return Err(format!(
                "this user runs K2 itself ({}). A compute node must run as its own user (k2node); see scripts/node/. (--dev overrides for local smoke tests.)",
                found.join(", ")
            ));
        }
    }
    let mut opts = NodeOptions::new(a.layout());
    opts.dev = a.has("--dev");
    let node = Node::open(opts).await?;
    k2_node::nlog!("k2-node {NODE_VERSION} fp {} home {}", &node.key.fingerprint()[..16], node.layout().home.display());
    #[cfg(k2_node_test_broken)]
    {
        return Err("K2_NODE_TEST_BROKEN build: exiting at once (test-keys only)".into());
    }
    #[allow(unreachable_code)]
    node.spawn_background();
    {
        // No controller to shake hands with: starting is healthy enough
        // for a self-update's health gate (enrolled nodes mark it after
        // the attach, in the session).
        let pin = node.pin.lock().unwrap().clone();
        if pin.map(|p| p.revoked).unwrap_or(true) {
            k2_node::admin::update::mark_healthy(&node.layout().home, true);
        }
    }
    let runner = tokio::spawn(k2_node::session::run_forever(node.clone()));
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(|e| e.to_string())?;
    tokio::select! {
        _ = term.recv() => {}
        _ = tokio::signal::ctrl_c() => {}
        _ = runner => {}
    }
    k2_node::nlog!("shutting down: stopping running jobs");
    node.stop_all(k2_node_proto::frames::JobState::Interrupted, "node_shutdown");
    // Give jobs their SIGTERM grace (10 s) to finish and record a receipt.
    for _ in 0..60 {
        if node.running_ids().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    node.set_status_state("offline", None, Some("k2-node stopped".into()));
    Ok(())
}

async fn cmd_enroll(a: &Args) -> Result<(), String> {
    if is_root() {
        return Err("enroll as the node user (sudo -u k2node k2-node enroll …), not root".into());
    }
    let controller = a.need("controller")?;
    let cred = match (a.get("enroll"), a.get("ticket")) {
        (Some(e), None) => k2_node::enroll::Credential::Code(EnrollString::parse(e).ok_or(
            "--enroll must look like ABCDE-FGHJK.0123456789abcdef (code . first 16 hex of the controller fingerprint)",
        )?),
        (None, Some(t)) => k2_node::enroll::Credential::Ticket(k2_node_proto::ticket::Ticket::decode(t)?),
        _ => return Err("pass --enroll <code> or --ticket <ticket>".into()),
    };
    let name = a.need("name")?;
    let mut labels = BTreeMap::new();
    for l in a.flags.get("label").cloned().unwrap_or_default() {
        let (k, v) = l.split_once('=').ok_or_else(|| format!("--label {l:?}: use key=value"))?;
        if matches!(k, "os" | "arch" | "name" | "vm") {
            return Err(format!("--label {k} is set by the node itself"));
        }
        labels.insert(k.to_string(), v.to_string());
    }
    let layout = a.layout();
    layout.ensure()?;
    if let Some(p) = k2_node::identity::read_pin(&layout.pin())? {
        if !p.revoked && !a.has("--force") {
            return Err(format!(
                "already enrolled as {} with {}; remove it on the controller first (or --force to replace)",
                p.name,
                p.controller_label()
            ));
        }
    }
    let key = k2_node::identity::load_or_create_key(&layout.key())?;
    let mut ws = k2_node::session::connect(controller, k2_node::session::ENROLL_PATH).await?;
    let done = k2_node::enroll::enroll_on(&mut ws, &key, controller, &cred, name, &labels, k2_node::util::now()).await?;
    k2_node::identity::write_pin(&layout.pin(), &done.pin)?;
    println!("Enrolled as {} (pending). Code: {}", done.pin.name, done.sas);
    println!("Confirm it on the controller: k2 compute node confirm {} {}", done.pin.name, done.sas.replace(' ', ""));
    Ok(())
}

fn cmd_status(a: &Args) -> Result<(), String> {
    let s = k2_node::status::read(&a.layout().status())?;
    if a.has("--json") {
        println!("{}", serde_json::to_string_pretty(&s).map_err(|e| e.to_string())?);
        return Ok(());
    }
    println!("k2-node {} (protocol {})  fp {}", s.version, s.protocol, s.node_fp.get(..16).unwrap_or(""));
    println!("state: {}{}", s.state, s.route.as_ref().map(|r| format!(" via {r}")).unwrap_or_default());
    if let Some(n) = &s.name {
        println!("name: {n}  controller: {}", s.controller.clone().unwrap_or_default());
    }
    if let Some(sas) = &s.sas {
        println!("waiting for confirmation on the controller, code {sas}");
    }
    println!("control: {}{}", s.control, s.control_by.as_ref().map(|b| format!(" (by {b})")).unwrap_or_default());
    if let Some(e) = &s.policy_error {
        println!("policy error: {e}");
    }
    if !s.unavailable.is_empty() {
        println!("unavailable: {}", s.unavailable.join(", "));
    }
    if let Some(l) = &s.holding_lock {
        println!("holding: {l}");
    }
    for l in &s.stale_locks {
        println!("stale lock (reported, not deleted): {l}");
    }
    println!("running: {}", if s.running.is_empty() { "-".to_string() } else { s.running.join(", ") });
    println!("caps: {}", if s.caps_hard { "hard (cgroups)" } else { "soft" });
    if let Some(e) = &s.last_error {
        println!("last error: {e}");
    }
    Ok(())
}

fn cmd_control(a: &Args, c: Control) -> Result<(), String> {
    let layout = a.layout();
    config::write_control(&layout.control(), c, &who(a), k2_node::util::now())?;
    println!("{} — k2-node picks it up within a few seconds{}", c.as_str(), if c == Control::Stopped { "; running jobs are stopped" } else { "" });
    Ok(())
}

fn cmd_policy(a: &Args) -> Result<(), String> {
    let layout = a.layout();
    let facts = k2_node::node::machine_facts(&layout);
    match a.positional.get(1).map(|s| s.as_str()) {
        Some("show") | None => {
            let f = config::read_policy_file(&layout.policy())?;
            let p = config::Policy::resolve(&f, &facts)?;
            println!("{}", serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some("set") => {
            let mut pairs = Vec::new();
            for kv in &a.positional[2..] {
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("{kv:?}: use key=value"))?;
                pairs.push((k.trim().to_string(), v.trim().to_string()));
            }
            if pairs.is_empty() {
                return Err("policy set needs key=value pairs".into());
            }
            let p = config::set_policy(&layout.policy(), &pairs, &facts)?;
            println!("{}", serde_json::to_string_pretty(&p).map_err(|e| e.to_string())?);
            Ok(())
        }
        Some(o) => Err(format!("policy {o:?}: use show or set")),
    }
}

/// Installer helper: exit 1 when `<dir>/.k2` holds K2 daemon files (CN25).
fn cmd_check_home(a: &Args) -> Result<(), String> {
    let dir = a.positional.get(1).ok_or("check-home <home dir>")?;
    let found = k2_node::paths::k2_files_in(std::path::Path::new(dir));
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!("{dir} belongs to a user who runs K2 ({}); a compute node needs its own user", found.join(", ")))
    }
}

fn cmd_request(a: &Args) -> Result<(), String> {
    let mut labels = BTreeMap::new();
    for l in a.flags.get("label").cloned().unwrap_or_default() {
        let (k, v) = l.split_once('=').ok_or_else(|| format!("--label {l:?}: use key=value"))?;
        labels.insert(k.to_string(), v.to_string());
    }
    let r = k2_node::request::Request {
        id: format!("cli-{}", k2_node_proto::crypto::random_id()?),
        controller: a.need("controller")?.to_string(),
        enroll: a.get("enroll").map(str::to_string),
        ticket: a.get("ticket").map(str::to_string),
        name: a.need("name")?.to_string(),
        labels,
        requested_by: Some(who(a)),
        requested_at: Some(k2_node::util::now()),
    };
    let layout = a.layout();
    k2_node::request::write_request(&layout, &r)?;
    println!("Asked the node to enroll as {} (request {}). Watch: k2-node status", r.name, r.id);
    Ok(())
}

/// `tools install|list` (as the node user; never root: installs land in
/// the node home and must stay owned by it).
async fn cmd_tools(a: &Args) -> Result<(), String> {
    let layout = a.layout();
    let base = vec![
        ("PATH".to_string(), k2_node::util::base_path(&layout.home)),
        ("HOME".to_string(), layout.git_home().display().to_string()),
        ("LANG".to_string(), "C".to_string()),
    ];
    let tc = k2_node::toolchain::Toolchains::new(&layout.home, base);
    match a.positional.get(1).map(String::as_str) {
        Some("list") | None => {
            let t = tc.installed_tools();
            if a.has("--json") {
                println!("{}", serde_json::to_string_pretty(&t).map_err(|e| e.to_string())?);
            } else if t.is_empty() {
                println!("(no toolchains installed in {} yet)", tc.root.display());
            } else {
                for (k, v) in t {
                    println!("{k:<18} {v}");
                }
            }
            Ok(())
        }
        Some("install") => {
            if is_root() {
                return Err("install toolchains as the node user, not root: sudo -u k2node k2-node tools install … (macOS: sudo -u _k2node …)".into());
            }
            let mut toks: Vec<String> = a.positional[2..].to_vec();
            if a.has("--nextest") {
                toks.push("cargo-nextest".into());
            }
            if toks.is_empty() {
                return Err("tools install <need>…  e.g. node@22 python@3.12 rust@stable cargo-nextest sys:pkg-config".into());
            }
            let needs = k2_node_proto::needs::parse_all(&toks)?;
            layout.ensure()?;
            let r = tc
                .resolve(&needs, &[], None, &mut |m: &str| println!("{m}"))
                .await
                .map_err(|(code, d)| format!("{code}: {d}"))?;
            for n in &r.notes {
                println!("{n}");
            }
            println!("ready: {} (k2-node re-probes tools within 10 minutes)", toks.join(" "));
            Ok(())
        }
        Some(o) => Err(format!("tools {o:?}: use install or list")),
    }
}

fn cmd_packages(a: &Args) -> Result<(), String> {
    let names: Vec<String> = a.positional.iter().skip(2).cloned().collect();
    match a.positional.get(1).map(String::as_str) {
        Some("list") | None => {
            let l = k2_node::packages::read_approved(std::path::Path::new(k2_node::packages::APPROVED_FILE))?;
            let m = k2_node::packages::detect().map(|m| m.name()).unwrap_or("none");
            let unit = k2_node::packages::installer_present();
            println!(
                "package manager: {m}; installer: {}",
                if unit { k2_node::packages::PATH_UNIT } else { "none (jobs can't install packages; re-run sudo k2-node setup)" }
            );
            if l.is_empty() {
                println!("approved: (none)");
            }
            for p in l {
                let ok = k2_node::packages::installed(k2_node::sysinfo::os_label(), &p);
                println!("approved: {p}{}", if ok { " (installed)" } else { "" });
            }
            Ok(())
        }
        Some("allow") => {
            let l = k2_node::packages::cmd_edit(&names, true)?;
            println!("approved: {}", l.join(" "));
            Ok(())
        }
        Some("disallow") => {
            let l = k2_node::packages::cmd_edit(&names, false)?;
            println!("approved: {}", if l.is_empty() { "(none)".to_string() } else { l.join(" ") });
            Ok(())
        }
        Some("install") => k2_node::packages::cmd_install(&names),
        Some("serve") => k2_node::packages::cmd_serve(&a.layout().home),
        Some(o) => Err(format!("packages {o:?}: use list, allow, disallow")),
    }
}

fn cmd_install_files(a: &Args) -> Result<(), String> {
    let p = k2_node::install::InstallParams {
        binary: a.need("binary")?,
        home: a.need("home")?,
        config: a.need("config-dir")?,
        user: a.need("user")?,
        group: a.need("group")?,
    };
    let out = PathBuf::from(a.need("out")?);
    for n in k2_node::install::write_files(a.need("os")?, &p, &out)? {
        println!("{}", out.join(n).display());
    }
    Ok(())
}

fn cmd_admin(a: &Args) -> Result<(), String> {
    use k2_node::admin::{cli, config as acfg, exec::ExecPaths, units::AdminUnitParams};
    use k2_node_proto::admin::Level;
    let config_dir = a.get("config-dir").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_DIR));
    let home = a.get("home").map(PathBuf::from).unwrap_or_else(default_home);
    let state = a.get("state-dir").map(PathBuf::from).unwrap_or_else(acfg::default_state_dir);
    let socket = a.get("socket").map(PathBuf::from).unwrap_or_else(acfg::default_socket);
    let level_arg = |s: Option<&String>| -> Result<Level, String> {
        let s = s.ok_or("give a level: off, policy or full")?;
        Level::parse(s).ok_or_else(|| format!("{s:?}: use off, policy or full"))
    };
    match a.positional.get(1).map(String::as_str) {
        Some("serve") => {
            let binary = PathBuf::from(a.need("binary")?);
            cli::serve(ExecPaths { config_dir, state_dir: state, home, binary }, &socket)
        }
        Some("init") => {
            let level = match a.get("level") {
                Some(l) => Some(Level::parse(l).ok_or_else(|| format!("--level {l:?}: use off, policy or full"))?),
                None => None,
            };
            println!("{}", cli::init(&config_dir, &home, level, a.get("auto-update"))?);
            Ok(())
        }
        Some("pin") => {
            println!("{}", cli::pin(&config_dir, &home)?);
            Ok(())
        }
        Some("level") => {
            println!("{}", cli::level(&config_dir, level_arg(a.positional.get(2))?)?);
            Ok(())
        }
        Some("audit") => {
            let tail = a.get("tail").map(|t| t.parse::<usize>().map_err(|_| "--tail N")).transpose()?.unwrap_or(50);
            cli::audit(&state, tail, a.has("--json"))
        }
        Some("status") => cli::status(&socket),
        Some("install-files") => {
            let gid = a.get("group").unwrap_or("k2node").to_string();
            let p = AdminUnitParams {
                binary: a.need("binary")?,
                home: a.need("home")?,
                config: a.need("config-dir")?,
                state: a.need("state-dir")?,
                socket: a.need("socket")?,
                group: &gid,
            };
            for f in cli::install_files(a.need("os")?, &p, std::path::Path::new(a.need("out")?))? {
                println!("{}", f.display());
            }
            Ok(())
        }
        Some(o) => Err(format!("admin {o:?}: use level, pin, audit, status, serve")),
        None => Err("admin: use level, pin, audit, status, serve".into()),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let a = match parse(&raw) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("k2-node: {e}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    if a.has("-h") || a.has("--help") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let r = match a.positional.first().map(|s| s.as_str()) {
        Some("run") => cmd_run(&a).await,
        Some("enroll") => cmd_enroll(&a).await,
        Some("status") => cmd_status(&a),
        Some("pause") => cmd_control(&a, if a.has("--now") { Control::Stopped } else { Control::Paused }),
        Some("drain") => cmd_control(&a, Control::Draining),
        Some("resume") => cmd_control(&a, Control::Active),
        Some("policy") => cmd_policy(&a),
        Some("install-files") => cmd_install_files(&a),
        Some("request") => cmd_request(&a),
        Some("tools") => cmd_tools(&a).await,
        Some("packages") => cmd_packages(&a),
        Some("setup") | Some("uninstall") => {
            let kind = if a.positional[0] == "setup" { k2_node::setup::Kind::Install } else { k2_node::setup::Kind::Uninstall };
            // Pass the original argv through untouched (after the verb).
            let rest: Vec<String> = raw.iter().skip_while(|x| *x != &a.positional[0]).skip(1).cloned().collect();
            match k2_node::setup::run(kind, &rest) {
                Ok(0) => Ok(()),
                Ok(code) => return ExitCode::from(code.clamp(1, 255) as u8),
                Err(e) => Err(e),
            }
        }
        Some("check-home") => cmd_check_home(&a),
        Some("admin") => cmd_admin(&a),
        Some("version") => {
            println!("k2-node {NODE_VERSION} (protocol {})", k2_node_proto::PROTOCOL);
            let _ = k2_node::build_marker();
            Ok(())
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("k2-node: {e}");
            ExitCode::from(1)
        }
    }
}
