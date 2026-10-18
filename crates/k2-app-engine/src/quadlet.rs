//! The App container as a rootless Quadlet `.container` for `k2app`
//! (`prd-k2-app-runtime-v1` §4.2–4.3, stage W §5): one canned template,
//! filled only with values the engine validated or made itself. The engine
//! never accepts unit text.
//!
//! The guarded floor, item by item:
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
//! 7. pasta with forwards from the container's own loopback (`-T`) only:
//!    `127.0.0.1:8730` to the App's own gateway link on host loopback
//!    (`-T 8730:<link>`, S14), the App's egress proxy port (S15; the
//!    container has no other way out), one per owner-approved box service
//!    (`-T <in-wall>:<host>`; the per-App nft rule admits only approved
//!    host ports, so a held or revoked one stays shut) and the App's
//!    database door (`-T 5432:<door>`, lane 3); the App's port published
//!    on host loopback only;
//! 9. environment from an env file (never systemd-expanded) and secrets as
//!    Podman secrets (never in the unit); the App pass is a file in the
//!    App's pass folder, mounted read-only (it rotates, S14);
//! 10. resources (S17, protocol 3): a memory cap only when the owner set
//!    one (L1: no cap by default; the Apps slice is the bound), no swap
//!    (`MemorySwapMax=0` on the unit), tasks, open files, the App's CPU
//!    weight on its own slice, `OOMPolicy=kill` (an OOM takes the whole
//!    App down cleanly and K2 restarts it), and a capped log file per
//!    deployment (`k8s-file` with `max-size`; the engine's `logs` verb
//!    reads it).
//!
//! The open path's **relaxed** container (§4.3, S13) keeps all of that
//! except: a writable root, `/app` a writable copy of the code made at
//! each start (Rosson Q3: `/data` is what persists), `/tmp` without
//! `noexec`, the App's own port inside (`$PORT`) behind K2's host port, and
//! extra TCP ports on host loopback. Its egress is broad but still only
//! through the App's proxy (the daemon's policy, K24).
//!
//! Plus `StopTimeout` ≥ the drain window, an init (catatonit) so SIGTERM
//! reaches the App, and the image never pulled at start.
//! Health is the daemon's HTTP probe, not a Podman healthcheck (W §5.6).

use k2_root_proto::apps::AppId;
use k2_root_proto::engine::{app_slice, unit_stem, Resources, Wall, IN_WALL_DB_PORT, IN_WALL_GATEWAY_PORT, PASS_MOUNT, SLOT_SIZE};

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
    pub wall: Wall,
    pub image: &'a str,
    /// The OCI shim for `crun` or `youki` (`/etc/k2app/containers.conf`
    /// picks which; `oci.rs` puts the App under its AppArmor profile).
    pub runtime: &'a str,
    /// First id of the App's slot in `k2app`'s namespace.
    pub base: u64,
    /// Guarded: the Seed (mounted read-only). Relaxed: this deployment's
    /// writable copy of it (mounted read-write).
    pub seed_dir: &'a str,
    pub data_dir: &'a str,
    pub cache_dir: &'a str,
    pub env_file: &'a str,
    pub seccomp: &'a str,
    pub gw_port: u16,
    /// The App's egress proxy port (S15), forwarded like the gateway.
    pub px_port: u16,
    /// The host loopback port K2 picked.
    pub port: u16,
    /// The App's own port inside (relaxed; guarded = `port`).
    pub app_port: u16,
    /// Relaxed: more ports, published under the same number.
    pub extra_ports: &'a [u16],
    pub argv: &'a [String],
    pub resources: &'a Resources,
    pub drain_secs: u64,
    pub secrets: &'a [SecretMount],
    /// The App's pass folder on the host (S14), mounted at `/run/k2`.
    pub pass_dir: Option<&'a str>,
    /// Disk ceilings (§6.2): `IO*Max=` on the unit; the user manager owns
    /// the delegated `io` controller (box prep's `Delegate=`).
    pub io: &'a k2_root_proto::apps::IoMax,
    /// Box services (validated by `check_start`): extra pasta `-T`s.
    pub box_forwards: &'a [k2_root_proto::engine::BoxForward],
    /// The database door on host loopback (`-T 5432:<door>`).
    pub db_port: Option<u16>,
}

/// One in-wall build (S16): the guarded floor around the manifest's build
/// argv, in the App's slot and slice (so its egress proxy and nft rule
/// apply), with the work copy writable at `/app`. No gateway link, no
/// pass, no secrets, no published port. Its output goes to `log_file`
/// through the unit's stdout (the container is removed when it exits).
#[derive(Debug, Clone)]
pub struct BuildRender<'a> {
    pub app: &'a AppId,
    pub build: &'a str,
    pub image: &'a str,
    pub runtime: &'a str,
    pub base: u64,
    pub work_dir: &'a str,
    pub env_file: &'a str,
    pub seccomp: &'a str,
    pub px_port: u16,
    pub argv: &'a [String],
    pub resources: &'a Resources,
    pub log_file: &'a str,
    pub timeout_secs: u64,
}

/// The build's container, unit and Quadlet file stem.
pub fn build_stem(app: &AppId, build: &str) -> String {
    format!("k2app-{}--{build}", app.as_str())
}

