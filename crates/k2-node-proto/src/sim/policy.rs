//! What a Mac node runs for a server, checked argument by argument (review
//! IR-5). The server drives the shipped iOS lane over the stream; the node
//! never runs anything just because the server sent it.
//!
//! - **Programs:** `xcrun simctl --set <this slot's set> <verb> …` for the
//!   verbs the lane uses, `xcrun xcodebuild -version`, and `/usr/bin/plutil`
//!   reading a device-type profile or an uploaded `Info.plist`. Nothing
//!   else (`spawn <udid> sh`, `xcrun clang`… are refused).
//! - **`--set`:** the lane already puts `simctl --set <set>` first, so the
//!   first three arguments must be exactly that; the node never inserts it.
//! - **Paths:** `install` only from this slot's uploads; disk ops only in
//!   the workspace's own temp folder (or the set itself for `ensure_dir`);
//!   the helper's `image` only into that temp folder. Lexical checks: an
//!   absolute path with no `.` or `..` component, inside its root.
//! - **URLs:** `openurl` refuses `file:` (IR-4: Simulator apps run outside
//!   Seatbelt and could read the Mac's files).
//!
//! Every refusal is [`super::codes::NODE_REFUSED`].

use std::path::{Component, Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::codes;
use super::ctl::DiskOp;

/// One server slot's places on the node, reported in `Offer.sim` and
/// passed back verbatim by the server.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SlotPaths {
    /// The slot's own device set (`/var/k2sim/sets/<slot>`).
    pub set: String,
    /// The slot's temp root; a workspace gets `<tmp>/<workspace_key>`.
    pub tmp: String,
    /// The slot's unpacked uploads (`/var/k2sim/uploads/<slot>`).
    pub uploads: String,
    /// Where `.simdevicetype` bundles live (inside Xcode and
    /// `/Library/Developer/CoreSimulator/Profiles/DeviceTypes`).
    #[serde(default)]
    pub devicetype_roots: Vec<String>,
}

