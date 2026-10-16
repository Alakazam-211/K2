//! The guarded container as a rootless Quadlet `.container` for `k2app`
//! (`prd-k2-app-runtime-v1` §4.2, stage W §5): one canned template, filled
//! only with values the engine validated or made itself. The engine never
//! accepts unit text.
//!
//! The floor, item by item:
//! 1. rootless Podman under `k2app`, the App's own pinned id range from
//!    `k2app`'s subordinate ids (`UIDMap`/`GIDMap`, one 65536-id slot per
//!    App, kept across deploys so its data stays its own and the first
//!    start's layer copy is paid once, W §5.5);
//! 2. `DropCapability=all`, `NoNewPrivileges=true`;
//! 3. K2's seccomp profile (Podman's default plus denies for
//!    `CLONE_NEWUSER`/`unshare`, keys, bpf, userfaultfd, perf; W §5.6);
//! 4. MAC: Podman runs the OCI runtime through the engine's shim
//!    (`GlobalArgs=--runtime=<state>/oci/<rt>`), which puts the container
//!    and every exec into it under the `k2-app` AppArmor profile where box
//!    prep loaded it (`oci.rs`); SELinux boxes get Podman's `container_t`;
//! 5. read-only root, tmpfs `/tmp` and HOME, the Seed read-only at `/app`,
//!    `data/` and `cache/` read-write;
//! 6. nothing privileged, no host namespaces, no devices, no sockets;
//! 7. pasta with two forwards from the container's own loopback (`-T`):
//!    `127.0.0.1:8730` to the App's own gateway link on host loopback
//!    (`-T 8730:<link>`, S14) and the App's egress proxy port (S15; the
//!    container has no other way out); plus one per owner-approved box
//!    service (`-T <in-wall>:<host>`, 0.46.0; the per-App nft rule admits
//!    only approved host ports, so a held or revoked one stays shut); the
//!    App's port published on host loopback only;
//! 9. environment from an env file (never systemd-expanded) and secrets as
//!    Podman secrets (never in the unit); the App pass is a file in the
//!    App's pass folder, mounted read-only (it rotates, S14);
//! 10. caps: memory (no swap), pids, open files.
//!
//! Plus `StopTimeout` ≥ the drain window, an init (catatonit) so SIGTERM
//! reaches the App, and the image never pulled at start. Output goes to
//! the unit's journal; the rate limit (W §5.3) is on `user@<k2app>`
//! (box prep's delegate drop-in): journald ignores `LogRateLimit*` on user
//! units.
//! Health is the daemon's HTTP probe, not a Podman healthcheck (W §5.6).

use k2_root_proto::apps::AppId;
use k2_root_proto::engine::{app_slice, unit_stem, IN_WALL_GATEWAY_PORT, PASS_MOUNT, SLOT_SIZE};

/// One secret mount for the template (the Podman secret already exists).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretMount {
    Env { secret: String, var: String },
    File { secret: String, target: String },
}

#[derive(Debug, Clone)]
pub struct Render<'a> {
    pub app: &'a AppId,
    pub deployment: &'a str,
    pub image: &'a str,
    /// The OCI shim for `crun` or `youki` (`/etc/k2app/containers.conf`
    /// picks which; `oci.rs` puts the App under its AppArmor profile).
    pub runtime: &'a str,
    /// First id of the App's slot in `k2app`'s namespace.
    pub base: u64,
    pub seed_dir: &'a str,
    pub data_dir: &'a str,
    pub cache_dir: &'a str,
    pub env_file: &'a str,
    pub seccomp: &'a str,
    pub gw_port: u16,
    /// The App's egress proxy port (S15), forwarded like the gateway.
    pub px_port: u16,
    pub port: u16,
    pub argv: &'a [String],
    pub memory_mib: u64,
    pub drain_secs: u64,
    pub secrets: &'a [SecretMount],
    /// The App's pass folder on the host (S14), mounted at `/run/k2`.
    pub pass_dir: Option<&'a str>,
    /// Disk ceilings (§6.2): `IO*Max=` on the unit; the user manager owns
    /// the delegated `io` controller (box prep's `Delegate=`).
    pub io: &'a k2_root_proto::apps::IoMax,
    /// Box services (validated by `check_start`): extra pasta `-T`s.
    pub box_forwards: &'a [k2_root_proto::engine::BoxForward],
}

/// Container stop timeout: the drain window plus 10 s (AR45).
pub fn stop_timeout(drain_secs: u64) -> u64 {
    drain_secs + 10
}

/// One `Exec=` word in systemd command-line syntax: double-quoted, `\`
/// and `"` escaped, `%` (specifiers) and `$` (variables) doubled.
pub fn sd_quote(a: &str) -> String {
    let mut s = String::with_capacity(a.len() + 2);
    s.push('"');
    for c in a.chars() {
        match c {
            '\\' => s.push_str("\\\\"),
            '"' => s.push_str("\\\""),
            '%' => s.push_str("%%"),
            '$' => s.push_str("$$"),
            _ => s.push(c),
        }
    }
    s.push('"');
    s
}