pub fn render_build(r: &BuildRender) -> String {
    let stem = build_stem(r.app, r.build);
    let mut o = String::new();
    let mut line = |s: String| {
        o.push_str(&s);
        o.push('\n');
    };
    line("# K2 App runtime (k2-app-engine): one in-wall build of an App (S16).".into());
    line("# Rendered from a canned template; edits are overwritten.".into());
    line("[Unit]".into());
    line(format!("Description=K2 App {} build {}", r.app.as_str(), r.build));
    line(String::new());
    line("[Container]".into());
    line(format!("ContainerName={stem}"));
    line(format!("Image={}", r.image));
    line("Pull=never".into());
    line(format!("GlobalArgs=--runtime={}", r.runtime));
    line(format!("Label=k2.app={}", r.app.as_str()));
    line(format!("Label=k2.build={}", r.build));
    line(format!("UIDMap=0:{}:{SLOT_SIZE}", r.base));
    line(format!("GIDMap=0:{}:{SLOT_SIZE}", r.base));
    line("DropCapability=all".into());
    line("NoNewPrivileges=true".into());
    line(format!("SeccompProfile={}", r.seccomp));
    line("ReadOnly=true".into());
    line("Tmpfs=/tmp:rw,nosuid,nodev,size=2g,mode=1777".into());
    line("Tmpfs=/app-home:rw,nosuid,nodev,size=512m,mode=0700".into());
    line(format!("Volume={}:/app:rw", r.work_dir));
    line("WorkingDir=/app".into());
    // Egress only through the App's proxy (registries are let through for
    // the build by the daemon's policy).
    line(format!("Network=pasta:-T,{}", r.px_port));
    line("RunInit=true".into());
    line(format!("PidsLimit={}", r.resources.tasks));
    line(format!("Ulimit=nofile={n}:{n}", n = r.resources.nofile));
    if let Some(m) = r.resources.memory_mib {
        line(format!("PodmanArgs=--memory={m}m --memory-swap={m}m"));
    }
    line("LogDriver=passthrough".into());
    line(format!("EnvironmentFile={}", r.env_file));
    line(format!("Exec={}", r.argv.iter().map(|a| sd_quote(a)).collect::<Vec<_>>().join(" ")));
    line(String::new());
    line("[Service]".into());
    line(format!("Slice={}", app_slice(r.app)));
    line("Restart=no".into());
    line(format!("TimeoutStartSec={}", r.timeout_secs + 60));
    line(format!("RuntimeMaxSec={}", r.timeout_secs));
    line(format!("StandardOutput=truncate:{}", r.log_file));
    line("StandardError=inherit".into());
    line("OOMPolicy=kill".into());
    line("MemorySwapMax=0".into());
    o
}

/// The App's own slice file in `k2app`'s user unit dir: its CPU weight
/// against the other Apps (the slice, not the unit: weights only compare
/// siblings). `None` = no file (systemd's defaults).
pub fn slice_unit(app: &AppId, r: &Resources) -> Option<String> {
    let w = r.cpu_weight?;
    Some(format!(
        "# K2 App runtime (k2-app-engine): App {}'s slice. Rendered; edits are overwritten.\n[Slice]\nCPUWeight={w}\n",
        app.as_str()
    ))
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
    let relaxed = r.wall == Wall::Relaxed;
    line(format!("# K2 App runtime (k2-app-engine): one deployment of a {} App.", if relaxed { "relaxed (open path)" } else { "guarded" }));
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
    line(format!("Label=k2.wall={}", r.wall.as_str()));
    line(format!("UIDMap=0:{}:{SLOT_SIZE}", r.base));
    line(format!("GIDMap=0:{}:{SLOT_SIZE}", r.base));
    line("DropCapability=all".into());
    line("NoNewPrivileges=true".into());
    line(format!("SeccompProfile={}", r.seccomp));
    if relaxed {
        // A writable root (tools write where they like; lost at restart)
        // and an exec-able /tmp (native addons, compilers).
        line("Tmpfs=/tmp:rw,nosuid,nodev,size=1g,mode=1777".into());
    } else {
        line("ReadOnly=true".into());
        line("Tmpfs=/tmp:rw,nosuid,nodev,noexec,size=256m,mode=1777".into());
    }
    line("Tmpfs=/app-home:rw,nosuid,nodev,size=64m,mode=0700".into());
    line(format!("Volume={}:/app:{}", r.seed_dir, if relaxed { "rw" } else { "ro" }));
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
    if let Some(db) = r.db_port {
        net.push_str(&format!(",-T,{IN_WALL_DB_PORT}:{db}"));
    }
    line(net);
    line(format!("PublishPort=127.0.0.1:{}:{}", r.port, r.app_port));
    for p in r.extra_ports {
        line(format!("PublishPort=127.0.0.1:{p}:{p}"));
    }
    line("RunInit=true".into());
    line(format!("PidsLimit={}", r.resources.tasks));
    line(format!("Ulimit=nofile={n}:{n}", n = r.resources.nofile));
    let mut podman_args = Vec::new();
    if let Some(m) = r.resources.memory_mib {
        podman_args.push(format!("--memory={m}m --memory-swap={m}m"));
    }
    podman_args.push(format!("--log-opt=max-size={}", r.resources.log_bytes));
    line(format!("PodmanArgs={}", podman_args.join(" ")));
    line(format!("StopTimeout={stop}"));
    line("LogDriver=k8s-file".into());
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
    // §6.2: an OOM anywhere in the App takes all of it down (K2 restarts
    // it with backoff), and it never swaps.
    line("OOMPolicy=kill".into());
    line("MemorySwapMax=0".into());
    for l in r.io.unit_lines() {
        line(l);
    }
    o
}

#[cfg(test)]
#[path = "quadlet_tests.rs"]
mod tests;
