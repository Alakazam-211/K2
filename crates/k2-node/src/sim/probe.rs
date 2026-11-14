//! What simhost finds on the Mac (IN-A7): Xcode from `xcodebuild -version`
//! (not the Command Line Tools, which have no Simulator), the iOS runtimes
//! with their iPhones and screen sizes, the owner steps still missing, and
//! the Simulator user's memory (IR-8). Parsers are pure; [`probe`] runs the
//! tools through an [`Exec`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use k2_node_proto::sim::{SimDeviceType, SimNeed, SimRuntime};
use serde_json::Value as J;

use super::exec::Exec;
use super::local::HostFacts;

/// The owner's steps, exact commands (§3.3; the Xcode licence is the
/// Mac owner's to accept, never an agent's).
pub const NEED_XCODE: (&str, &str) = (
    "install Xcode (App Store or developer.apple.com), point the tools at it and accept its licence",
    "sudo xcode-select -s /Applications/Xcode.app && sudo xcodebuild -license accept && sudo xcodebuild -runFirstLaunch",
);
pub const NEED_RUNTIME: (&str, &str) = ("install an iOS Simulator runtime", "xcodebuild -downloadPlatform iOS");

fn need(n: (&str, &str)) -> SimNeed {
    SimNeed { what: n.0.into(), command: n.1.into() }
}

/// `Xcode 26.6\nBuild version 17F113\n` → `26.6 (17F113)`.
pub fn xcode_version(stdout: &str) -> Option<String> {
    let mut v = None;
    let mut b = None;
    for l in stdout.lines() {
        if let Some(x) = l.trim().strip_prefix("Xcode ") {
            v = Some(x.trim().to_string());
        }
        if let Some(x) = l.trim().strip_prefix("Build version ") {
            b = Some(x.trim().to_string());
        }
    }
    match (v, b) {
        (Some(v), Some(b)) => Some(format!("{v} ({b})")),
        (Some(v), None) => Some(v),
        _ => None,
    }
}

/// One device type as `simctl list -j runtimes` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedType {
    pub identifier: String,
    pub name: String,
    pub bundle_path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedRuntime {
    pub identifier: String,
    pub name: String,
    pub version: String,
    /// iPhones only (the lane is portrait iPhone only).
    pub iphones: Vec<ListedType>,
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

/// Available iOS runtimes, newest first, with their iPhones.
pub fn runtimes(json: &str) -> Result<Vec<ListedRuntime>, String> {
    let v: J = serde_json::from_str(json).map_err(|e| format!("simctl list runtimes: {e}"))?;
    let list = v.get("runtimes").and_then(J::as_array).ok_or("simctl list runtimes: no runtimes array")?;
    let s = |o: &J, k: &str| o.get(k).and_then(J::as_str).unwrap_or_default().to_string();
    let mut out: Vec<ListedRuntime> = list
        .iter()
        .filter(|r| r.get("platform").and_then(J::as_str) == Some("iOS") && r.get("isAvailable").and_then(J::as_bool) == Some(true))
        .map(|r| ListedRuntime {
            identifier: s(r, "identifier"),
            name: s(r, "name"),
            version: s(r, "version"),
            iphones: r
                .get("supportedDeviceTypes")
                .and_then(J::as_array)
                .into_iter()
                .flatten()
                .filter(|t| t.get("productFamily").and_then(J::as_str) == Some("iPhone"))
                .map(|t| ListedType { identifier: s(t, "identifier"), name: s(t, "name"), bundle_path: s(t, "bundlePath") })
                .collect(),
        })
        .collect();
    out.sort_by(|a, b| version_key(&b.version).cmp(&version_key(&a.version)));
    Ok(out)
}

/// `profile.plist` as JSON → (points wide, points high, scale × 1000).
pub fn screen(profile_json: &str) -> Option<(u32, u32, u32)> {
    let v: J = serde_json::from_str(profile_json).ok()?;
    let scale = v.get("mainScreenScale")?.as_f64()?;
    if scale <= 0.0 {
        return None;
    }
    let w = v.get("mainScreenWidth")?.as_f64()? / scale;
    let h = v.get("mainScreenHeight")?.as_f64()? / scale;
    Some((w.round() as u32, h.round() as u32, (scale * 1000.0).round() as u32))
}

/// The folders device type bundles live in: `plutil` may read profiles
/// only under these (the policy's `devicetype_roots`).
pub fn devicetype_roots(rts: &[ListedRuntime]) -> Vec<String> {
    let mut out: Vec<String> = vec!["/Library/Developer/CoreSimulator/Profiles/DeviceTypes".into()];
    for t in rts.iter().flat_map(|r| &r.iphones) {
        if let Some(parent) = Path::new(&t.bundle_path).parent() {
            let p = parent.to_string_lossy().into_owned();
            if t.bundle_path.ends_with(".simdevicetype") && k2_node_proto::sim::policy::clean_abs(&p) && !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// The plainest newest iPhone ("iPhone 17" over "iPhone 17 Pro"), the
/// same rule as the local lane.
pub fn default_index(types: &[SimDeviceType]) -> Option<usize> {
    let plain = types.iter().enumerate().filter_map(|(i, t)| t.name.strip_prefix("iPhone ").and_then(|n| n.parse::<u32>().ok()).map(|n| (n, i))).max_by_key(|(n, _)| *n).map(|(_, i)| i);
    plain.or(if types.is_empty() { None } else { Some(0) })
}

/// Listed runtimes + screen sizes → the offer's runtimes. A type whose
/// profile can't be read is left out (the lane couldn't size it).
pub fn offer_runtimes(rts: &[ListedRuntime], screen_of: impl Fn(&str) -> Option<(u32, u32, u32)>) -> Vec<SimRuntime> {
    rts.iter()
        .map(|r| {
            let mut types: Vec<SimDeviceType> = r
                .iphones
                .iter()
                .filter_map(|t| {
                    let (w, h, s) = screen_of(&t.bundle_path)?;
                    Some(SimDeviceType { identifier: t.identifier.clone(), name: t.name.clone(), width_pt: w, height_pt: h, scale_milli: s, default: false })
                })
                .collect();
            if let Some(i) = default_index(&types) {
                types[i].default = true;
            }
            SimRuntime { identifier: r.identifier.clone(), name: r.name.clone(), version: r.version.clone(), device_types: types }
        })
        .collect()
}

/// The steps still missing, in order.
pub fn needs(xcode: Option<&str>, runtimes: &[SimRuntime], helper_error: Option<&str>) -> Vec<SimNeed> {
    if xcode.is_none() {
        return vec![need(NEED_XCODE)];
    }
    let mut out = Vec::new();
    if !runtimes.iter().any(|r| !r.device_types.is_empty()) {
        out.push(need(NEED_RUNTIME));
    }
    if let Some(e) = helper_error {
        out.push(SimNeed {
            what: format!("the Simulator helper doesn't build with this Xcode ({e}); open Xcode once so it finishes installing"),
            command: "sudo xcodebuild -runFirstLaunch".into(),
        });
    }
    out
}

/// `ps -axo uid=,rss=` → the RSS of `uid`'s processes, in bytes.
pub fn rss_of(ps: &str, uid: u32) -> u64 {
    ps.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let u = it.next()?.parse::<u32>().ok()?;
            let kb = it.next()?.parse::<u64>().ok()?;
            (u == uid).then_some(kb * 1024)
        })
        .sum()
}

/// Bytes under `p` (never following links).
pub fn du(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if !m.is_dir() {
        return m.len();
    }
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| du(&e.path())).sum()).unwrap_or(0)
}

