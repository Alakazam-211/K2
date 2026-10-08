//! Service files the installers drop in place (§8.3, CN24, CN27).
//! Rendered here (not in shell) so they are golden-tested.

use std::path::Path;

pub struct InstallParams<'a> {
    pub binary: &'a str,
    pub home: &'a str,
    pub config: &'a str,
    pub user: &'a str,
    pub group: &'a str,
}

/// systemd system unit `k2-node.service`.
pub fn render_unit(p: &InstallParams<'_>) -> String {
    format!(
        "# K2 compute node. Installed by scripts/node/install-node-linux.sh.\n\
[Unit]\n\
Description=K2 compute node (runs jobs from your K2 server as {user})\n\
After=network-online.target\n\
Wants=network-online.target\n\
\n\
[Service]\n\
Type=simple\n\
User={user}\n\
Group={group}\n\
ExecStart={binary} run --home {home} --config-dir {config}\n\
Restart=always\n\
RestartSec=5\n\
# cgroup v2 delegation: k2-node moves itself to supervisor/ and gives each\n\
# job its own leaf with cpu.max / memory.max / pids.max (CN24).\n\
Delegate=yes\n\
KillMode=control-group\n\
NoNewPrivileges=yes\n\
ProtectSystem=full\n\
ProtectHome=read-only\n\
ReadWritePaths={home} /var/tmp /tmp\n\
WorkingDirectory={home}\n\
LimitNOFILE=65536\n\
\n\
[Install]\n\
WantedBy=multi-user.target\n",
        binary = p.binary,
        home = p.home,
        config = p.config,
        user = p.user,
        group = p.group,
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// launchd LaunchDaemon `dev.k2.node.plist`.
pub fn render_plist(p: &InstallParams<'_>) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- K2 compute node. Installed by scripts/node/install-node-macos.sh. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>dev.k2.node</string>
	<key>ProgramArguments</key>
	<array>
		<string>{binary}</string>
		<string>run</string>
		<string>--home</string>
		<string>{home}</string>
		<string>--config-dir</string>
		<string>{config}</string>
	</array>
	<key>UserName</key>
	<string>{user}</string>
	<key>GroupName</key>
	<string>{group}</string>
	<key>RunAtLoad</key>
	<true/>
	<key>KeepAlive</key>
	<true/>
	<key>WorkingDirectory</key>
	<string>{home}</string>
	<key>StandardOutPath</key>
	<string>{home}/log/k2-node.log</string>
	<key>StandardErrorPath</key>
	<string>{home}/log/k2-node.log</string>
	<key>SoftResourceLimits</key>
	<dict>
		<key>NumberOfFiles</key>
		<integer>65536</integer>
	</dict>
</dict>
</plist>
"#,
        binary = xml(p.binary),
        home = xml(p.home),
        config = xml(p.config),
        user = xml(p.user),
        group = xml(p.group),
    )
}

/// Write the service file plus default `policy.toml` / `control.toml`
/// into `out` (the installer copies them, keeping existing config).
pub fn write_files(os: &str, p: &InstallParams<'_>, out: &Path) -> Result<Vec<String>, String> {
    std::fs::create_dir_all(out).map_err(|e| format!("create {}: {e}", out.display()))?;
    let (name, body) = match os {
        "linux" => ("k2-node.service", render_unit(p)),
        "macos" => ("dev.k2.node.plist", render_plist(p)),
        other => return Err(format!("--os {other:?}: use linux or macos")),
    };
    let mut written = Vec::new();
    for (n, b) in [
        (name, body),
        ("policy.toml", crate::config::default_policy_toml()),
        ("control.toml", "# The machine owner's pause switch. k2-node pause|drain|resume edits it.\nstate = \"active\"\n".to_string()),
    ] {
        std::fs::write(out.join(n), b).map_err(|e| format!("write {n}: {e}"))?;
        written.push(n.to_string());
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params<'a>(os: &str) -> InstallParams<'a> {
        if os == "linux" {
            InstallParams { binary: "/usr/local/libexec/k2-node", home: "/var/lib/k2node", config: "/etc/k2-node", user: "k2node", group: "k2node" }
        } else {
            InstallParams {
                binary: "/Library/Application Support/K2/node/k2-node",
                home: "/var/k2node",
                config: "/etc/k2-node",
                user: "_k2node",
                group: "_k2node",
            }
        }
    }

    fn golden(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read golden {}: {e}", p.display()))
    }

    #[test]
    fn rendered_files_match_goldens() {
        for os in ["linux", "macos"] {
            let d = crate::util::temp_dir(&format!("install-{os}"));
            let written = write_files(os, &params(os), &d).unwrap();
            for n in written {
                let got = std::fs::read_to_string(d.join(&n)).unwrap();
                let want = golden(&format!("{os}-{n}"));
                assert_eq!(got, want, "{os}/{n} differs from tests/golden/{os}-{n}");
            }
        }
        assert!(write_files("windows", &params("linux"), &crate::util::temp_dir("install-win")).is_err());
    }

    #[test]
    fn plist_escapes_xml() {
        let p = InstallParams { binary: "/a&b/<k2>", home: "/h", config: "/c", user: "u", group: "g" };
        let s = render_plist(&p);
        assert!(s.contains("<string>/a&amp;b/&lt;k2&gt;</string>"));
    }
}
