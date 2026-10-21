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

  k2-node run [--home DIR] [--config-dir DIR] [--dev [--allow-fake-clock]]   (tests: K2_NODE_FAKE_CLOCK_OFFSET=+600 moves a --dev node's clock)
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

More than one K2 server (0.46.3; the machine owner, at this machine):
  k2-node servers [list] [--json]                        every server this node serves
  sudo k2-node servers add --url URL --code CODE.FP [--trust own|team] [--for 7d|none]
                 [--weight N] [--max-running N] [--max-queued N] [--cpu-hours N] [--max-time 2h]
                 [--cpus N] [--mem 8G] [--disk 40G] [--network on|no-lan|none] [--gate] [--exclusive]
  k2-node servers limits <server> [key=value...] [--weight N] [--for 7d|none]   (keys as the add flags)
  k2-node servers pause|resume <server>
  sudo k2-node servers revoke <server> [--now] [--wipe]
  sudo k2-node servers prune                             promote confirmed adds, drop stale ones, wipe due slots now
  (<server> is a label, e.g. baden.k2.dev, or slot:<n>; the new server mints CODE.FP
   with `k2 compute node add <this node>`; then read it the SAS that `servers` shows)
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
  sudo k2-node admin job-user <name>             Linux: jobs run as <name> in a walled unit (setup: k2job)
  k2-node admin homes [--json]                   home folders other users (and unwalled jobs) can read
  sudo k2-node admin homes --fix [--yes]         make them private (chmod 700); asks first unless --yes

The job wall (0.46.3): Linux jobs run as k2job in a sandboxed systemd unit; macOS
jobs run under a Seatbelt profile. Neither can read the node's key or state or
people's home folders, and network = "none" keeps them off the network.
  k2-node wall-probe [--read P]... [--list D]... [--connect IP:PORT]...   (tests: what this process can reach)

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
    "--fix",
    "--yes",
    "--gate",
    "--exclusive",
    "--wipe",
    "--allow-fake-clock",
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
    if let Some(off) = fake_clock_offset(std::env::var(FAKE_CLOCK_ENV).ok().as_deref(), opts.dev, a.has("--allow-fake-clock"))? {
        eprintln!("WARNING: {FAKE_CLOCK_ENV}={off}: this test node's clock is off by {off} s (tests only)");
        opts.clock = std::sync::Arc::new(OffsetClock { inner: k2_node::ledger::SystemClock::new(), offset: off });
    }
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
        let pin = node.owner().pin.lock().unwrap().clone();
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
    // A shared node's update or restart (the executor marked it): the jobs
    // it couldn't drain end `node_update`, infra without a strike (V55).
    let why = if k2_node::admin::update::recent_update_mark(&node.layout().config, k2_node::util::now()) {
        k2_node_proto::share::codes::NODE_UPDATE
    } else {
        "node_shutdown"
    };
    // Nothing new starts from here: the capacity the stops free must not
    // start queued work during the shutdown.
    node.begin_shutdown();
    node.stop_all(k2_node_proto::frames::JobState::Interrupted, why);
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
    let mut s = k2_node::status::read(&a.layout().status())?;
    k2_node::admin::servers::fill_sas(&a.layout(), &mut s.servers);
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
    if let Some(k) = s.clock_skew_secs.filter(|k| k.abs() > k2_node::status::SKEW_WARN_SECS) {
        println!("clock skew: this machine's clock is {k:+} s off the owner server's");
    }
    if let Some(e) = &s.servers_error {
        println!("servers.toml: {e}");
    }
    for l in &s.servers {
        println!("{}", server_line(l));
    }
    if let Some(e) = &s.last_error {
        println!("last error: {e}");
    }
    Ok(())
}

fn server_line(l: &k2_node::status::ServerLine) -> String {
    let mut s = format!("server slot {}: {}  {} ({}) · {}", l.slot, l.label, l.state, l.trust, l.link);
    if let Some(sas) = &l.sas {
        s.push_str(&format!(" · SAS {sas} (read it to {}'s owner: k2 compute node confirm <node> {})", l.label, sas.replace(' ', "")));
    }
    if l.running > 0 {
        s.push_str(&format!(" · running {}", l.running));
    }
    if let Some(e) = l.expires_at {
        s.push_str(&format!(" · until {}", fmt_time(e)));
    }
    if let Some(k) = l.skew_secs.filter(|k| k.abs() > k2_node::status::SKEW_WARN_SECS) {
        s.push_str(&format!(" · clock skew {k:+} s"));
    }
    if let Some(e) = &l.last_error {
        s.push_str(&format!(" · {e}"));
    }
    s
}