/// Probe the Mac. `build_helper` compiles (or finds) the helper for an
/// Xcode and returns why it couldn't.
pub fn probe(exec: &dyn Exec, root: &Path, build_helper: impl Fn(&str) -> Result<PathBuf, String>) -> HostFacts {
    let t = Duration::from_secs(60);
    let xcode_out = exec.run("xcrun", &["xcodebuild".into(), "-version".into()], t).ok().filter(|o| o.status == 0);
    let xcode = xcode_out.as_ref().and_then(|o| xcode_version(&o.stdout));
    let xcode_path = exec.run("/usr/bin/xcode-select", &["-p".into()], t).ok().filter(|o| o.status == 0).map(|o| o.stdout.trim().trim_end_matches("/Contents/Developer").to_string());
    let mut facts = HostFacts { xcode: xcode.clone(), xcode_path, bytes: du(root), ..Default::default() };
    let Some(xv) = xcode else {
        facts.needs = needs(None, &[], None);
        return facts;
    };
    let listed = exec
        .run("xcrun", &["simctl".into(), "list".into(), "-j".into(), "runtimes".into()], t)
        .ok()
        .filter(|o| o.status == 0)
        .and_then(|o| runtimes(&o.stdout).ok())
        .unwrap_or_default();
    facts.devicetype_roots = devicetype_roots(&listed);
    facts.runtimes = offer_runtimes(&listed, |bundle| {
        let plist = format!("{bundle}/Contents/Resources/profile.plist");
        let o = exec.run("/usr/bin/plutil", &["-convert".into(), "json".into(), "-o".into(), "-".into(), plist], Duration::from_secs(10)).ok()?;
        (o.status == 0).then(|| screen(&o.stdout)).flatten()
    });
    let helper = build_helper(&xv);
    facts.helper_built = helper.is_ok();
    facts.needs = needs(Some(&xv), &facts.runtimes, helper.as_ref().err().map(String::as_str));
    facts
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `xcrun simctl list -j runtimes` prints with Xcode 26.
    pub const RUNTIMES: &str = r#"{"runtimes":[
      {"bundlePath":"/Library/Developer/CoreSimulator/Volumes/iOS_23F79/x/iOS 26.5.simruntime","buildversion":"23F79","platform":"iOS","identifier":"com.apple.CoreSimulator.SimRuntime.iOS-26-5","version":"26.5","isInternal":false,"isAvailable":true,"name":"iOS 26.5",
       "supportedDeviceTypes":[
        {"bundlePath":"/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/Library/Developer/CoreSimulator/Profiles/DeviceTypes/iPhone 17 Pro.simdevicetype","name":"iPhone 17 Pro","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17-Pro","productFamily":"iPhone"},
        {"bundlePath":"/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/Library/Developer/CoreSimulator/Profiles/DeviceTypes/iPhone 17.simdevicetype","name":"iPhone 17","identifier":"com.apple.CoreSimulator.SimDeviceType.iPhone-17","productFamily":"iPhone"},
        {"bundlePath":"/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/Library/Developer/CoreSimulator/Profiles/DeviceTypes/iPad Air 11-inch (M3).simdevicetype","name":"iPad Air 11-inch (M3)","identifier":"com.apple.CoreSimulator.SimDeviceType.iPad-Air-11-inch-M3","productFamily":"iPad"}]},
      {"platform":"iOS","identifier":"com.apple.CoreSimulator.SimRuntime.iOS-18-6","version":"18.6","isAvailable":true,"name":"iOS 18.6","supportedDeviceTypes":[]},
      {"platform":"iOS","identifier":"com.apple.CoreSimulator.SimRuntime.iOS-27-0","version":"27.0","isAvailable":false,"name":"iOS 27.0","supportedDeviceTypes":[]},
      {"platform":"watchOS","identifier":"w","version":"26.0","isAvailable":true,"name":"watchOS 26.0","supportedDeviceTypes":[]}]}"#;

    #[test]
    fn xcode_comes_from_xcodebuild() {
        assert_eq!(xcode_version("Xcode 26.6\nBuild version 17F113\n").as_deref(), Some("26.6 (17F113)"));
        assert_eq!(xcode_version("xcode-select: error: tool 'xcodebuild' requires Xcode"), None, "the Command Line Tools alone aren't Xcode");
    }

    #[test]
    fn runtimes_are_ios_available_newest_first_iphones_only() {
        let r = runtimes(RUNTIMES).unwrap();
        assert_eq!(r.iter().map(|x| x.version.as_str()).collect::<Vec<_>>(), vec!["26.5", "18.6"]);
        assert_eq!(r[0].iphones.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["iPhone 17 Pro", "iPhone 17"]);
        let roots = devicetype_roots(&r);
        assert!(roots.contains(&"/Applications/Xcode.app/Contents/Developer/Platforms/iPhoneOS.platform/Library/Developer/CoreSimulator/Profiles/DeviceTypes".to_string()), "{roots:?}");
        assert!(roots.contains(&"/Library/Developer/CoreSimulator/Profiles/DeviceTypes".to_string()));
        let off = offer_runtimes(&r, |_| Some((402, 874, 3000)));
        assert_eq!(off[0].device_types.iter().filter(|t| t.default).map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["iPhone 17"], "the plainest newest iPhone");
        let some_sized = offer_runtimes(&r, |b| (!b.contains("17 Pro")).then_some((402, 874, 3000)));
        assert_eq!(some_sized[0].device_types.len(), 1, "a type with no readable profile is left out");
    }

    #[test]
    fn screen_sizes_are_points_and_milli_scale() {
        assert_eq!(screen(r#"{"mainScreenWidth":1206,"mainScreenHeight":2622,"mainScreenScale":3}"#), Some((402, 874, 3000)));
        assert_eq!(screen(r#"{"mainScreenWidth":1206}"#), None);
    }

    #[test]
    fn needs_name_the_owner_steps_in_order() {
        let n = needs(None, &[], None);
        assert_eq!(n.len(), 1);
        assert!(n[0].command.contains("xcodebuild -license accept") && n[0].command.contains("xcode-select -s"), "{n:?}");
        let n = needs(Some("26.6 (17F113)"), &[], None);
        assert_eq!(n[0].command, "xcodebuild -downloadPlatform iOS");
        let rt = offer_runtimes(&runtimes(RUNTIMES).unwrap(), |_| Some((402, 874, 3000)));
        assert!(needs(Some("26.6"), &rt, None).is_empty());
        assert!(needs(Some("26.6"), &rt, Some("swiftc exited 1"))[0].what.contains("helper doesn't build"));
    }

    #[test]
    fn rss_sums_one_uid() {
        let ps = "  0 1000\n 299 2048\n299 1024\n  501 99999\nbad line\n";
        assert_eq!(rss_of(ps, 299), 3072 * 1024);
        assert_eq!(rss_of(ps, 77), 0);
    }
}
