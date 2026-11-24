//! `k2-node simhost` (the service's own entry) and `k2-node sim …` (the
//! Mac owner's switch and status), §3.3 and §3.8.

use std::path::{Path, PathBuf};
use std::time::Duration;

use super::local::{self, Hello};
use super::{switch, SimRoot, NODE_USER, SIMHOST_LABEL, SIM_ROOT, SIM_USER};
use k2_brand::PRODUCT_NAME;
use k2_brand::product_name;

pub const USAGE: &str = concat!("\
iPhones on this Mac (the iOS Simulator for your own ", product_name!(), " servers):
  sudo k2-node setup --sim [--server <slot|label>] [--private-home]
                                       create the Simulator user and service; turn iPhones on (default: the owner server)
  sudo k2-node sim enable --server <slot|label>    turn iPhones on for one of your own servers
  sudo k2-node sim disable --server <slot|label>   turn them off (a running iPhone ends when its lease does)
  k2-node sim status [--json]                      what's set up, Xcode and its runtimes, what's missing");

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The LaunchDaemon `dev.k2.simhost`: `k2-node simhost` as the Simulator
/// user, in the system domain (the shape the node itself runs in).
pub fn render_plist(binary: &str, root: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- {PRODUCT_NAME} compute node: the iOS Simulator service. Installed by k2-node setup --sim. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{binary}</string>
		<string>simhost</string>
		<string>--root</string>
		<string>{root}</string>
		<string>--peer-user</string>
		<string>{peer}</string>
	</array>
	<key>UserName</key>
	<string>{user}</string>
	<key>GroupName</key>
	<string>{user}</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>Umask</key>
	<integer>63</integer>
	<key>WorkingDirectory</key>
	<string>{root}</string>
	<key>EnvironmentVariables</key>
	<dict>
		<key>HOME</key>
		<string>{root}</string>
		<key>TMPDIR</key>
		<string>{root}/cache/tmp</string>
		<key>PATH</key>
		<string>/usr/bin:/bin:/usr/sbin:/sbin</string>
	</dict>
	<key>StandardOutPath</key>
	<string>{root}/log/simhost.log</string>
	<key>StandardErrorPath</key>
	<string>{root}/log/simhost.log</string>
</dict>
</plist>
"#,
        label = SIMHOST_LABEL,
        binary = xml(binary),
        root = xml(root),
        peer = NODE_USER,
        user = SIM_USER,
    )
}

fn is_root() -> bool {
    unsafe { libc::geteuid() == 0 }
}

