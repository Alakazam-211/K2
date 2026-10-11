//! Service files for the root executor (prd-k2-node-admin-v1 §5, §5.2):
//! a systemd socket + service on Linux, a LaunchDaemon with a launchd
//! socket on macOS. Rendered here so they're golden-tested.

pub struct AdminUnitParams<'a> {
    pub binary: &'a str,
    pub home: &'a str,
    pub config: &'a str,
    pub state: &'a str,
    pub socket: &'a str,
    /// Linux: group name; macOS: numeric gid as a string.
    pub group: &'a str,
}

pub const LINUX_SOCKET_UNIT: &str = "k2-node-admin.socket";
pub const LINUX_SERVICE_UNIT: &str = "k2-node-admin.service";
pub const MACOS_LABEL: &str = "dev.k2.node-admin";

pub fn render_socket_unit(p: &AdminUnitParams<'_>) -> String {
    format!(
        "# K2 node admin: the root executor's local socket (no network door).\n\
[Unit]\n\
Description=K2 node admin executor socket\n\
\n\
[Socket]\n\
ListenStream={socket}\n\
SocketMode=0660\n\
SocketUser=root\n\
SocketGroup={group}\n\
Accept=no\n\
\n\
[Install]\n\
WantedBy=sockets.target\n",
        socket = p.socket,
        group = p.group,
    )
}

pub fn render_service_unit(p: &AdminUnitParams<'_>) -> String {
    format!(
        "# K2 node admin: the root executor. It runs only commands signed by the\n\
# pinned controller (and, for privileged ones, the owner's approval key),\n\
# checked here, as root. prd-k2-node-admin-v1.\n\
[Unit]\n\
Description=K2 node admin executor (checks signed commands itself)\n\
Requires={sock}\n\
After={sock}\n\
\n\
[Service]\n\
Type=simple\n\
ExecStart={binary} admin serve --config-dir {config} --state-dir {state} --home {home} --binary {binary}\n\
Restart=on-failure\n\
RestartSec=2\n\
UMask=0077\n\
WorkingDirectory=/\n\
\n\
[Install]\n\
Also={sock}\n",
        sock = LINUX_SOCKET_UNIT,
        binary = p.binary,
        config = p.config,
        state = p.state,
        home = p.home,
    )
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// `/Library/LaunchDaemons/dev.k2.node-admin.plist`. Socket-activated:
/// launchd owns the socket (0660 = 432, root, the node group) and starts
/// the executor on demand.
pub fn render_plist(p: &AdminUnitParams<'_>) -> String {
    let args = [p.binary, "admin", "serve", "--config-dir", p.config, "--state-dir", p.state, "--home", p.home, "--binary", p.binary];
    let mut argv = String::new();
    for a in args {
        argv.push_str(&format!("\t\t<string>{}</string>\n", xml(a)));
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- K2 node admin executor (root). Runs only signed commands it checks itself. -->
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
{argv}	</array>
	<key>UserName</key>
	<string>root</string>
	<key>Sockets</key>
	<dict>
		<key>Listener</key>
		<dict>
			<key>SockPathName</key>
			<string>{socket}</string>
			<key>SockPathMode</key>
			<integer>432</integer>
			<key>SockPathOwner</key>
			<integer>0</integer>
			<key>SockPathGroup</key>
			<integer>{group}</integer>
		</dict>
	</dict>
	<key>Umask</key>
	<integer>63</integer>
	<key>WorkingDirectory</key>
	<string>/</string>
	<key>StandardErrorPath</key>
	<string>/Library/Logs/k2-node-admin.log</string>
</dict>
</plist>
"#,
        label = MACOS_LABEL,
        argv = argv,
        socket = xml(p.socket),
        group = xml(p.group),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn golden(name: &str) -> String {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden").join(name);
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read golden {}: {e}", p.display()))
    }

    #[test]
    fn units_match_goldens() {
        let l = AdminUnitParams {
            binary: "/usr/local/libexec/k2-node",
            home: "/var/lib/k2node",
            config: "/etc/k2-node",
            state: "/var/lib/k2-node-admin",
            socket: "/run/k2-node-admin.sock",
            group: "k2node",
        };
        assert_eq!(render_socket_unit(&l), golden("linux-k2-node-admin.socket"));
        assert_eq!(render_service_unit(&l), golden("linux-k2-node-admin.service"));
        let m = AdminUnitParams {
            binary: "/Library/Application Support/K2/node/k2-node",
            home: "/var/k2node",
            config: "/etc/k2-node",
            state: "/var/db/k2-node-admin",
            socket: "/var/run/k2-node-admin.sock",
            group: "399",
        };
        assert_eq!(render_plist(&m), golden("macos-dev.k2.node-admin.plist"));
        assert!(!render_service_unit(&l).contains("User="), "the executor is root by design; jobs never are");
    }
}