impl SlotPaths {
    /// The workspace's temp folder: the lane's `data_dir` on the node.
    pub fn workspace_tmp(&self, workspace_key: &str) -> String {
        format!("{}/{}", self.tmp.trim_end_matches('/'), workspace_key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub code: &'static str,
    pub message: String,
}

fn no(message: impl Into<String>) -> Refusal {
    Refusal { code: codes::NODE_REFUSED, message: message.into() }
}

/// An absolute path with only normal components (no `.`, `..`, NUL).
pub fn clean_abs(p: &str) -> bool {
    !p.is_empty()
        && !p.contains('\0')
        && Path::new(p).is_absolute()
        && Path::new(p).components().all(|c| matches!(c, Component::RootDir | Component::Normal(_)))
}

/// `p` is `root` or inside it (component-wise, after [`clean_abs`]).
pub fn within(p: &str, root: &str) -> bool {
    clean_abs(p) && clean_abs(root) && Path::new(p).starts_with(Path::new(root))
}

/// Strictly inside `root` (not `root` itself).
pub fn inside(p: &str, root: &str) -> bool {
    within(p, root) && Path::new(p) != Path::new(root)
}

/// Bundle ids and executables (letters, digits, `.`, `-`, `_`).
pub fn bundle_id_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 255 && !s.starts_with('-') && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
}

/// `openurl` targets: no control characters, at most 2048, never `file:`.
pub fn check_open_url(url: &str) -> Result<(), Refusal> {
    if url.len() > 2048 || url.chars().any(char::is_control) {
        return Err(no("open-url takes one URL of at most 2048 characters"));
    }
    let scheme = url.trim().split(':').next().unwrap_or("").trim().to_ascii_lowercase();
    if scheme == "file" {
        return Err(no("file: URLs aren't opened on a Mac node: the iPhone runs outside the job wall and could show the Mac's own files. Open an http(s) URL or an app link."));
    }
    if !url.contains(':') {
        return Err(no("open-url needs a URL or deep link (scheme:…)"));
    }
    Ok(())
}

const SIMCTL_UDID_VERBS: &[&str] = &["boot", "shutdown", "erase", "delete"];

/// May the node run `program args` for this slot and workspace?
pub fn check_run(paths: &SlotPaths, workspace_key: &str, program: &str, args: &[String]) -> Result<(), Refusal> {
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match program {
        "xcrun" => match a.as_slice() {
            ["xcodebuild", "-version"] => Ok(()),
            ["simctl", "--set", set, rest @ ..] => {
                if *set != paths.set {
                    return Err(no("simctl may only use this server's own device set"));
                }
                check_simctl(paths, workspace_key, rest)
            }
            ["simctl", ..] => Err(no("simctl must name this server's device set first (simctl --set <set> …)")),
            _ => Err(no(format!("xcrun {} isn't something a Mac node runs for a server", a.first().copied().unwrap_or("")))),
        },
        "/usr/bin/plutil" => match a.as_slice() {
            ["-convert", "json", "-o", "-", p] => {
                if paths.devicetype_roots.iter().any(|r| inside(p, r)) && p.ends_with(".simdevicetype/Contents/Resources/profile.plist") {
                    Ok(())
                } else {
                    Err(no("plutil reads only a Simulator device type's profile.plist"))
                }
            }
            ["-extract", key, "raw", "-o", "-", p] => {
                if !matches!(*key, "CFBundleIdentifier" | "CFBundleShortVersionString" | "CFBundleExecutable") {
                    return Err(no("plutil reads only an app's id, version and executable"));
                }
                if inside(p, &paths.uploads) && p.ends_with("/Info.plist") {
                    Ok(())
                } else {
                    Err(no("plutil reads only an uploaded app's Info.plist"))
                }
            }
            _ => Err(no("plutil: not a read this node allows")),
        },
        other => Err(no(format!("{other} isn't something a Mac node runs for a server"))),
    }
}

fn check_simctl(paths: &SlotPaths, workspace_key: &str, rest: &[&str]) -> Result<(), Refusal> {
    let udid = |u: &str| if super::valid_udid(u) { Ok(()) } else { Err(no("not a Simulator device id")) };
    match rest {
        ["list", "-j", "devices"] | ["list", "-j", "runtimes"] => Ok(()),
        ["create", name, dtype, runtime] => {
            let prefix = format!("K2-{workspace_key}--");
            if !name.starts_with(&prefix) || name.len() > 128 || !name[prefix.len()..].chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                return Err(no("a device's name must be K2-<this workspace>--<model>"));
            }
            if !bundle_id_ok(dtype) || !bundle_id_ok(runtime) {
                return Err(no("not a device type and runtime"));
            }
            Ok(())
        }
        [verb, u] if SIMCTL_UDID_VERBS.contains(verb) => udid(u),
        ["bootstatus", u, "-b"] => udid(u),
        [verb, u, bundle] if matches!(*verb, "get_app_container" | "launch" | "terminate") => {
            udid(u)?;
            if bundle_id_ok(bundle) {
                Ok(())
            } else {
                Err(no("not a bundle id"))
            }
        }
        ["install", u, p] => {
            udid(u)?;
            if inside(p, &paths.uploads) && p.ends_with(".app") {
                Ok(())
            } else {
                Err(no("install takes only an app uploaded to this server's folder on the node"))
            }
        }
        ["openurl", u, url] => {
            udid(u)?;
            check_open_url(url)
        }
        ["location", u, "set", at] => {
            udid(u)?;
            let ok = at.split_once(',').is_some_and(|(x, y)| x.parse::<f64>().is_ok() && y.parse::<f64>().is_ok());
            if ok {
                Ok(())
            } else {
                Err(no("location takes lat,lon"))
            }
        }
        ["ui", u, "appearance", "light" | "dark"] => udid(u),
        ["spawn", u, "notifyutil", "-p", name] => {
            udid(u)?;
            if name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
                Ok(())
            } else {
                Err(no("not a notification name"))
            }
        }
        ["spawn", u, "log", "show", "--last", last, "--style", "compact", tail @ ..] => {
            udid(u)?;
            if !last.strip_suffix('s').is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                return Err(no("log show --last takes <n>s"));
            }
            match tail {
                [] | ["--predicate", _] => Ok(()),
                _ => Err(no("log show takes only --predicate")),
            }
        }
        ["spawn", ..] => Err(no("simctl spawn runs only notifyutil and log show on a Mac node")),
        [verb, ..] => Err(no(format!("simctl {verb} isn't something a Mac node runs for a server"))),
        [] => Err(no("simctl needs a verb")),
    }
}