/// `k2-node sim …` and `k2-node simhost`. `get` reads a `--flag value`.
pub fn run(positional: &[String], get: &dyn Fn(&str) -> Option<String>, json: bool, config_dir: &Path) -> Result<(), String> {
    if positional.first().map(String::as_str) == Some("simhost") {
        let root = get("root").unwrap_or_else(|| SIM_ROOT.into());
        let peer = get("peer-user").unwrap_or_else(|| NODE_USER.into());
        if !super::plain_abs(Path::new(&root)) {
            return Err("--root must be an absolute path".into());
        }
        return super::host::main(Path::new(&root), &peer);
    }
    match positional.get(1).map(String::as_str) {
        Some(verb @ ("enable" | "disable")) => {
            let server = get("server").ok_or("--server <slot|label> is required (k2-node servers list shows them)")?;
            if !is_root() {
                return Err(format!("run as root: sudo k2-node sim {verb} --server {server}"));
            }
            println!("{}", switch::set(config_dir, cfg!(target_os = "macos"), &server, verb == "enable")?);
            Ok(())
        }
        Some("install-files") => {
            let binary = get("binary").ok_or("--binary is required")?;
            let out = PathBuf::from(get("out").ok_or("--out is required")?);
            let root = get("root").unwrap_or_else(|| SIM_ROOT.into());
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            let p = out.join(format!("{SIMHOST_LABEL}.plist"));
            std::fs::write(&p, render_plist(&binary, &root)).map_err(|e| format!("{}: {e}", p.display()))?;
            println!("{}", p.display());
            Ok(())
        }
        Some("status") | None => {
            let sw = switch::read(config_dir);
            let sock = SimRoot::new(get("root").unwrap_or_else(|| SIM_ROOT.into())).socket();
            let facts = local::call(&sock, &Hello::Facts, Duration::from_secs(600));
            if json {
                let v = serde_json::json!({
                    "slots": sw.as_ref().map(|s| s.slots.clone()).unwrap_or_default(),
                    "switch_error": sw.as_ref().err(),
                    "service": facts.as_ref().map(|_| "running").unwrap_or("not reachable"),
                    "facts": facts.as_ref().ok().and_then(|a| a.facts.clone()),
                    "error": facts.as_ref().err(),
                });
                println!("{v}");
                return Ok(());
            }
            match &sw {
                Ok(s) if s.slots.is_empty() => println!("iPhones: off for every server (sudo k2-node sim enable --server 0)"),
                Ok(s) => println!("iPhones: on for server slot(s) {:?}", s.slots),
                Err(e) => println!("iPhones: off ({e})"),
            }
            match facts {
                Ok(a) => {
                    let f = a.facts.unwrap_or_default();
                    println!("service: running ({SIMHOST_LABEL}, as {SIM_USER})");
                    println!("Xcode: {}", f.xcode.as_deref().unwrap_or("not found"));
                    for r in &f.runtimes {
                        let names: Vec<&str> = r.device_types.iter().map(|t| t.name.as_str()).collect();
                        println!("runtime: {} ({})", r.name, names.join(", "));
                    }
                    println!("helper: {}", if f.helper_built { "built" } else { "not built" });
                    println!("disk: {} MB under {SIM_ROOT}", f.bytes >> 20);
                    if f.ready() {
                        println!("ready: yes");
                    }
                    for n in &f.needs {
                        println!("still needed: {}: {}", n.what, n.command);
                    }
                }
                Err(e) => println!("service: {e}\nstill needed: sudo k2-node setup --sim"),
            }
            Ok(())
        }
        Some(other) => Err(format!("unknown: k2-node sim {other}\n{USAGE}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_service_runs_simhost_as_the_simulator_user() {
        let p = render_plist("/Library/Application Support/K2/node/k2-node", "/var/k2sim");
        for want in [
            "<string>dev.k2.simhost</string>",
            "<string>simhost</string>",
            "<key>UserName</key>\n\t<string>_k2sim</string>",
            "<string>--peer-user</string>\n\t\t<string>_k2node</string>",
            "<key>HOME</key>\n\t\t<string>/var/k2sim</string>",
            "<string>/var/k2sim/log/simhost.log</string>",
        ] {
            assert!(p.contains(want), "missing {want:?} in\n{p}");
        }
        assert!(render_plist("/a&b", "/r").contains("/a&amp;b"));
    }

    #[test]
    fn enable_needs_root_and_a_server() {
        let d = crate::util::temp_dir("simcli");
        let get = |k: &str| (k == "server").then(|| "0".to_string());
        let none = |_: &str| None;
        let pos = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(run(&pos(&["sim", "enable"]), &none, false, &d).unwrap_err().contains("--server"));
        if !is_root() {
            assert!(run(&pos(&["sim", "enable"]), &get, false, &d).unwrap_err().contains("sudo k2-node sim enable --server 0"));
        }
        assert!(run(&pos(&["sim", "bogus"]), &none, false, &d).unwrap_err().contains("unknown"));
        let out = d.join("out");
        let files = |k: &str| match k {
            "binary" => Some("/x/k2-node".to_string()),
            "out" => Some(out.to_string_lossy().into_owned()),
            _ => None,
        };
        run(&pos(&["sim", "install-files"]), &files, false, &d).unwrap();
        assert!(std::fs::read_to_string(out.join("dev.k2.simhost.plist")).unwrap().contains("/x/k2-node"));
    }
}