fn fmt_time(t: i64) -> String {
    let left = t - k2_node::util::now();
    if left <= 0 {
        "ended".into()
    } else if left < 2 * 3600 {
        format!("in {} min", left / 60)
    } else if left < 2 * 86400 {
        format!("in {} h", left / 3600)
    } else {
        format!("in {} days", left / 86400)
    }
}

/// `7d`, `12h`, `30m`, `90s`, or `none` (Some(None)).
fn parse_for(v: &str) -> Result<Option<i64>, String> {
    if v == "none" {
        return Ok(None);
    }
    let (n, unit) = v.split_at(v.find(|c: char| !c.is_ascii_digit()).unwrap_or(v.len()));
    let n: i64 = n.parse().map_err(|_| format!("{v:?}: use e.g. 7d, 12h, 30m or none"))?;
    let mul = match unit {
        "d" => 86400,
        "h" => 3600,
        "m" => 60,
        "s" | "" => 1,
        _ => return Err(format!("{v:?}: use e.g. 7d, 12h, 30m or none")),
    };
    Ok(Some(n * mul))
}

/// `8G` / `8` → 8 (whole GB).
fn parse_gb(v: &str) -> Result<u32, String> {
    v.trim_end_matches(['G', 'g', 'B', 'b']).parse().map_err(|_| format!("{v:?}: use whole GB, e.g. 8G"))
}

/// Share changes from `--flag value` (add) or `key=value` (limits).
fn share_overrides(pairs: &[(String, String)], gate: bool, exclusive: bool) -> Result<k2_node_proto::share::ShareOverrides, String> {
    use k2_node_proto::share::{JobClass, Network, ShareOverrides};
    let mut o = ShareOverrides::default();
    let num = |k: &str, v: &str| v.parse::<u32>().map_err(|_| format!("{k}: {v:?} is not a whole number"));
    for (k, v) in pairs {
        match k.replace('-', "_").as_str() {
            "weight" => o.weight = Some(num(k, v)?),
            "max_running" => o.max_running = Some(num(k, v)?),
            "max_queued" => o.max_queued = Some(num(k, v)?),
            "cpu_hours" | "cpu_hours_day" => o.cpu_hours_day = Some(num(k, v)?),
            "max_time" | "max_job_secs" => o.max_job_secs = Some(parse_for(v)?.ok_or("max time can't be none")? as u64),
            "cpus" | "max_cpus" => o.max_cpus = Some(num(k, v)?),
            "mem" | "max_mem_gb" => o.max_mem_gb = Some(parse_gb(v)?),
            "disk" | "disk_gb" => o.disk_gb = Some(parse_gb(v)?),
            "network" => o.network = Some(Network::parse(v).ok_or_else(|| format!("network {v:?}: on, no-lan or none"))?),
            "top_class" => o.top_class = Some(JobClass::parse(v).ok_or_else(|| format!("top_class {v:?}: batch, interactive or gate"))?),
            "gate" => o.gate = Some(v == "true"),
            "exclusive" => o.exclusive = Some(v == "true"),
            other => return Err(format!("unknown share setting {other:?}")),
        }
    }
    if gate {
        o.gate = Some(true);
        o.top_class = Some(JobClass::Gate);
    }
    if exclusive {
        o.exclusive = Some(true);
    }
    Ok(o)
}