/// May the server do `op` on `path`?
pub fn check_disk(paths: &SlotPaths, workspace_key: &str, op: DiskOp, path: &str) -> Result<(), Refusal> {
    let ws = paths.workspace_tmp(workspace_key);
    let ok = match op {
        DiskOp::EnsureDir => path == paths.set || within(path, &ws),
        DiskOp::IsDir | DiskOp::IsFile => within(path, &ws) || inside(path, &paths.uploads),
        DiskOp::Read | DiskOp::RemoveFile => inside(path, &ws),
        DiskOp::RemoveDirAll => path == ws,
    };
    if ok && super::valid_workspace_key(workspace_key) {
        Ok(())
    } else {
        Err(no(format!("{} is outside this workspace's folder on the node", op.as_str())))
    }
}

/// The helper ops the lane sends.
pub const HELPER_OPS: &[&str] = &["hello", "focus", "touch", "tree", "value", "set_value", "tap", "swipe", "type", "key", "button", "frames", "image"];

/// May this request line go to the helper? It is parsed, never passed
/// through as text (review IR-5): `image` writes only into the
/// workspace's temp folder.
pub fn check_helper_line(paths: &SlotPaths, workspace_key: &str, line: &[u8]) -> Result<Value, Refusal> {
    let v: Value = serde_json::from_slice(line).map_err(|_| no("a helper request must be one JSON object"))?;
    let op = v.get("op").and_then(Value::as_str).ok_or_else(|| no("a helper request needs an op"))?;
    if !v.is_object() || !HELPER_OPS.contains(&op) {
        return Err(no(format!("helper op {op:?} isn't allowed from a server")));
    }
    if op == "image" {
        let p = v.get("path").and_then(Value::as_str).unwrap_or("");
        if !(inside(p, &paths.workspace_tmp(workspace_key)) && p.ends_with(".png")) {
            return Err(no("the helper writes screenshots only into this workspace's folder on the node"));
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEY: &str = "0123456789abcdef01234567";
    const UDID: &str = "AD6DB1AF-E89F-48D1-A6D7-DDEF9C68B1EB";

    fn paths() -> SlotPaths {
        SlotPaths {
            set: "/var/k2sim/sets/0".into(),
            tmp: "/var/k2sim/tmp/0".into(),
            uploads: "/var/k2sim/uploads/0".into(),
            devicetype_roots: vec!["/Library/Developer/CoreSimulator/Profiles/DeviceTypes".into()],
        }
    }

    fn simctl(rest: &[&str]) -> Vec<String> {
        let mut v = vec!["simctl".to_string(), "--set".into(), "/var/k2sim/sets/0".into()];
        v.extend(rest.iter().map(|s| s.to_string()));
        v
    }

    fn run(args: Vec<String>) -> Result<(), Refusal> {
        check_run(&paths(), KEY, "xcrun", &args)
    }

    #[test]
    fn every_simctl_call_the_lane_makes_passes() {
        let name = format!("K2-{KEY}--iPhone-17");
        let ok: Vec<Vec<&str>> = vec![
            vec!["list", "-j", "devices"],
            vec!["list", "-j", "runtimes"],
            vec!["create", &name, "com.apple.CoreSimulator.SimDeviceType.iPhone-17", "com.apple.CoreSimulator.SimRuntime.iOS-26-5"],
            vec!["boot", UDID],
            vec!["bootstatus", UDID, "-b"],
            vec!["get_app_container", UDID, "dev.k2.simfixture"],
            vec!["spawn", UDID, "notifyutil", "-p", "com.apple.UIKit.SimulatorShake"],
            vec!["shutdown", UDID],
            vec!["erase", UDID],
            vec!["delete", UDID],
            vec!["install", UDID, "/var/k2sim/uploads/0/abc/K2Fixture.app"],
            vec!["launch", UDID, "dev.k2.simfixture"],
            vec!["terminate", UDID, "dev.k2.simfixture"],
            vec!["openurl", UDID, "https://web.app.k2.dev"],
            vec!["openurl", UDID, "myapp://deep/link"],
            vec!["location", UDID, "set", "1.5,-2.25"],
            vec!["ui", UDID, "appearance", "dark"],
            vec!["spawn", UDID, "log", "show", "--last", "120s", "--style", "compact"],
            vec!["spawn", UDID, "log", "show", "--last", "120s", "--style", "compact", "--predicate", "process == \"A\""],
        ];
        for rest in ok {
            run(simctl(&rest)).unwrap_or_else(|e| panic!("{rest:?}: {}", e.message));
        }
        assert!(check_run(&paths(), KEY, "xcrun", &["xcodebuild".into(), "-version".into()]).is_ok());
    }

    #[test]
    fn anything_else_is_refused() {
        let bad: Vec<(&str, Vec<String>)> = vec![
            ("xcrun", simctl(&["spawn", UDID, "sh", "-c", "id"])),
            ("xcrun", simctl(&["spawn", UDID, "log", "stream"])),
            ("xcrun", vec!["simctl".into(), "--set".into(), "/var/k2sim/sets/1".into(), "list".into(), "-j".into(), "devices".into()]),
            ("xcrun", vec!["simctl".into(), "list".into()]),
            ("xcrun", vec!["clang".into(), "-o".into(), "/tmp/x".into()]),
            ("xcrun", simctl(&["install", UDID, "/var/k2node/state/key.pem"])),
            ("xcrun", simctl(&["install", UDID, "/var/k2sim/uploads/0/../../k2node/x.app"])),
            ("xcrun", simctl(&["install", UDID, "/var/k2sim/uploads/1/abc/A.app"])),
            ("xcrun", simctl(&["openurl", UDID, "file:///var/k2node/"])),
            ("xcrun", simctl(&["openurl", UDID, " FILE:///Users/"])),
            ("xcrun", simctl(&["boot", "booted"])),
            ("xcrun", simctl(&["create", "K2-ffffffffffffffffffffffff--iPhone-17", "t", "r"])),
            ("xcrun", simctl(&["create", "Mine", "t", "r"])),
            ("xcrun", simctl(&["io", UDID, "screenshot", "/tmp/x.png"])),
            ("xcrun", simctl(&["launch", UDID, "a;rm"])),
            ("/usr/bin/plutil", vec!["-convert".into(), "json".into(), "-o".into(), "-".into(), "/etc/k2-node/policy.toml".into()]),
            ("/usr/bin/plutil", vec!["-extract".into(), "CFBundleIdentifier".into(), "raw".into(), "-o".into(), "-".into(), "/Users/patlee/Info.plist".into()]),
            ("/usr/bin/plutil", vec!["-extract".into(), "NSPrivate".into(), "raw".into(), "-o".into(), "-".into(), "/var/k2sim/uploads/0/a/A.app/Info.plist".into()]),
            ("/bin/sh", vec!["-c".into(), "id".into()]),
        ];
        for (prog, args) in bad {
            let e = check_run(&paths(), KEY, prog, &args).expect_err(&format!("{prog} {args:?} must be refused"));
            assert_eq!(e.code, codes::NODE_REFUSED);
        }
        // The reads the lane needs pass.
        let dt = "/Library/Developer/CoreSimulator/Profiles/DeviceTypes/iPhone 17.simdevicetype/Contents/Resources/profile.plist";
        assert!(check_run(&paths(), KEY, "/usr/bin/plutil", &["-convert".into(), "json".into(), "-o".into(), "-".into(), dt.into()]).is_ok());
        let ip = "/var/k2sim/uploads/0/abc/A.app/Info.plist";
        assert!(check_run(&paths(), KEY, "/usr/bin/plutil", &["-extract".into(), "CFBundleIdentifier".into(), "raw".into(), "-o".into(), "-".into(), ip.into()]).is_ok());
    }

    #[test]
    fn disk_ops_stay_in_the_workspace_folder() {
        let p = paths();
        let ws = p.workspace_tmp(KEY);
        assert!(check_disk(&p, KEY, DiskOp::EnsureDir, "/var/k2sim/sets/0").is_ok());
        assert!(check_disk(&p, KEY, DiskOp::EnsureDir, &ws).is_ok());
        assert!(check_disk(&p, KEY, DiskOp::Read, &format!("{ws}/.shot-1-2.png")).is_ok());
        assert!(check_disk(&p, KEY, DiskOp::RemoveDirAll, &ws).is_ok());
        assert!(check_disk(&p, KEY, DiskOp::IsDir, "/var/k2sim/uploads/0/abc/A.app").is_ok());
        for (op, path) in [
            (DiskOp::Read, "/var/k2node/state/node-key.pem".to_string()),
            (DiskOp::Read, format!("{ws}/../ffffffffffffffffffffffff/x")),
            (DiskOp::Read, "/var/k2sim/tmp/0/ffffffffffffffffffffffff/x.png".to_string()),
            (DiskOp::RemoveDirAll, "/var/k2sim".to_string()),
            (DiskOp::RemoveDirAll, "/var/k2sim/tmp/0".to_string()),
            (DiskOp::EnsureDir, "/var/k2sim/sets/1".to_string()),
            (DiskOp::RemoveFile, ws.clone()),
        ] {
            assert_eq!(check_disk(&p, KEY, op, &path).unwrap_err().code, codes::NODE_REFUSED, "{op:?} {path}");
        }
    }

    #[test]
    fn helper_requests_are_parsed_and_image_paths_policed() {
        let p = paths();
        let ws = p.workspace_tmp(KEY);
        assert!(check_helper_line(&p, KEY, br#"{"id":1,"op":"tap","x":1,"y":2}"#).is_ok());
        assert!(check_helper_line(&p, KEY, format!(r#"{{"id":2,"op":"image","path":"{ws}/.shot-1.png"}}"#).as_bytes()).is_ok());
        for bad in [
            r#"{"id":2,"op":"image","path":"/var/k2node/state/x.png"}"#.to_string(),
            format!(r#"{{"id":2,"op":"image","path":"{ws}/../../../k2node/x.png"}}"#),
            format!(r#"{{"id":2,"op":"image","path":"{ws}/a.pem"}}"#),
            r#"{"id":3,"op":"quit"}"#.to_string(),
            r#"{"id":3}"#.to_string(),
            "not json".to_string(),
        ] {
            assert_eq!(check_helper_line(&p, KEY, bad.as_bytes()).unwrap_err().code, codes::NODE_REFUSED, "{bad}");
        }
    }

    #[test]
    fn file_urls_are_refused_and_web_and_app_links_pass() {
        for ok in ["https://web.app.k2.dev", "http://example.com/x", "myapp://open?x=1"] {
            assert!(check_open_url(ok).is_ok(), "{ok}");
        }
        for bad in ["file:///etc/passwd", "File:/Users/patlee", "  file:x", "nocolon", "https://x\n"] {
            assert!(check_open_url(bad).is_err(), "{bad}");
        }
    }
}
