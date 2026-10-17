//! Box prep for the App runtime (`prd-k2-app-runtime-v1` §10, S10;
//! vs-live 2 AR21/AR38/AR43/AR45): `GET /cli/app/box` (what `k2 app
//! doctor` prints) and `POST /cli/app/box/prepare` (the owner runs the
//! root helper's `apps` verbs, protocol 6, in order).
//!
//! The daemon reads what it can without root (kernel, cgroup v2, live
//! delegation, tool versions, `/etc/subuid`, reserved ports) and asks the
//! helper's `apps status` (open, read-only) for the rest (the nft table,
//! drop-ins, the configured OCI runtime). [`doctor`] is a pure function of
//! those [`Facts`], so the checks are tested off Linux.
//!
//! Dark until the runtime preview is on (the route table is the App
//! runtime's). Agents may read the doctor (like `k2 root-helper status`);
//! only the owner prepares a box.

use serde_json::{json, Value as J};

use k2_root_proto::apps::{OciRuntime, DB_PORTS, GATEWAY_PORTS, PODMAN_MIN_MAJOR, PROXY_PORTS};
use k2_root_proto::version::APPS_MIN_PROTOCOL;

use super::routes::{need_credential, no, Refusal, R};
use crate::compute_routes::Who;
use crate::root_helper::{self, Kind, RootHelperState};
use k2_core::connect_users::Role;

/// Everything the doctor looks at.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    pub linux: bool,
    pub preview: bool,
    pub helper_kind: Option<Kind>,
    pub helper_protocol: u32,
    pub helper_version: Option<String>,
    /// The helper's `apps status` JSON (protocol ≥ 6).
    pub helper_status: Option<J>,
    pub helper_status_error: Option<String>,
    pub kernel: Option<String>,
    /// `/sys/fs/cgroup/cgroup.controllers` (None = no cgroup v2).
    pub cgroup2: Option<String>,
    pub systemd: Option<String>,
    pub ptrace_scope: Option<String>,
    pub userns_restricted: Option<String>,
    /// Live controllers in `user@<k2app uid>.service` (None = not running).
    pub delegated: Option<String>,
    /// Versions the daemon itself can see (`--version`, no root).
    pub podman: Option<String>,
    pub crun: Option<String>,
    pub youki: Option<String>,
    pub pasta: bool,
    pub conmon: Option<String>,
    /// AR43: an owner confirm works headless (vault password or an Owner
    /// Connect login). `None` off Linux (Touch ID / login password).
    pub headless_confirm: Option<bool>,
    /// The engine's `version` answer, or why it didn't answer (S11).
    pub engine: Option<Result<J, String>>,
    pub skopeo: bool,
    /// `/etc/os-release` (lane P: is K2's Podman bundle for this box?).
    pub os_release: Option<String>,
    /// K2's Podman bundle is installed (`/opt/k2/podman`, lane P).
    pub podman_bundle: bool,
}

fn s(v: &Option<String>) -> &str {
    v.as_deref().unwrap_or("")
}

fn major(v: &str) -> Option<u32> {
    v.split('.').next()?.parse().ok()
}

fn kernel_ok(v: &str) -> bool {
    let mut it = v.split(|c: char| !c.is_ascii_digit());
    let (a, b) = (
        it.next().and_then(|x| x.parse::<u32>().ok()),
        it.next().and_then(|x| x.parse::<u32>().ok()),
    );
    // 5.11: rootless overlay and the idmapping Podman 5 leans on.
    matches!((a, b), (Some(a), Some(b)) if a > 5 || (a == 5 && b >= 11))
}

struct Check {
    id: &'static str,
    status: &'static str,
    detail: String,
    fix: Option<String>,
}

fn c(
    id: &'static str,
    status: &'static str,
    detail: impl Into<String>,
    fix: Option<&str>,
) -> Check {
    Check {
        id,
        status,
        detail: detail.into(),
        fix: fix.map(str::to_string),
    }
}

const PREPARE: &str = "k2 app box prepare (owner)";
/// Lane P: the fix on a box K2's Podman bundle is for.
const PREPARE_PODMAN: &str = "k2 app box prepare --install-podman (owner; --dry-run first to see every change)";

/// The MAC row (§4.2 item 4) from `apps status`'s `mac`. AppArmor on but
/// the `k2-app` profile not applied is a gate failure: Apps would start
/// without their MAC layer. No MAC at all is a warning (the floor is still
/// user namespaces, seccomp, no capabilities and a read-only root).
fn mac_check(m: &J) -> Check {
    match m["lsm"].as_str() {
        Some("selinux") => c("mac", "ok", "SELinux enforcing: Apps run as container_t (Podman's labels)", None),
        Some("apparmor") if m["applied"].as_bool() == Some(true) => {
            c("mac", "ok", "AppArmor: Apps run under the k2-app profile (enforce)", None)
        }
        // The kernel has AppArmor but the box has no apparmor_parser: box
        // prep installs the distro's apparmor package (apt, pacman).
        Some("apparmor") if m.get("parser").is_some_and(J::is_null) => c(
            "mac",
            "fail",
            "AppArmor is on in the kernel but apparmor_parser isn't installed: box prep installs the distro's apparmor package, then loads the k2-app profile",
            Some(PREPARE),
        ),
        Some("apparmor") => c(
            "mac",
            "fail",
            format!(
                "AppArmor is on but the k2-app profile isn't applied (loaded: {}, profile {}, engine marker {}, process drop-in {})",
                m["loaded"].as_str().unwrap_or("no"),
                m["profile"].as_str().unwrap_or("?"),
                m["marker"].as_str().unwrap_or("?"),
                m["processDropin"].as_str().unwrap_or("?"),
            ),
            Some(PREPARE),
        ),
        Some(_) => c(
            "mac",
            "warn",
            "no MAC layer on this box (AppArmor and SELinux are off): Apps keep user namespaces, seccomp, no capabilities and a read-only root",
            None,
        ),
        None => c("mac", "unknown", "the root helper didn't report the MAC layer (update it)", None),
    }
}