pub fn render(r: &Render) -> String {
    let stem = unit_stem(r.app, r.deployment);
    let stop = stop_timeout(r.drain_secs);
    let mut o = String::new();
    let mut line = |s: String| {
        o.push_str(&s);
        o.push('\n');
    };
    line("# K2 App runtime (k2-app-engine): one deployment of a guarded App.".into());
    line("# Rendered from a canned template; edits are overwritten.".into());
    line("[Unit]".into());
    line(format!("Description=K2 App {} ({})", r.app.as_str(), r.deployment));
    line(String::new());
    line("[Container]".into());
    line(format!("ContainerName={stem}"));
    line(format!("Image={}", r.image));
    line("Pull=never".into());
    line(format!("GlobalArgs=--runtime={}", r.runtime));
    line(format!("Label=k2.app={}", r.app.as_str()));
    line(format!("Label=k2.deployment={}", r.deployment));
    line(format!("UIDMap=0:{}:{SLOT_SIZE}", r.base));
    line(format!("GIDMap=0:{}:{SLOT_SIZE}", r.base));
    line("DropCapability=all".into());
    line("NoNewPrivileges=true".into());
    line(format!("SeccompProfile={}", r.seccomp));
    line("ReadOnly=true".into());
    line("Tmpfs=/tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777".into());
    line("Tmpfs=/app-home:rw,nosuid,nodev,size=64m,mode=0700".into());
    line(format!("Volume={}:/app:ro", r.seed_dir));
    line(format!("Volume={}:/data:rw", r.data_dir));
    line(format!("Volume={}:/cache:rw", r.cache_dir));
    line("WorkingDir=/app".into());
    if let Some(d) = r.pass_dir {
        line(format!("Volume={d}:{PASS_MOUNT}:ro"));
    }
    let mut net = format!("Network=pasta:-T,{IN_WALL_GATEWAY_PORT}:{},-T,{}", r.gw_port, r.px_port);
    for f in r.box_forwards {
        net.push_str(&format!(",-T,{}:{}", f.app_port, f.host_port));
    }
    line(net);
    line(format!("PublishPort=127.0.0.1:{p}:{p}", p = r.port));
    line("RunInit=true".into());
    line("PidsLimit=512".into());
    line("Ulimit=nofile=4096:4096".into());
    line(format!("PodmanArgs=--memory={m}m --memory-swap={m}m", m = r.memory_mib));
    line(format!("StopTimeout={stop}"));
    line("LogDriver=passthrough".into());
    line(format!("EnvironmentFile={}", r.env_file));
    for s in r.secrets {
        match s {
            SecretMount::Env { secret, var } => line(format!("Secret={secret},type=env,target={var}")),
            SecretMount::File { secret, target } => line(format!("Secret={secret},type=mount,target={target},mode=0400,uid=0,gid=0")),
        }
    }
    line(format!("Exec={}", r.argv.iter().map(|a| sd_quote(a)).collect::<Vec<_>>().join(" ")));
    line(String::new());
    line("[Service]".into());
    line(format!("Slice={}", app_slice(r.app)));
    line("Restart=no".into());
    line("TimeoutStartSec=300".into());
    line(format!("TimeoutStopSec={}", stop + 20));
    for l in r.io.unit_lines() {
        line(l);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base_render<'a>(app: &'a AppId, argv: &'a [String], secrets: &'a [SecretMount]) -> Render<'a> {
        Render {
            app,
            deployment: "d-0123456789ab",
            image: "localhost/k2-base/node24:ec857ea6bb60",
            runtime: "/var/lib/k2app/engine/oci/crun",
            base: 65_537,
            seed_dir: "/var/lib/k2app/seeds/aaaa",
            data_dir: "/var/lib/k2app/apps/nsi-skin/data",
            cache_dir: "/var/lib/k2app/apps/nsi-skin/cache",
            env_file: "/var/lib/k2app/engine/run/nsi-skin/d-0123456789ab.env",
            seccomp: "/var/lib/k2app/engine/seccomp-k2-v1.json",
            gw_port: 38_601,
            px_port: 38_901,
            port: 41_234,
            argv,
            memory_mib: 512,
            drain_secs: 30,
            secrets,
            pass_dir: Some("/run/k2app/pass/nsi-skin"),
            io: &IO,
            box_forwards: &[],
        }
    }

    static IO: k2_root_proto::apps::IoMax = k2_root_proto::apps::IoMax {
        rbps: None,
        wbps: Some(41_943_040),
        riops: None,
        wiops: Some(2000),
    };

    #[test]
    fn the_floor_is_in_every_unit() {
        let app = AppId::parse("nsi-skin").unwrap();
        let argv = vec!["node".to_string(), "server.mjs".to_string()];
        let secrets = vec![
            SecretMount::Env { secret: "k2-nsi-skin--d-0123456789ab--0".into(), var: "STRIPE_KEY".into() },
            SecretMount::File { secret: "k2-nsi-skin--d-0123456789ab--1".into(), target: "/run/secrets/db.json".into() },
        ];
        let u = render(&base_render(&app, &argv, &secrets));
        for want in [
            "\nContainerName=k2app-nsi-skin--d-0123456789ab\n",
            "\nPull=never\n",
            "\nGlobalArgs=--runtime=/var/lib/k2app/engine/oci/crun\n",
            "\nUIDMap=0:65537:65536\n",
            "\nGIDMap=0:65537:65536\n",
            "\nDropCapability=all\n",
            "\nNoNewPrivileges=true\n",
            "\nSeccompProfile=/var/lib/k2app/engine/seccomp-k2-v1.json\n",
            "\nReadOnly=true\n",
            "\nVolume=/var/lib/k2app/seeds/aaaa:/app:ro\n",
            "\nNetwork=pasta:-T,8730:38601,-T,38901\n",
            "\nVolume=/run/k2app/pass/nsi-skin:/run/k2:ro\n",
            "\nPublishPort=127.0.0.1:41234:41234\n",
            "\nRunInit=true\n",
            "\nPidsLimit=512\n",
            "\nPodmanArgs=--memory=512m --memory-swap=512m\n",
            "\nStopTimeout=40\n",
            "\nSecret=k2-nsi-skin--d-0123456789ab--0,type=env,target=STRIPE_KEY\n",
            "\nSecret=k2-nsi-skin--d-0123456789ab--1,type=mount,target=/run/secrets/db.json,mode=0400,uid=0,gid=0\n",
            "\nExec=\"node\" \"server.mjs\"\n",
            "\nSlice=k2apps-nsi_skin.slice\n",
            "\nRestart=no\n",
            "\nTimeoutStopSec=60\n",
            "\nIOWriteBandwidthMax=/var/lib/k2app 41943040\n",
            "\nIOWriteIOPSMax=/var/lib/k2app 2000\n",
        ] {
            assert!(u.contains(want), "missing {want:?} in\n{u}");
        }
        // Never: host namespaces, privilege, devices, host loopback, sockets.
        for never in ["--privileged", "Network=host", "AddDevice", "map-host-loopback", "AddCapability", "docker.sock", "podman.sock", "PodmanArgs=--userns", "HealthCmd", "Environment="] {
            assert!(!u.contains(never), "{never} in\n{u}");
        }
        assert!(!u.contains("[Install]"), "never started at boot by systemd itself (the daemon adopts)");
        assert!(!u.contains("target=/run/k2/pass"), "the pass is a mounted file, never a Podman secret (it rotates)");
        let mut r = base_render(&app, &argv, &secrets);
        r.pass_dir = None;
        assert!(!render(&r).contains(":/run/k2:ro"), "no pass, no mount");
    }

    #[test]
    fn box_forwards_are_extra_pasta_forwards() {
        let app = AppId::parse("hearth").unwrap();
        let argv = vec!["node".to_string()];
        let fwd = [
            k2_root_proto::engine::BoxForward { app_port: 8470, host_port: 8470 },
            k2_root_proto::engine::BoxForward { app_port: 9000, host_port: 7880 },
        ];
        let mut r = base_render(&app, &argv, &[]);
        r.box_forwards = &fwd;
        let u = render(&r);
        assert!(u.contains("\nNetwork=pasta:-T,8730:38601,-T,38901,-T,8470:8470,-T,9000:7880\n"), "{u}");
        for never in ["map-host-loopback", "Network=host", "--map-gw"] {
            assert!(!u.contains(never), "{never} in\n{u}");
        }
    }

    #[test]
    fn exec_words_are_quoted_for_systemd() {
        assert_eq!(sd_quote("a b"), "\"a b\"");
        assert_eq!(sd_quote("$HOME"), "\"$$HOME\"");
        assert_eq!(sd_quote("100%"), "\"100%%\"");
        assert_eq!(sd_quote("say \"hi\""), "\"say \\\"hi\\\"\"");
        assert_eq!(sd_quote("C:\\x"), "\"C:\\\\x\"");
        let app = AppId::parse("jobs").unwrap();
        let argv = vec!["sh".to_string(), "-c".to_string(), "echo $X %h".to_string()];
        let u = render(&base_render(&app, &argv, &[]));
        assert!(u.contains("\nExec=\"sh\" \"-c\" \"echo $$X %%h\"\n"), "{u}");
    }

    #[test]
    fn stop_timeout_covers_the_drain() {
        assert!(stop_timeout(30) >= 30);
        assert_eq!(stop_timeout(0), 10);
    }
}