/// `k2-node servers …`: the machine owner's own `servers.*` (§9).
fn cmd_servers(a: &Args) -> Result<(), String> {
    use k2_node::admin::{config as acfg, servers};
    use k2_node_proto::share;
    let layout = a.layout();
    let state_dir = a.get("state-dir").map(PathBuf::from).unwrap_or_else(acfg::default_state_dir);
    let cfg = acfg::read(&layout.config.join(acfg::ADMIN_FILE)).ok().flatten();
    let macos = cfg!(target_os = "macos");
    let ctx = servers::Ctx {
        config_dir: layout.config.clone(),
        state_dir,
        home: layout.home.clone(),
        now: k2_node::util::now(),
        macos,
        walled: servers::node_walled(&layout, macos, true),
        tenant_wall: false,
        pinned_owner: cfg.as_ref().and_then(|c| c.controller.as_ref().map(|x| x.fp.clone())),
        node_user: cfg.map(|c| c.node_user).unwrap_or_else(|| acfg::default_node_user().to_string()),
        by: format!("local:{}", who(a)),
        root: is_root(),
    };
    // A `--dev` test node (the person running it owns everything; tests
    // and the S5 harness pass their own --state-dir) needs no sudo.
    let dev_node = k2_node::node::dev_test_exemption(k2_node::status::read(&layout.status()).is_ok_and(|s| s.dev), &layout.config);
    let need_root = |what: &str| -> Result<(), String> {
        if is_root() || dev_node {
            Ok(())
        } else {
            Err(format!("servers {what} changes root's records (tenant users, the wipe schedule): run it with sudo"))
        }
    };
    let server = || a.positional.get(2).cloned().ok_or("give a server: its label (baden.k2.dev) or slot:<n>");
    let show = |v: serde_json::Value| -> Result<(), String> {
        println!("{}", serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?);
        Ok(())
    };
    let r = match a.positional.get(1).map(String::as_str) {
        Some("list") | None => {
            let file = servers::list_file(&ctx);
            let status = k2_node::status::read(&layout.status()).ok().map(|mut s| {
                servers::fill_sas(&layout, &mut s.servers);
                s
            });
            if a.has("--json") {
                return show(serde_json::json!({"file": file, "node": status.map(|s| s.servers)}));
            }
            let lines = file.get("servers").and_then(|v| v.as_array()).cloned().unwrap_or_default();
            if lines.is_empty() {
                println!("Solo: this node serves only its owner server (no servers.toml).");
                return Ok(());
            }
            for l in lines {
                let slot = l["slot"].as_u64().unwrap_or(0);
                let node_line = status.as_ref().and_then(|s| s.servers.iter().find(|x| x.slot as u64 == slot).cloned());
                match node_line {
                    Some(n) => println!("{}", server_line(&n)),
                    None => println!("server slot {slot}: {}  {} ({}){}", l["label"].as_str().unwrap_or(""), l["state"].as_str().unwrap_or(""), l["trust"].as_str().unwrap_or(""), if slot == 0 { " · owner" } else { "" }),
                }
            }
            if let Some(e) = file.get("error").and_then(|v| v.as_str()) {
                println!("servers.toml: {e}");
            }
            return Ok(());
        }
        Some("add") => {
            need_root("add")?;
            let code = k2_node_proto::pairing::EnrollString::parse(a.need("code")?)
                .ok_or("--code must look like ABCDE-FGHJK.0123456789abcdef (from `k2 compute node add` on the new server)")?;
            let trust = share::Trust::parse(a.get("trust").unwrap_or("own")).ok_or("--trust own|team")?;
            let mut pairs = Vec::new();
            for k in ["weight", "max-running", "max-queued", "cpu-hours", "max-time", "cpus", "mem", "disk", "network"] {
                if let Some(v) = a.get(k) {
                    pairs.push((k.to_string(), v.to_string()));
                }
            }
            let for_secs = match a.get("for") {
                Some(v) => parse_for(v)?,
                None => None,
            };
            let args = share::AddArgs {
                url: a.need("url")?.trim_end_matches('/').to_string(),
                code_binding: k2_node_proto::pairing::code_binding(&code.code),
                fp_prefix: code.controller_fp_prefix.clone(),
                trust,
                for_secs,
                limits: share_overrides(&pairs, a.has("--gate"), a.has("--exclusive"))?,
            };
            if dev_node && !is_root() {
                // A --dev test node has no wall: its other servers' jobs run
                // as you (review A L1). Own servers only, and said loudly.
                if trust == share::Trust::Team {
                    return Err("this is a --dev test node with no job wall: it never takes other people's servers (--trust team)".into());
                }
                eprintln!("WARNING: a --dev test node has no job wall: {}'s jobs run as you and can read your files. Tests only.", servers::label_of(&args.url));
            }
            if trust == share::Trust::Team {
                println!("Jobs from {} get their own user and folders here. They can't read your files or other servers' job folders. They do share this machine's kernel, CPU and disk, and they can reach the internet and any service on this machine's loopback, including servers other servers' jobs start there (and see this machine's open sockets in /proc/net). Only add people you trust.", servers::label_of(&args.url));
            }
            servers::run(&ctx, share::VERB_ADD, &serde_json::to_value(&args).map_err(|e| e.to_string())?)
        }
        Some("limits") => {
            let mut pairs = Vec::new();
            for kv in a.positional.iter().skip(3) {
                let (k, v) = kv.split_once('=').ok_or_else(|| format!("{kv:?}: use key=value"))?;
                pairs.push((k.to_string(), v.to_string()));
            }
            let for_secs = match a.get("for") {
                Some(v) => Some(parse_for(v)?.unwrap_or(0)),
                None => None,
            };
            if let Some(w) = a.get("weight") {
                pairs.push(("weight".to_string(), w.to_string()));
            }
            let args = share::LimitsArgs { server: server()?, set: share_overrides(&pairs, false, false)?, for_secs };
            servers::run(&ctx, share::VERB_LIMITS, &serde_json::to_value(&args).map_err(|e| e.to_string())?)
        }
        Some(v @ ("pause" | "resume")) => {
            let verb = if v == "pause" { share::VERB_PAUSE } else { share::VERB_RESUME };
            servers::run(&ctx, verb, &serde_json::json!({ "server": server()? }))
        }
        Some("revoke") => {
            need_root("revoke")?;
            let args = share::RevokeArgs { server: server()?, now: a.has("--now"), wipe: a.has("--wipe") };
            servers::run(&ctx, share::VERB_REVOKE, &serde_json::to_value(&args).map_err(|e| e.to_string())?)
        }
        // What the executor does every 30 s on its own (promote confirmed
        // adds, drop expired pending lines, wipe due slots), now.
        Some("prune") => {
            need_root("prune")?;
            servers::prune(&ctx)
        }
        Some(o) => return Err(format!("servers {o:?}: use list, add, limits, pause, resume, revoke, prune")),
    };
    match r {
        Ok(v) => {
            if a.has("--json") {
                return show(v);
            }
            let label = v.get("label").and_then(|x| x.as_str()).unwrap_or("");
            match a.positional.get(1).map(String::as_str) {
                Some("add") => println!(
                    "Added {label} (slot {}, pending). The node enrols with it now; `k2-node servers` shows the SAS to read to its owner within 10 minutes.",
                    v["slot"]
                ),
                Some("revoke") => println!("Revoked {label}: its session ends{}; the slot is wiped {}.", if v["now"] == true { " and its running jobs stop" } else { ", running jobs finish" }, if v["wiped"].as_array().is_some_and(|w| !w.is_empty()) { "now" } else { "in 24 h" }),
                _ => println!("{label}: {}", serde_json::to_string(&v).map_err(|e| e.to_string())?),
            }
            Ok(())
        }
        Err(r) => Err(format!("{}: {}", r.code, r.message)),
    }
}