/// The doctor: one row per readiness item, `ok` | `warn` | `fail` |
/// `unknown`. `ready` = no `fail` and no `unknown` among the gate rows.
pub fn doctor(f: &Facts) -> J {
    let h = f.helper_status.as_ref();
    let hs = |ptr: &str| h.and_then(|v| v.pointer(ptr)).cloned().unwrap_or(J::Null);
    let mut out: Vec<Check> = Vec::new();

    out.push(if f.linux {
        c("platform", "ok", "Linux", None)
    } else {
        c(
            "platform",
            "fail",
            "Apps run on Linux boxes only; this machine can build and validate Seeds",
            None,
        )
    });

    let helper_detail = match f.helper_kind {
        Some(Kind::Root) => format!(
            "k2-root-helper {} protocol {}",
            s(&f.helper_version),
            f.helper_protocol
        ),
        Some(Kind::NotAllowed) => "installed, but sudoers doesn't let the daemon run it".into(),
        _ => "not installed".into(),
    };
    out.push(match f.helper_kind {
        Some(Kind::Root) if f.helper_protocol >= APPS_MIN_PROTOCOL => {
            c("helper", "ok", helper_detail, None)
        }
        Some(Kind::Root) => c(
            "helper",
            "fail",
            format!("{helper_detail}; Apps need protocol {APPS_MIN_PROTOCOL}"),
            Some("k2 root-helper update"),
        ),
        _ => c(
            "helper",
            "fail",
            helper_detail,
            Some("k2 root-helper status (the install command)"),
        ),
    });
    if let Some(e) = &f.helper_status_error {
        out.push(c(
            "helperStatus",
            "fail",
            format!("apps status failed: {e}"),
            None,
        ));
    }

    out.push(match &f.kernel {
        Some(k) if kernel_ok(k) => c("kernel", "ok", k.clone(), None),
        Some(k) => c(
            "kernel",
            "fail",
            format!("{k}: Apps need Linux 5.11 or newer"),
            None,
        ),
        None => c("kernel", "unknown", "no /proc/sys/kernel/osrelease", None),
    });
    out.push(match &f.cgroup2 {
        Some(ctl) => c(
            "cgroupV2",
            "ok",
            format!("controllers: {}", ctl.trim()),
            None,
        ),
        None => c(
            "cgroupV2",
            "fail",
            "no unified cgroup v2 hierarchy at /sys/fs/cgroup",
            None,
        ),
    });
    out.push(match f.systemd.as_deref().and_then(major) {
        Some(v) if v >= 249 => c("systemd", "ok", format!("systemd {v}"), None),
        Some(v) => c(
            "systemd",
            "fail",
            format!("systemd {v}: Apps need 249 or newer"),
            None,
        ),
        None => c(
            "systemd",
            "unknown",
            "systemctl --version gave no version",
            None,
        ),
    });

    // Engine: the helper's view first (it ran as root), the daemon's second.
    let podman = hs("/engine/podman")
        .as_str()
        .map(str::to_string)
        .or_else(|| f.podman.clone());
    // Lane P: on Ubuntu 24.04 / Debian 12 box prep installs K2's bundle.
    let bundle_os = f.os_release.as_deref().and_then(k2_root_proto::podman::supported_os);
    let bundle = f.podman_bundle || hs("/engine/podmanSource").as_str() == Some("k2");
    out.push(match podman.as_deref() {
        Some(v) if major(v).is_some_and(|m| m >= PODMAN_MIN_MAJOR) && bundle => {
            c("podman", "ok", format!("{v} (K2's bundle, {}; the distro's Podman is untouched)", k2_root_proto::podman::PREFIX), None)
        }
        Some(v) if major(v).is_some_and(|m| m >= PODMAN_MIN_MAJOR) => {
            c("podman", "ok", v.to_string(), None)
        }
        Some(v) if bundle_os.is_some() => c(
            "podman",
            "fail",
            format!(
                "Podman {v}: Apps need {PODMAN_MIN_MAJOR}+; box prep can install K2's pinned Podman {} for the k2app user",
                k2_root_proto::podman::BUNDLE_VERSION
            ),
            Some(PREPARE_PODMAN),
        ),
        Some(v) => c(
            "podman",
            "fail",
            format!(
                "Podman {v}: Apps need {PODMAN_MIN_MAJOR}+ (Debian 13, Ubuntu 26.04, Fedora, Arch)"
            ),
            None,
        ),
        None if bundle_os.is_some() => c("podman", "fail", "Podman is not installed", Some(PREPARE_PODMAN)),
        None => c("podman", "fail", "Podman is not installed", Some(PREPARE)),
    });
    let pasta = hs("/engine/pasta").as_bool().unwrap_or(f.pasta);
    out.push(if pasta {
        c("pasta", "ok", k2_root_proto::podman::pick(bundle, k2_root_proto::podman::PASTA, k2_root_proto::apps::PASTA), None)
    } else {
        c(
            "pasta",
            "fail",
            "pasta (passt) is not installed",
            Some(PREPARE),
        )
    });
    let crun = hs("/engine/crun")
        .as_str()
        .map(str::to_string)
        .or_else(|| f.crun.clone());
    let youki = hs("/engine/youki")
        .as_str()
        .map(str::to_string)
        .or_else(|| f.youki.clone());
    let configured = hs("/engine/runtime").as_str().map(str::to_string);
    let active = configured.clone().unwrap_or_else(|| "crun".into());
    let active_present = match active.as_str() {
        "youki" => youki.is_some(),
        _ => crun.is_some(),
    };
    let rt_detail = format!(
        "active {active}{} · crun {} · youki {}",
        if configured.is_none() {
            " (default, not recorded)"
        } else {
            ""
        },
        crun.as_deref().unwrap_or("missing"),
        youki.as_deref().unwrap_or("not installed (opt-in)"),
    );
    out.push(match (active_present, configured.is_some()) {
        (true, true) => c("ociRuntime", "ok", rt_detail, None),
        (true, false) => c("ociRuntime", "warn", rt_detail, Some(PREPARE)),
        (false, _) => c("ociRuntime", "fail", rt_detail, Some(PREPARE)),
    });
    let conmon = hs("/engine/conmon")
        .as_str()
        .map(str::to_string)
        .or_else(|| f.conmon.clone());
    out.push(match conmon {
        Some(v) => c("conmon", "ok", v, None),
        None => c("conmon", "fail", "conmon is not installed", Some(PREPARE)),
    });

    // Box prep state (needs the helper's apps status).
    if h.is_none() {
        out.push(c(
            "boxPrep",
            "unknown",
            "needs k2-root-helper protocol 6 to read",
            None,
        ));
    } else {
        out.push(
            match (hs("/user/exists").as_bool(), hs("/user/ok").as_bool()) {
                (Some(true), Some(true)) => c(
                    "appUser",
                    "ok",
                    format!("k2app uid {}", hs("/user/uid")),
                    None,
                ),
                (Some(true), _) => c(
                    "appUser",
                    "fail",
                    format!(
                        "k2app exists but {}",
                        hs("/user/problem")
                            .as_str()
                            .unwrap_or("has the wrong shape")
                    ),
                    None,
                ),
                _ => c("appUser", "fail", "no k2app user", Some(PREPARE)),
            },
        );
        out.push(match hs("/subid/ok").as_bool() {
            Some(true) => c(
                "subid",
                "ok",
                format!("{} subordinate ids", hs("/subid/count")),
                None,
            ),
            _ => c(
                "subid",
                "fail",
                hs("/subid/problem")
                    .as_str()
                    .unwrap_or("no subordinate uid/gid range for k2app")
                    .to_string(),
                Some(PREPARE),
            ),
        });
        out.push(if hs("/linger").as_bool() == Some(true) {
            c("linger", "ok", "k2app's user manager runs at boot", None)
        } else {
            c("linger", "fail", "linger is off for k2app", Some(PREPARE))
        });
        let want = ["cpu", "io", "memory", "pids"];
        out.push(match (hs("/delegate").as_str(), f.delegated.as_deref()) {
            (Some("ok"), Some(live))
                if want
                    .iter()
                    .all(|w| live.split_whitespace().any(|x| x == *w)) =>
            {
                c(
                    "delegation",
                    "ok",
                    format!("user@k2app: {}", live.trim()),
                    None,
                )
            }
            (Some("ok"), Some(live)) => c(
                "delegation",
                "warn",
                format!(
                    "drop-in present, user@k2app still has only: {} (restart it)",
                    live.trim()
                ),
                None,
            ),
            (Some("ok"), None) => c(
                "delegation",
                "ok",
                "drop-in present (user@k2app not running yet)",
                None,
            ),
            (Some(st), _) => c(
                "delegation",
                "fail",
                format!("delegate drop-in {st}"),
                Some(PREPARE),
            ),
            _ => c(
                "delegation",
                "fail",
                "delegate drop-in unknown",
                Some(PREPARE),
            ),
        });
        out.push(match (hs("/ioCost/available").as_bool(), hs("/ioCost/enabled").as_bool()) {
            (Some(true), Some(true)) => c("ioCost", "ok", format!("enabled on {}", devs(&hs("/ioCost/devices"))), None),
            (Some(true), _) => c("ioCost", "fail", format!("off on {}", devs(&hs("/ioCost/devices"))), Some(PREPARE)),
            _ => c(
                "ioCost",
                "warn",
                "io.cost unavailable (not the host's cgroup root, or no blk-iocost): disk I/O between Apps and the cores is not isolated (stage W9)",
                None,
            ),
        });
        out.push(if hs("/reservedPorts/covered").as_bool() == Some(true) {
            c(
                "reservedPorts",
                "ok",
                format!(
                    "{} reserved",
                    hs("/reservedPorts/range").as_str().unwrap_or("")
                ),
                None,
            )
        } else {
            c(
                "reservedPorts",
                "fail",
                "the App gateway range is not in ip_local_reserved_ports",
                Some(PREPARE),
            )
        });
        out.push(if hs("/nft/current").as_bool() == Some(true) {
            c(
                "nft",
                "ok",
                "inet k2app: App sockets reach only the gateway, proxy and db ports",
                None,
            )
        } else {
            c(
                "nft",
                "fail",
                "the inet k2app table is missing (it is re-applied at boot)",
                Some(PREPARE),
            )
        });
        out.push(match hs("/processUnit").as_str() {
            Some("ok") => c(
                "processUnit",
                "ok",
                "k2app-process@.service (system unit, User=k2app)",
                None,
            ),
            Some(st) => c(
                "processUnit",
                "fail",
                format!("k2app-process@.service {st}"),
                Some(PREPARE),
            ),
            None => c("processUnit", "unknown", "", None),
        });
        out.push(mac_check(&hs("/mac")));
        out.push(if hs("/coresSlice/present").as_bool() == Some(true) {
            c(
                "coresSlice",
                "ok",
                format!(
                    "k2cores.slice MemoryMin={}",
                    hs("/coresSlice/memoryMin").as_str().unwrap_or("?")
                ),
                None,
            )
        } else {
            c(
                "coresSlice",
                "warn",
                "no k2cores.slice yet (the resource layer sets the floor)",
                None,
            )
        });
        out.push(match (hs("/enginePresent").as_bool() == Some(true), &f.engine) {
            (true, Some(Ok(v))) if v["version"].as_str() == Some(env!("CARGO_PKG_VERSION")) => c(
                "engine",
                "ok",
                &format!("k2-app-engine {} answering (runtime {})", env!("CARGO_PKG_VERSION"), v["runtime"].as_str().unwrap_or("?")),
                None,
            ),
            (true, Some(Ok(v))) => c(
                "engine",
                "warn",
                &format!("k2-app-engine {} answers; this daemon is {} (k2 app engine install)", v["version"].as_str().unwrap_or("?"), env!("CARGO_PKG_VERSION")),
                None,
            ),
            (true, _) => c("engine", "warn", "k2-app-engine installed but not answering (k2 app engine install)", None),
            (false, _) => c(
                "engine",
                "warn",
                "k2-app-engine not installed yet (Apps run on the process engine until it is: k2 app engine install)",
                None,
            ),
        });
        out.push(if f.skopeo {
            c("skopeo", "ok", "skopeo present (fetches the pinned base image)", None)
        } else {
            c("skopeo", "warn", "skopeo missing: guarded deploys can't fetch the base image (k2 app box prepare)", None)
        });
    }

    out.push(match f.ptrace_scope.as_deref().map(str::trim) {
        Some("0") => c(
            "ptraceScope",
            "warn",
            "kernel.yama.ptrace_scope=0: agents (daemon uid) may ptrace the daemon (AR25)",
            None,
        ),
        Some(v) => c(
            "ptraceScope",
            "ok",
            format!("kernel.yama.ptrace_scope={v}"),
            None,
        ),
        None => c("ptraceScope", "ok", "no Yama", None),
    });
    if let Some("1") = f.userns_restricted.as_deref().map(str::trim) {
        out.push(c(
            "usernsRestricted",
            "warn",
            "Ubuntu restricts unprivileged user namespaces: process Apps run as system units for this reason (AR45)",
            None,
        ));
    }
    if let Some(ok) = f.headless_confirm {
        out.push(if ok {
            c("ownerConfirm", "ok", "a vault password or an Owner Connect login can approve widenings", None)
        } else {
            c(
                "ownerConfirm",
                "warn",
                "no vault password and no Owner Connect login: App approvals stay held until one is set (AR43)",
                Some("k2 vault password set"),
            )
        });
    }

    const GATE: &[&str] = &[
        "platform",
        "helper",
        "helperStatus",
        "kernel",
        "cgroupV2",
        "systemd",
        "podman",
        "pasta",
        "ociRuntime",
        "conmon",
        "boxPrep",
        "appUser",
        "subid",
        "linger",
        "delegation",
        "reservedPorts",
        "nft",
        "processUnit",
        "mac",
    ];
    let ready = out
        .iter()
        .filter(|ch| GATE.contains(&ch.id))
        .all(|ch| ch.status == "ok" || ch.status == "warn");
    let checks: Vec<J> = out
        .into_iter()
        .map(|ch| json!({"id": ch.id, "status": ch.status, "detail": ch.detail, "fix": ch.fix}))
        .collect();
    json!({
        "ready": ready,
        "preview": f.preview,
        "helperProtocol": f.helper_protocol,
        "appsMinProtocol": APPS_MIN_PROTOCOL,
        "runtime": active,
        "runtimeRecorded": configured.is_some(),
        "checks": checks,
        "helperStatus": f.helper_status,
    })
}

fn devs(v: &J) -> String {
    let d: Vec<String> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| x["dev"].as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if d.is_empty() {
        "no disk found".into()
    } else {
        d.join(", ")
    }
}

// ── gathering (Linux, no root) ──────────────────────────────────────────

fn read_trim(p: &str) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

fn version_of(abs: &str) -> Option<String> {
    if !std::path::Path::new(abs).exists() {
        return None;
    }
    let out = std::process::Command::new(abs)
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    text.split(|c: char| c.is_whitespace() || c == ',')
        .map(|w| w.trim_start_matches('v'))
        .find(|w| {
            let p: Vec<&str> = w.split('.').collect();
            p.len() >= 2
                && p.iter()
                    .all(|x| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_string)
}

fn app_uid() -> Option<u32> {
    let pw = std::fs::read_to_string("/etc/passwd").ok()?;
    pw.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() == 7 && f[0] == k2_root_proto::apps::APP_USER)
            .then(|| f[2].parse().ok())
            .flatten()
    })
}

fn headless_confirm() -> Option<bool> {
    if cfg!(target_os = "macos") {
        return None;
    }
    let vault_pw = crate::vault::get()
        .map(|v| v.has_confirm_password())
        .unwrap_or(false);
    let owner_login = k2_core::connect_users::list_users()
        .map(|u| u.iter().any(|u| !u.disabled && u.role == Role::Owner))
        .unwrap_or(false);
    Some(vault_pw || owner_login)
}