/// A test-only node clock offset in seconds (S5 expiry and skew scenarios).
const FAKE_CLOCK_ENV: &str = "K2_NODE_FAKE_CLOCK_OFFSET";

/// The fake clock offset to use, if any. Honoured only on a `--dev` node
/// started with the explicit `--allow-fake-clock` switch: a real node (never
/// `--dev`) ignores the variable entirely, whatever its environment says.
/// With the switch, a malformed value is an error, never a silent zero.
fn fake_clock_offset(env: Option<&str>, dev: bool, allow: bool) -> Result<Option<i64>, String> {
    if allow && !dev {
        return Err("--allow-fake-clock is for --dev test nodes only".into());
    }
    if !allow {
        return Ok(None);
    }
    match env {
        None => Ok(None),
        Some(v) => {
            let v = v.trim().trim_start_matches('+');
            let n: i64 = v.parse().map_err(|_| format!("{FAKE_CLOCK_ENV}={v:?}: a whole number of seconds, e.g. +600"))?;
            if n.abs() > 366 * 24 * 3600 {
                return Err(format!("{FAKE_CLOCK_ENV}: at most a year either way"));
            }
            Ok(Some(n))
        }
    }
}

/// The system clock moved by a fixed offset (test nodes only).
struct OffsetClock {
    inner: k2_node::ledger::SystemClock,
    offset: i64,
}

impl k2_node::ledger::Clock for OffsetClock {
    fn now(&self) -> i64 {
        self.inner.now() + self.offset
    }
    fn boot_id(&self) -> String {
        self.inner.boot_id()
    }
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
        Some("job-user") => {
            if !is_root() {
                return Err("admin job-user writes /etc/k2-node/jobs.toml: run it with sudo".into());
            }
            let name = a.positional.get(2).ok_or("admin job-user <name> (k2job)")?;
            let node_user = acfg::read(&config_dir.join(acfg::ADMIN_FILE))
                .ok()
                .flatten()
                .map(|c| c.node_user)
                .unwrap_or_else(|| acfg::default_node_user().to_string());
            println!("{}", k2_node::admin::jobs::write_jobs_file(&config_dir, name, &node_user)?);
            Ok(())
        }
        Some("homes") => cmd_homes(a),
        Some(o) => Err(format!("admin {o:?}: use level, pin, audit, status, serve, job-user, homes")),
        None => Err("admin: use level, pin, audit, status, serve, job-user, homes".into()),
    }
}

/// `admin homes [--fix [--yes]] [--json]`: the open home folders, and the
/// owner's fix (never silent: --fix, then a yes at a terminal or --yes).
fn cmd_homes(a: &Args) -> Result<(), String> {
    let root = k2_node::wall::homes_root();
    let open = k2_node::wall::open_homes_in(root);
    if a.has("--json") && !a.has("--fix") {
        let v: Vec<_> = open.iter().map(|(p, m)| serde_json::json!({"home": p, "mode": format!("{m:o}")})).collect();
        println!("{}", serde_json::Value::Array(v));
        return Ok(());
    }
    if open.is_empty() {
        println!("Every home folder under {} is private.", root.display());
        return Ok(());
    }
    println!("Home folders under {} that other users can read or enter:", root.display());
    for (p, m) in &open {
        println!("  {} (mode {m:o})", p.display());
    }
    if !a.has("--fix") {
        println!("Jobs behind the job wall can't read them, but other local users (and jobs on an older k2-node) can.");
        println!("Fix (owner only): sudo k2-node admin homes --fix");
        return Ok(());
    }
    if !is_root() {
        return Err("--fix changes other people's folders: run it with sudo".into());
    }
    if !a.has("--yes") {
        use std::io::{BufRead, IsTerminal, Write};
        if !std::io::stdin().is_terminal() {
            return Err("not a terminal: pass --yes to make these private".into());
        }
        print!("Make them private (chmod 700: only the owner)? [y/N] ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line).map_err(|e| e.to_string())?;
        if !matches!(line.trim(), "y" | "Y" | "yes") {
            println!("Left as they are.");
            return Ok(());
        }
    }
    for (p, _) in &open {
        match k2_node::wall::make_private(root, p) {
            Ok((was, now)) => println!("  {}: {was:o} → {now:o}", p.display()),
            Err(e) => eprintln!("  {}: {e}", p.display()),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    // The Linux job unit's shim (runs as the job user): exec or fail.
    if raw.first().map(String::as_str) == Some("job-exec") {
        let Some(spec) = raw.get(1) else {
            eprintln!("k2-node job-exec <spec>");
            return ExitCode::from(2);
        };
        eprintln!("{}", k2_node::wall::job_exec(std::path::Path::new(spec)));
        return ExitCode::from(127);
    }
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
        Some("servers") => cmd_servers(&a),
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
        Some("wall-probe") => {
            let list = |k: &str| a.flags.get(k).cloned().unwrap_or_default();
            println!("{}", k2_node::wall::probe(&list("read"), &list("list"), &list("connect")));
            Ok(())
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use k2_node::ledger::Clock;

    /// The fake clock is a test node's alone: without the explicit switch
    /// (every real node) the variable changes nothing; the switch needs
    /// `--dev`; a bad value fails loudly.
    #[test]
    fn the_fake_clock_needs_dev_and_the_switch() {
        assert_eq!(fake_clock_offset(Some("+600"), false, false), Ok(None), "a real node ignores the variable");
        assert_eq!(fake_clock_offset(Some("+600"), true, false), Ok(None), "--dev alone isn't enough");
        assert!(fake_clock_offset(Some("+600"), false, true).is_err(), "the switch is for --dev nodes only");
        assert_eq!(fake_clock_offset(Some("+600"), true, true), Ok(Some(600)));
        assert_eq!(fake_clock_offset(Some("-30"), true, true), Ok(Some(-30)));
        assert_eq!(fake_clock_offset(None, true, true), Ok(None));
        assert!(fake_clock_offset(Some("ten minutes"), true, true).is_err());
        assert!(fake_clock_offset(Some("99999999999"), true, true).is_err());
        let c = OffsetClock { inner: k2_node::ledger::SystemClock::new(), offset: 600 };
        let real = k2_node::util::now();
        assert!((c.now() - real - 600).abs() <= 1, "offset applied");
        assert_eq!(c.boot_id(), k2_node::ledger::SystemClock::new().boot_id(), "same boot");
    }

    #[test]
    fn durations_and_sizes_parse() {
        assert_eq!(parse_for("7d"), Ok(Some(7 * 86400)));
        assert_eq!(parse_for("2h"), Ok(Some(7200)));
        assert_eq!(parse_for("none"), Ok(None));
        assert!(parse_for("soon").is_err());
        assert_eq!(parse_gb("8G"), Ok(8));
        let o = share_overrides(&[("weight".into(), "3".into()), ("max-running".into(), "2".into())], true, false).unwrap();
        assert_eq!((o.weight, o.max_running, o.gate), (Some(3), Some(2), Some(true)));
        assert!(share_overrides(&[("weight".into(), "x".into())], false, false).is_err());
    }
}