/// Read the box. Off Linux only the platform/preview rows are real.
pub fn gather() -> Facts {
    let st = root_helper::state();
    let mut f = Facts {
        linux: cfg!(target_os = "linux"),
        preview: k2_core::deploy::enabled(),
        helper_kind: Some(st.kind),
        helper_protocol: st.protocol,
        helper_version: st.version.clone(),
        ..Facts::default()
    };
    if !f.linux {
        return f;
    }
    apply_helper_status(&mut f, &st, |argv| root_helper::call_text(argv, None));
    f.kernel = read_trim("/proc/sys/kernel/osrelease");
    f.cgroup2 = read_trim("/sys/fs/cgroup/cgroup.controllers");
    f.systemd = std::process::Command::new("/usr/bin/systemctl")
        .arg("--version")
        .output()
        .ok()
        .and_then(|o| {
            let t = String::from_utf8_lossy(&o.stdout).to_string();
            t.split_whitespace().nth(1).map(str::to_string)
        });
    f.ptrace_scope = read_trim("/proc/sys/kernel/yama/ptrace_scope");
    f.userns_restricted = read_trim("/proc/sys/kernel/apparmor_restrict_unprivileged_userns");
    if let Some(uid) = app_uid() {
        f.delegated = read_trim(&format!(
            "/sys/fs/cgroup/user.slice/user-{uid}.slice/user@{uid}.service/cgroup.controllers"
        ));
    }
    // What k2app runs: K2's Podman bundle when installed (lane P).
    f.os_release = std::fs::read_to_string(k2_root_proto::podman::OS_RELEASE).ok();
    f.podman_bundle = k2_root_proto::podman::installed();
    f.podman = version_of(k2_root_proto::podman::podman_bin());
    f.crun = version_of(k2_root_proto::podman::crun_bin());
    f.youki = version_of(k2_root_proto::apps::YOUKI);
    f.pasta = std::path::Path::new(k2_root_proto::podman::pasta_bin()).exists();
    f.conmon = if f.podman_bundle {
        version_of(k2_root_proto::podman::CONMON)
    } else {
        k2_root_proto::apps::CONMON.iter().find_map(|p| version_of(p))
    };
    f.headless_confirm = headless_confirm();
    f.skopeo = std::path::Path::new(super::guarded::SKOPEO).exists();
    if std::path::Path::new(k2_root_proto::engine::SOCKET).exists() {
        f.engine = Some(super::guarded::engine_version());
    }
    f
}

fn apply_helper_status(
    f: &mut Facts,
    st: &RootHelperState,
    call: impl Fn(&[String]) -> Result<String, String>,
) {
    if !st.usable(APPS_MIN_PROTOCOL) {
        return;
    }
    match call(&["apps".to_string(), "status".to_string()]) {
        Ok(text) => match serde_json::from_str::<J>(&text) {
            Ok(v) => f.helper_status = Some(v),
            Err(_) => f.helper_status_error = Some("not JSON".into()),
        },
        Err(e) => f.helper_status_error = Some(e),
    }
}

// ── prepare ─────────────────────────────────────────────────────────────

/// What the owner asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareOpts {
    pub runtime: OciRuntime,
    /// `apps cores-slice <MiB>` (the resource layer sets real defaults).
    pub cores_mem_min_mib: Option<u64>,
    /// `apps budget <MiB> <tasks>`.
    pub budget: Option<(u64, u64)>,
}

/// The verbs, in order. Delegation before linger, so the user manager
/// starts with every controller (stage W2: restarting user@ after the
/// fact failed once).
pub fn plan(o: &PrepareOpts) -> Vec<Vec<String>> {
    let sv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let mut p = vec![sv(&["apps", "packages", "install"])];
    if o.runtime == OciRuntime::Youki {
        p.push(sv(&["apps", "packages", "install-youki"]));
    }
    p.extend([
        sv(&["apps", "ensure-user"]),
        sv(&["apps", "subid"]),
        sv(&["apps", "delegate", "on"]),
        sv(&["apps", "linger", "on"]),
        sv(&["apps", "reserve-ports", "on"]),
        sv(&["apps", "io-cost", "on"]),
        sv(&[
            "apps",
            "nft",
            "apply",
            &GATEWAY_PORTS.token(),
            &PROXY_PORTS.token(),
            &DB_PORTS.token(),
        ]),
        sv(&["apps", "process-unit", "on"]),
        // After the process template: its AppArmor drop-in sits next to it.
        sv(&["apps", "apparmor", "on"]),
        sv(&["apps", "runtime", o.runtime.as_str()]),
    ]);
    if let Some(n) = o.cores_mem_min_mib {
        p.push(sv(&["apps", "cores-slice", &n.to_string()]));
    }
    if let Some((m, t)) = o.budget {
        p.push(sv(&["apps", "budget", &m.to_string(), &t.to_string()]));
    }
    p
}

/// Run the plan through `call` (the root helper), stopping at the first
/// failure. `Err` carries the steps so far.
pub fn prepare_with(
    o: &PrepareOpts,
    call: impl Fn(&[String]) -> Result<String, String>,
) -> Result<J, J> {
    prepare_steps(plan(o), call)
}

/// Run a given plan (lane P: [`plan`] with the Podman install in it).
pub fn prepare_steps(
    plan: Vec<Vec<String>>,
    call: impl Fn(&[String]) -> Result<String, String>,
) -> Result<J, J> {
    let mut steps = Vec::new();
    for argv in plan {
        let verb = argv.join(" ");
        match call(&argv) {
            Ok(text) => {
                let result = serde_json::from_str::<J>(&text).unwrap_or(J::String(text));
                steps.push(json!({"verb": verb, "ok": true, "result": result}));
            }
            Err(e) => {
                steps.push(json!({"verb": verb, "ok": false, "error": e}));
                return Err(json!({"steps": steps, "failedStep": verb}));
            }
        }
    }
    Ok(json!({"steps": steps}))
}

/// Box prep is the owner's (§8.5: passports refused, Admin logins too).
pub(crate) fn may_prepare(who: &Who) -> R<()> {
    need_credential(who)?;
    match who {
        Who::Owner
        | Who::Login {
            role: Role::Owner, ..
        } => Ok(()),
        Who::Agent { .. } | Who::Shell { .. } => Err(no(
            "owner_only",
            403,
            "box prep is the owner's: run it from Settings or a terminal outside K2",
        )),
        _ => Err(no("owner_only", 403, "box prep is the owner's")),
    }
}

pub(crate) fn parse_opts(body: &J) -> R<PrepareOpts> {
    let runtime = match body.get("runtime").and_then(J::as_str) {
        None => OciRuntime::Crun,
        Some(r) => OciRuntime::parse(r).ok_or_else(|| {
            no(
                "bad_request",
                400,
                "runtime is crun (the default) or youki (opt-in)",
            )
        })?,
    };
    let num = |k: &str| body.get(k).and_then(J::as_u64);
    let cores_mem_min_mib = num("coresMemMinMib");
    let budget = match (num("appsMemMaxMib"), num("appsTasksMax")) {
        (Some(m), Some(t)) => Some((m, t)),
        (None, None) => None,
        _ => {
            return Err(no(
                "bad_request",
                400,
                "appsMemMaxMib and appsTasksMax go together",
            ))
        }
    };
    Ok(PrepareOpts {
        runtime,
        cores_mem_min_mib,
        budget,
    })
}

// ── routes ──────────────────────────────────────────────────────────────

/// `GET /cli/app/box`.
pub(crate) fn box_route(who: &Who) -> R<J> {
    need_credential(who)?;
    Ok(doctor(&gather()))
}

/// The Podman version the doctor goes by: the helper's view first (it
/// ran as root), the daemon's second.
fn podman_seen(f: &Facts) -> Option<String> {
    f.helper_status
        .as_ref()
        .and_then(|v| v.pointer("/engine/podman"))
        .and_then(J::as_str)
        .map(str::to_string)
        .or_else(|| f.podman.clone())
}

/// `POST /cli/app/box/prepare {runtime?, coresMemMinMib?, appsMemMaxMib?,
/// appsTasksMax?, installPodman?, dryRun?, podmanFrom?}`. Lane P: on a box
/// K2's Podman bundle is for, Podman < 5 stops prepare with
/// `podman_install_needed` unless `installPodman` (then the bundle goes in
/// before `packages install`); `dryRun` reports and changes nothing.
pub(crate) fn prepare_route(who: &Who, body: &J) -> R<J> {
    use super::podman_install as pi;
    let pod = pi::parse(body)?;
    if pod.dry_run {
        pi::may_dry_run(who)?;
    } else {
        may_prepare(who)?;
    }
    let opts = parse_opts(body)?;
    if pod.dry_run {
        let f = gather();
        let d = pi::decide(podman_seen(&f).as_deref(), f.podman_bundle, f.os_release.as_deref());
        let p = match d {
            pi::Decision::Offer { .. } => pi::plan_with_install(plan(&opts)),
            _ => plan(&opts),
        };
        return Ok(pi::dry_run(&d, &pod, &p, app_uid(), doctor(&f)));
    }
    if !cfg!(target_os = "linux") {
        return Err(no("linux_only", 409, "Apps run on Linux boxes only"));
    }
    let st = root_helper::state();
    if !st.usable(APPS_MIN_PROTOCOL) {
        return Err(no(
            "helper_needed",
            409,
            root_helper::helper_needed("App box prep", APPS_MIN_PROTOCOL, &st),
        ));
    }
    // Lane P: K2's Podman, opted in, before any step (never on upgrade).
    let before = gather();
    let d = pi::decide(podman_seen(&before).as_deref(), before.podman_bundle, before.os_release.as_deref());
    let tarball = pi::prepare_step(&pod, &d, &st, app_uid())?;
    let steps = match &tarball {
        Some(_) => pi::plan_with_install(plan(&opts)),
        None => plan(&opts),
    };
    let res = prepare_steps(steps, |argv| match (&tarball, pi::is_install(argv)) {
        (Some(b), true) => root_helper::call_text(argv, Some(b)),
        _ => root_helper::call_text(argv, None),
    });
    root_helper::invalidate();
    if tarball.is_some() {
        let r = match &res {
            Ok(_) => "ok".to_string(),
            Err(v) if v["failedStep"] == "apps podman install" => format!(
                "failed {}",
                v["steps"].as_array().and_then(|s| s.last()).map(|s| s["error"].to_string()).unwrap_or_default()
            ),
            Err(_) => "ok (a later box prep step failed)".to_string(),
        };
        pi::audit("app-podman-install", who, &format!("{r} version={}", k2_root_proto::podman::BUNDLE_VERSION));
    }
    let doc = doctor(&gather());
    match res {
        Ok(mut v) => {
            v["ok"] = json!(true);
            v["runtime"] = json!(opts.runtime.as_str());
            v["podman"] = match (&tarball, &d) {
                (Some(_), _) => json!({"action": "installed", "version": k2_root_proto::podman::BUNDLE_VERSION, "prefix": k2_root_proto::podman::PREFIX}),
                (None, pi::Decision::Keep { version, source }) => json!({"action": "keep", "have": version, "source": source}),
                (None, _) => json!({"action": "distro"}),
            };
            v["box"] = doc;
            Ok(v)
        }
        Err(v) => {
            let failed = v["failedStep"].as_str().unwrap_or("").to_string();
            let mut r: Refusal = no(
                "box_prep_failed",
                409,
                format!("box prep stopped at `{failed}`"),
            );
            let mut extra = v;
            extra["box"] = doc;
            r.extra = Some(extra);
            Err(r)
        }
    }
}

/// After every boot: the nft table and io.cost don't survive a reboot, and
/// another sysctl file can drop the reserved range. Re-apply the open
/// (tightening) verbs on a prepared box. Linux, preview on, helper ≥ 6.
pub fn spawn_boot_reapply() {
    if !cfg!(target_os = "linux") || !k2_core::deploy::enabled() {
        return;
    }
    std::thread::Builder::new()
        .name("app-box-reapply".into())
        .spawn(|| {
            let st = root_helper::state();
            if !st.usable(APPS_MIN_PROTOCOL) {
                return;
            }
            let status = root_helper::call_text(&["apps".into(), "status".into()], None)
                .ok()
                .and_then(|t| serde_json::from_str::<J>(&t).ok());
            for argv in reapply_plan(status.as_ref()) {
                if let Err(e) = root_helper::call_text(&argv, None) {
                    k2_core::log_debug!("[apps] box re-apply `{}` failed: {e}", argv.join(" "));
                }
            }
        })
        .ok();
}

/// The verbs to re-apply, given `apps status` (nothing unless the box was
/// prepared: k2app exists and has the right shape).
pub fn reapply_plan(status: Option<&J>) -> Vec<Vec<String>> {
    let Some(s) = status else { return Vec::new() };
    if s.pointer("/user/ok").and_then(J::as_bool) != Some(true) {
        return Vec::new();
    }
    let sv = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let mut p = Vec::new();
    if s.pointer("/nft/current").and_then(J::as_bool) != Some(true) {
        p.push(sv(&[
            "apps",
            "nft",
            "apply",
            &GATEWAY_PORTS.token(),
            &PROXY_PORTS.token(),
            &DB_PORTS.token(),
        ]));
    }
    if s.pointer("/reservedPorts/file").and_then(J::as_bool) == Some(true)
        && s.pointer("/reservedPorts/covered").and_then(J::as_bool) != Some(true)
    {
        p.push(sv(&["apps", "reserve-ports", "on"]));
    }
    if s.pointer("/ioCost/available").and_then(J::as_bool) == Some(true)
        && s.pointer("/ioCost/enabled").and_then(J::as_bool) != Some(true)
    {
        p.push(sv(&["apps", "io-cost", "on"]));
    }
    // AppArmor box whose k2-app profile isn't loaded in enforce mode (or a
    // pointer to it went missing): load it again before any App starts.
    if s.pointer("/mac/lsm").and_then(J::as_str) == Some("apparmor")
        && s.pointer("/mac/applied").and_then(J::as_bool) != Some(true)
    {
        p.push(sv(&["apps", "apparmor", "on"]));
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn good_status() -> J {
        json!({
            "user": {"exists": true, "ok": true, "uid": 996},
            "subid": {"ok": true, "count": 4194304},
            "linger": true,
            "delegate": "ok",
            "ioCost": {"available": true, "enabled": true, "devices": [{"dev": "259:0", "enabled": true}]},
            "reservedPorts": {"covered": true, "file": true, "range": "38600-38999"},
            "nft": {"table": true, "current": true},
            "processUnit": "ok",
            "mac": {"lsm": "apparmor", "applied": true, "loaded": "enforce", "profile": "ok", "marker": "ok", "processDropin": "ok"},
            "coresSlice": {"present": false},
            "engine": {"podman": "5.4.2", "podmanOk": true, "pasta": true, "crun": "1.21", "youki": null,
                       "conmon": "2.1.12", "runtime": "crun"},
            "enginePresent": false,
        })
    }

    fn good() -> Facts {
        Facts {
            linux: true,
            preview: true,
            helper_kind: Some(Kind::Root),
            helper_protocol: 6,
            helper_version: Some("0.46.0".into()),
            helper_status: Some(good_status()),
            kernel: Some("6.12.48+deb13-amd64".into()),
            cgroup2: Some("cpuset cpu io memory hugetlb pids rdma misc".into()),
            systemd: Some("257".into()),
            ptrace_scope: Some("1".into()),
            delegated: Some("cpuset cpu io memory pids".into()),
            headless_confirm: Some(true),
            ..Facts::default()
        }
    }

    fn status_of<'a>(d: &'a J, id: &str) -> &'a str {
        d["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == id)
            .unwrap_or_else(|| panic!("no check {id}"))["status"]
            .as_str()
            .unwrap()
    }

    #[test]
    fn a_prepared_box_is_ready_and_warnings_dont_block() {
        let d = doctor(&good());
        assert_eq!(d["ready"], true, "{d:#}");
        assert_eq!(d["runtime"], "crun");
        assert_eq!(status_of(&d, "podman"), "ok");
        assert_eq!(status_of(&d, "nft"), "ok");
        assert_eq!(status_of(&d, "engine"), "warn");
        assert_eq!(status_of(&d, "coresSlice"), "warn");
        for id in [
            "platform",
            "helper",
            "kernel",
            "cgroupV2",
            "systemd",
            "pasta",
            "ociRuntime",
            "conmon",
            "appUser",
            "subid",
            "linger",
            "delegation",
            "ioCost",
            "reservedPorts",
            "processUnit",
            "mac",
            "ptraceScope",
            "ownerConfirm",
        ] {
            assert_eq!(status_of(&d, id), "ok", "{id}");
        }
    }

    #[test]
    fn each_gate_failure_makes_the_box_not_ready() {
        type Edit = fn(&mut Facts);
        let cases: &[(&str, Edit)] = &[
            ("platform", |f| f.linux = false),
            ("helper", |f| f.helper_protocol = 5),
            ("helper", |f| f.helper_kind = Some(Kind::Missing)),
            ("kernel", |f| f.kernel = Some("5.4.0-150-generic".into())),
            ("cgroupV2", |f| f.cgroup2 = None),
            ("systemd", |f| f.systemd = Some("245".into())),
            ("podman", |f| {
                f.helper_status.as_mut().unwrap()["engine"]["podman"] = json!("4.9.3")
            }),
            ("pasta", |f| {
                f.helper_status.as_mut().unwrap()["engine"]["pasta"] = json!(false)
            }),
            (
                "appUser",
                |f| {
                    f.helper_status.as_mut().unwrap()["user"] =
                        json!({"exists": true, "ok": false, "problem": "k2app has a login shell"})
                },
            ),
            ("subid", |f| {
                f.helper_status.as_mut().unwrap()["subid"] = json!({"ok": false})
            }),
            ("linger", |f| {
                f.helper_status.as_mut().unwrap()["linger"] = json!(false)
            }),
            ("delegation", |f| {
                f.helper_status.as_mut().unwrap()["delegate"] = json!("drift")
            }),
            ("reservedPorts", |f| {
                f.helper_status.as_mut().unwrap()["reservedPorts"]["covered"] = json!(false)
            }),
            ("nft", |f| {
                f.helper_status.as_mut().unwrap()["nft"]["current"] = json!(false)
            }),
            ("processUnit", |f| {
                f.helper_status.as_mut().unwrap()["processUnit"] = json!("missing")
            }),
            // AppArmor on, k2-app not applied: Apps would start without MAC.
            ("mac", |f| {
                f.helper_status.as_mut().unwrap()["mac"] =
                    json!({"lsm": "apparmor", "applied": false, "loaded": null, "profile": "missing", "marker": "missing", "processDropin": "missing"})
            }),
            ("mac", |f| {
                f.helper_status.as_mut().unwrap()["mac"]["loaded"] = json!("complain");
                f.helper_status.as_mut().unwrap()["mac"]["applied"] = json!(false);
            }),
            // An older helper that doesn't report it: unknown, not ready.
            ("mac", |f| {
                f.helper_status.as_mut().unwrap().as_object_mut().unwrap().remove("mac");
            }),
            ("ociRuntime", |f| {
                f.helper_status.as_mut().unwrap()["engine"]["runtime"] = json!("youki");
            }),
            ("boxPrep", |f| f.helper_status = None),
        ];
        for (id, edit) in cases {
            let mut f = good();
            edit(&mut f);
            let d = doctor(&f);
            assert_eq!(d["ready"], false, "{id}: {d:#}");
            let st = status_of(&d, id);
            assert!(st == "fail" || st == "unknown", "{id}: {st}");
        }
    }

    #[test]
    fn soft_findings_warn_without_blocking() {
        let mut f = good();
        f.helper_status.as_mut().unwrap()["ioCost"] =
            json!({"available": false, "enabled": false, "devices": []});
        f.ptrace_scope = Some("0".into());
        f.userns_restricted = Some("1".into());
        f.headless_confirm = Some(false);
        f.delegated = Some("memory pids".into());
        f.helper_status.as_mut().unwrap()["engine"]["runtime"] = J::Null;
        let d = doctor(&f);
        assert_eq!(d["ready"], true, "{d:#}");
        for id in [
            "ioCost",
            "ptraceScope",
            "usernsRestricted",
            "ownerConfirm",
            "delegation",
            "ociRuntime",
        ] {
            assert_eq!(status_of(&d, id), "warn", "{id}");
        }
        assert_eq!(d["runtimeRecorded"], false);
        // A youki box records youki as active.
        let mut f = good();
        f.helper_status.as_mut().unwrap()["engine"]["runtime"] = json!("youki");
        f.helper_status.as_mut().unwrap()["engine"]["youki"] = json!("0.5.3");
        let d = doctor(&f);
        assert_eq!(d["runtime"], "youki");
        assert_eq!(status_of(&d, "ociRuntime"), "ok");
        // SELinux boxes: Podman's labels are the MAC layer. No MAC at all:
        // a warning (the rest of the floor still holds), never a block.
        let mut f = good();
        f.helper_status.as_mut().unwrap()["mac"] = json!({"lsm": "selinux", "applied": true});
        assert_eq!(status_of(&doctor(&f), "mac"), "ok");
        f.helper_status.as_mut().unwrap()["mac"] = json!({"lsm": "none", "applied": false});
        let d = doctor(&f);
        assert_eq!(status_of(&d, "mac"), "warn");
        assert_eq!(d["ready"], true);
    }

    #[test]
    fn helper_status_is_asked_only_at_protocol_6() {
        let asked = RefCell::new(0);
        let mut st = RootHelperState::missing();
        st.kind = Kind::Root;
        st.protocol = 5;
        let mut f = Facts::default();
        apply_helper_status(&mut f, &st, |_| {
            *asked.borrow_mut() += 1;
            Ok("{}".into())
        });
        assert_eq!(*asked.borrow(), 0);
        st.protocol = 6;
        apply_helper_status(&mut f, &st, |a| {
            assert_eq!(a, ["apps", "status"]);
            *asked.borrow_mut() += 1;
            Err("sudo: a password is required".into())
        });
        assert_eq!(*asked.borrow(), 1);
        assert!(f.helper_status_error.is_some());
        assert_eq!(status_of(&doctor(&f), "helperStatus"), "fail");
    }

    #[test]
    fn plan_order_and_options() {
        let base = PrepareOpts {
            runtime: OciRuntime::Crun,
            cores_mem_min_mib: None,
            budget: None,
        };
        let p: Vec<String> = plan(&base).iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            p,
            vec![
                "apps packages install",
                "apps ensure-user",
                "apps subid",
                "apps delegate on",
                "apps linger on",
                "apps reserve-ports on",
                "apps io-cost on",
                "apps nft apply 38600-38899 38900-38999 5432",
                "apps process-unit on",
                "apps apparmor on",
                "apps runtime crun",
            ]
        );
        // Every planned argv is on the helper's catalog.
        for argv in plan(&PrepareOpts {
            runtime: OciRuntime::Youki,
            cores_mem_min_mib: Some(768),
            budget: Some((4096, 4096)),
        }) {
            let r: Vec<&str> = argv.iter().map(String::as_str).collect();
            assert!(k2_root_proto::Verb::parse(&r).is_ok(), "{argv:?}");
        }
        let y: Vec<String> = plan(&PrepareOpts {
            runtime: OciRuntime::Youki,
            cores_mem_min_mib: Some(768),
            budget: Some((4096, 512)),
        })
        .iter()
        .map(|a| a.join(" "))
        .collect();
        assert_eq!(y[1], "apps packages install-youki");
        assert!(y.contains(&"apps runtime youki".to_string()));
        assert_eq!(y[y.len() - 2], "apps cores-slice 768");
        assert_eq!(y[y.len() - 1], "apps budget 4096 512");
    }

    #[test]
    fn prepare_stops_at_the_first_failure() {
        let seen = RefCell::new(Vec::new());
        let o = PrepareOpts {
            runtime: OciRuntime::Crun,
            cores_mem_min_mib: None,
            budget: None,
        };
        let r = prepare_with(&o, |a| {
            seen.borrow_mut().push(a.join(" "));
            if a[1] == "subid" {
                Err("refused: k2app's subordinate range overlaps another user's".into())
            } else {
                Ok(r#"{"changed":true}"#.into())
            }
        });
        let e = r.unwrap_err();
        assert_eq!(e["failedStep"], "apps subid");
        assert_eq!(seen.borrow().len(), 3, "nothing after the failed step ran");
        assert_eq!(e["steps"][2]["ok"], false);
        assert_eq!(e["steps"][0]["result"]["changed"], true);
        let ok = prepare_with(&o, |_| Ok("{}".into())).unwrap();
        assert_eq!(ok["steps"].as_array().unwrap().len(), plan(&o).len());
    }

    #[test]
    fn only_the_owner_prepares() {
        assert!(may_prepare(&Who::Owner).is_ok());
        assert!(may_prepare(&Who::Login {
            username: "o".into(),
            role: Role::Owner
        })
        .is_ok());
        for w in [
            Who::Login {
                username: "a".into(),
                role: Role::Admin,
            },
            Who::Login {
                username: "m".into(),
                role: Role::Member,
            },
            Who::Agent {
                project_id: "p".into(),
                session_id: "s".into(),
            },
            Who::Shell {
                project_id: "p".into(),
                session_id: "s".into(),
            },
            Who::Wrong,
            Who::Invalid,
        ] {
            let e = may_prepare(&w).unwrap_err();
            assert_eq!(e.status, 403, "{w:?}");
        }
    }

    #[test]
    fn opts_parse_strictly() {
        assert_eq!(
            parse_opts(&json!({})).ok().unwrap().runtime,
            OciRuntime::Crun
        );
        assert_eq!(
            parse_opts(&json!({"runtime": "youki"}))
                .ok()
                .unwrap()
                .runtime,
            OciRuntime::Youki
        );
        for bad in [
            json!({"runtime": "runc"}),
            json!({"runtime": "/usr/bin/crun"}),
            json!({"appsMemMaxMib": 4096}),
        ] {
            assert!(parse_opts(&bad).is_err(), "{bad}");
        }
        let o = parse_opts(
            &json!({"coresMemMinMib": 768, "appsMemMaxMib": 4096, "appsTasksMax": 4096}),
        )
        .ok()
        .unwrap();
        assert_eq!(o.cores_mem_min_mib, Some(768));
        assert_eq!(o.budget, Some((4096, 4096)));
    }

    #[test]
    fn boot_reapply_only_on_a_prepared_box_and_only_what_drifted() {
        assert!(reapply_plan(None).is_empty());
        assert!(reapply_plan(Some(&json!({"user": {"exists": false}}))).is_empty());
        assert!(
            reapply_plan(Some(&good_status())).is_empty(),
            "nothing drifted"
        );
        let mut s = good_status();
        s["nft"]["current"] = json!(false);
        s["ioCost"]["enabled"] = json!(false);
        s["reservedPorts"]["covered"] = json!(false);
        let p: Vec<String> = reapply_plan(Some(&s)).iter().map(|a| a.join(" ")).collect();
        assert_eq!(
            p,
            vec![
                "apps nft apply 38600-38899 38900-38999 5432",
                "apps reserve-ports on",
                "apps io-cost on"
            ]
        );
        // Reserved ports only come back if box prep wrote the file.
        s["reservedPorts"]["file"] = json!(false);
        s["ioCost"]["available"] = json!(false);
        let p: Vec<String> = reapply_plan(Some(&s)).iter().map(|a| a.join(" ")).collect();
        assert_eq!(p, vec!["apps nft apply 38600-38899 38900-38999 5432"]);
        // The k2-app profile not loaded after a boot: load it again; never
        // on a box whose LSM isn't AppArmor.
        let mut s = good_status();
        s["mac"]["applied"] = json!(false);
        let p: Vec<String> = reapply_plan(Some(&s)).iter().map(|a| a.join(" ")).collect();
        assert_eq!(p, vec!["apps apparmor on"]);
        s["mac"] = json!({"lsm": "none", "applied": false});
        assert!(reapply_plan(Some(&s)).is_empty());
        // Every re-apply verb is open (a tightening) on the helper's table.
        for argv in reapply_plan(Some(&{
            let mut s = good_status();
            s["nft"]["current"] = json!(false);
            s["ioCost"]["enabled"] = json!(false);
            s["reservedPorts"]["covered"] = json!(false);
            s
        })) {
            let r: Vec<&str> = argv.iter().map(String::as_str).collect();
            assert_eq!(
                k2_root_proto::Verb::parse(&r).unwrap().class(),
                k2_root_proto::Class::Open
            );
        }
    }

    #[test]
    fn apparmor_without_its_parser_points_at_box_prep() {
        let mut f = good();
        f.helper_status.as_mut().unwrap()["mac"] =
            json!({"lsm": "apparmor", "applied": false, "loaded": null, "profile": "missing", "marker": "missing", "processDropin": "missing", "parser": null});
        let d = doctor(&f);
        assert_eq!(d["ready"], false, "{d:#}");
        let row = d["checks"].as_array().unwrap().iter().find(|c| c["id"] == "mac").unwrap();
        assert_eq!(row["status"], "fail");
        assert!(row["detail"].as_str().unwrap().contains("apparmor_parser isn't installed"), "{row}");
        assert_eq!(row["fix"], PREPARE);
        // Prep installs packages before it loads the profile.
        let p: Vec<String> = plan(&PrepareOpts { runtime: OciRuntime::Crun, cores_mem_min_mib: None, budget: None })
            .iter()
            .map(|a| a.join(" "))
            .collect();
        let pk = p.iter().position(|v| v == "apps packages install").unwrap();
        let aa = p.iter().position(|v| v == "apps apparmor on").unwrap();
        assert!(pk < aa, "{p:?}");
    }
}
