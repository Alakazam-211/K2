//! `cell_egress` — P4-H5 fail-closed host egress allowlist for microVM cells.
//!
//! ## Why
//!
//! P4-H5 enables real internet for a sandbox cell: the worker now passes
//! `KRUN_TSI_HIJACK_INET` to `krun_add_vsock`, so libkrun re-issues the guest's
//! outbound INET `connect()`s on the HOST network stack from the priv-dropped
//! `k2cell` VMM (there is no virtio-net device). That is the FUNCTIONAL unblock
//! — without it the guest's `claude` cannot reach `api.anthropic.com`.
//!
//! But "the cell can now reach the internet" is exactly the thing an UNTRUSTED
//! tenant must NOT have unrestricted. This module is the CONTROL: a host
//! `nftables` policy that, for the cell's egress uid (`k2cell`), **default-DROPs
//! outbound INET and ALLOWs only**:
//!   * `tcp dport 443` — the LLM endpoint (Anthropic, TLS), and
//!   * `udp/tcp dport 53` — DNS,
//! to NON-loopback destinations. Everything else — other ports, raw IPs on
//! other ports, host-loopback services (the daemon's own loopback TCP) — is
//! dropped. **Fail-closed: chain default is structured so a k2cell packet
//! always hits an explicit verdict, ending in `drop`; only 443/53 escape it.**
//!
//! ## uid-precision (the spike's key finding)
//!
//! `meta skuid <k2cell>` is uid-precise on this kernel (nft 1.0.9, 6.8): it
//! catches ONLY the k2cell VMM's TSI sockets, never the daemon/owner/root. The
//! base chain's only rule is a POSITIVE match, `meta skuid <uid> jump
//! cell_policy`; everything else falls to the chain policy `accept`, so the
//! daemon's own egress, owner traffic, and the box's services are unaffected.
//!
//! **Why positive (0.45.1, quiet-gate PRD §6.1):** the first version used
//! `meta skuid != <uid> accept` and then `ip daddr 127.0.0.0/8 drop`. A packet
//! with NO owning socket (the kernel's RST for a closed port) has no skuid, so
//! `!= <uid>` did not match either, the packet fell through, and the loopback
//! drop ate it. Every refused localhost connection on the host then HUNG
//! (SYN retransmits, no RST) for as long as a cell table existed, including
//! tables left behind by a crash. `meta skuid <uid>` never matches a
//! socket-less packet, so those always pass. The vsock cell-channel is AF_VSOCK/AF_UNIX, not INET, so this
//! OUTPUT hook never touches it (`k2 msg`/inbox keep working).
//!
//! ## Shape + lifecycle (P4-H6 — per-session)
//!
//! With H6 each cell's VMM drops to its OWN per-session uid (see
//! [`crate::cell_uid_pool`]), so the policy is now PER-SESSION: a dedicated
//! `inet k2sandbox_<uid>` table per live cell, each with one base chain hooking
//! `output` and scoping ONLY that cell's `skuid`. It is installed idempotently +
//! atomically by the daemon (root) BEFORE it boots the cell, REFUSED-closed if
//! it can't be installed (we never boot a now-internet-capable cell with no
//! egress lockdown), and TORN DOWN per cell on `ChildExit` (the authoritative
//! observer calls [`remove_egress_policy`] with the same uid). No shared-policy
//! compromise: cell A's table is removed when cell A exits without touching cell
//! B's.
//!
//! ## Multiple base chains compose correctly (the H6 key property)
//!
//! Every per-uid table registers one base chain at `hook output priority 0`.
//! netfilter evaluates ALL base chains at a hook: a terminal `accept` in one
//! chain does NOT prevent another chain from `drop`ping the packet, and the
//! packet is delivered only if NO chain drops it. Each base chain jumps to its
//! policy ONLY for its own uid (`meta skuid <uid> jump cell_policy`) and
//! accepts everything else by policy, so cell A's packet is policed by chain A
//! (default-DROP unless 443/53) while chain B simply accepts it — the per-uid tables stack without interfering, and
//! the daemon/owner/root (no per-uid table targets them) are never policed.
//!
//! ## Default-OFF
//!
//! Reached ONLY from the microVM spawn path, which only resolves on a
//! `linux + sandbox-microvm` build with `K2_SANDBOX` enabled. With the sandbox
//! off, nothing here runs and NO nft state is touched (byte-identical). On
//! non-Linux the functions are no-ops (nft is Linux-only; the branch is dead
//! there anyway because `resolve_sandbox` never returns `Microvm`).

/// The per-session base-chain name (one base chain per per-uid table).
#[cfg(target_os = "linux")]
const CHAIN: &str = "cell_egress";

/// The regular chain holding the cell's verdicts (reached only by a jump
/// for the cell's own uid).
#[cfg(target_os = "linux")]
const POLICY_CHAIN: &str = "cell_policy";

/// Table-name prefix shared by every per-session table.
#[cfg(target_os = "linux")]
const TABLE_PREFIX: &str = "k2sandbox_";

/// The PER-SESSION table name for `cell_uid` — `k2sandbox_<uid>`. Namespaced so
/// we never collide with a box's existing firewall, and per-uid so install +
/// teardown target EXACTLY this cell's state without touching any other cell.
#[cfg(target_os = "linux")]
fn table_name(cell_uid: u32) -> String {
    format!("{TABLE_PREFIX}{cell_uid}")
}

/// Install (idempotently + atomically) the fail-closed egress allowlist for THIS
/// cell's per-session `cell_uid`. Safe to call on every microVM spawn — the
/// script `add table; delete table; table {…}` replaces this cell's OWN table
/// atomically; other live cells' per-uid tables are untouched.
///
/// **Fail-closed:** returns `Err` if `nft` is missing, errors, or is killed —
/// the caller MUST refuse to boot the cell rather than run it with open egress.
#[cfg(target_os = "linux")]
pub(crate) fn ensure_egress_policy(cell_uid: u32) -> std::io::Result<()> {
    // Refuse a nonsense/root uid defensively (the allocator floor already keeps
    // uids far above 0, but this is the security boundary — never install a
    // policy keyed on uid 0, which would scope the daemon/root itself).
    if cell_uid == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "refusing to install egress policy for uid 0",
        ));
    }

    run_nft(&build_ruleset(cell_uid))
}

/// Build the atomic idempotent install ruleset for `cell_uid`'s per-session
/// table. `add table` makes the subsequent `delete table` safe whether or not it
/// already existed; then we (re)define the whole `k2sandbox_<uid>` table in one
/// `nft -f` transaction.
///
/// Base chain: ONE rule, `meta skuid <uid> jump cell_policy`, policy
/// `accept`. Every packet that is not from this cell's socket (another uid,
/// or no socket at all, such as a kernel RST) is accepted untouched; each
/// other cell has its OWN per-uid table that polices it.
///
/// `cell_policy` (this cell's uid only):
///   1. loopback dst        → drop  (no host-local services, incl. the daemon's
///                                  loopback TCP listener)
///   2. ipv6                → drop
///   3. tcp dport 443       → accept (Anthropic / LLM, TLS) to non-loopback
///   4. udp/tcp dport 53    → accept (DNS) to non-loopback
///   5. (fall-through)      → drop  (fail-closed: all other cell egress)
/// A packet from this cell's uid always hits an explicit verdict in
/// `cell_policy`; it never returns to the base chain's `accept` policy.
#[cfg(target_os = "linux")]
fn build_ruleset(cell_uid: u32) -> String {
    let table = table_name(cell_uid);
    format!(
        "add table inet {table}\n\
         delete table inet {table}\n\
         table inet {table} {{\n\
         \tchain {CHAIN} {{\n\
         \t\ttype filter hook output priority 0; policy accept;\n\
         \t\tmeta skuid {uid} jump {POLICY_CHAIN}\n\
         \t}}\n\
         \tchain {POLICY_CHAIN} {{\n\
         \t\tip daddr 127.0.0.0/8 drop\n\
         \t\tip6 daddr ::1 drop\n\
         \t\tmeta nfproto ipv6 counter drop\n\
         \t\ttcp dport 443 counter accept\n\
         \t\tudp dport 53 counter accept\n\
         \t\ttcp dport 53 counter accept\n\
         \t\tcounter drop\n\
         \t}}\n\
         }}\n",
        table = table,
        CHAIN = CHAIN,
        POLICY_CHAIN = POLICY_CHAIN,
        uid = cell_uid,
    )
}

/// Remove THIS cell's per-session egress table. Idempotent (the `add table`
/// makes the `delete` safe when absent), so it always "leaves clean". Called
/// per-cell on `ChildExit` by the authoritative teardown observer with the SAME
/// `cell_uid` the spawn allocated — removing only this cell's table, never a
/// concurrent cell's.
#[cfg(target_os = "linux")]
pub(crate) fn remove_egress_policy(cell_uid: u32) -> std::io::Result<()> {
    let table = table_name(cell_uid);
    let ruleset = format!(
        "add table inet {table}\n\
         delete table inet {table}\n",
        table = table,
    );
    let removed = run_nft(&ruleset);
    // Teardown also clears any OTHER table left behind by a crash or a
    // killed daemon (best-effort; this cell's own removal result is what
    // the caller sees).
    if let Err(e) = sweep_stale_tables() {
        k2_core::log_debug!("[sandbox] stale egress-table sweep after uid {cell_uid}: {e}");
    }
    removed
}

/// The per-session uids named by `k2sandbox_<uid>` tables in an
/// `nft list tables` listing (`table inet k2sandbox_60001` lines).
#[cfg(target_os = "linux")]
fn sandbox_table_uids(listing: &str) -> Vec<u32> {
    let mut uids: Vec<u32> = listing
        .lines()
        .filter_map(|l| {
            let mut parts = l.split_whitespace();
            match (parts.next(), parts.next(), parts.next(), parts.next()) {
                (Some("table"), Some("inet"), Some(name), None) => name
                    .strip_prefix(TABLE_PREFIX)
                    .and_then(|n| n.parse::<u32>().ok()),
                _ => None,
            }
        })
        .collect();
    uids.sort_unstable();
    uids.dedup();
    uids
}

/// The tables to delete: every sandbox table whose uid is neither allocated
/// in the pool (a cell is booting or live in THIS daemon) nor owns a
/// running process (a cell from a previous daemon that is still alive).
#[cfg(target_os = "linux")]
fn stale_uids(tables: &[u32], allocated: &[u32], live_process_uids: &std::collections::HashSet<u32>) -> Vec<u32> {
    tables
        .iter()
        .copied()
        .filter(|u| !allocated.contains(u) && !live_process_uids.contains(u))
        .collect()
}

/// Every uid (real, effective, saved or fs) that owns a process right now.
#[cfg(target_os = "linux")]
fn live_process_uids() -> std::io::Result<std::collections::HashSet<u32>> {
    let mut out = std::collections::HashSet::new();
    for entry in std::fs::read_dir("/proc")? {
        let entry = entry?;
        let name = entry.file_name();
        if !name.to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        // A process can exit between readdir and read: skip it.
        let Ok(status) = std::fs::read_to_string(entry.path().join("status")) else {
            continue;
        };
        if let Some(line) = status.lines().find(|l| l.starts_with("Uid:")) {
            out.extend(line.split_whitespace().skip(1).filter_map(|v| v.parse::<u32>().ok()));
        }
    }
    Ok(out)
}

/// Delete every `k2sandbox_<uid>` table with no live cell (not allocated in
/// the pool, no process with that uid). Run at daemon boot and on every cell
/// teardown, so a crashed cell or killed daemon never leaves a table behind.
/// Returns the uids whose tables were removed.
#[cfg(target_os = "linux")]
pub(crate) fn sweep_stale_tables() -> std::io::Result<Vec<u32>> {
    let out = std::process::Command::new("nft")
        .args(["list", "tables"])
        .stdin(std::process::Stdio::null())
        .output()?;
    if !out.status.success() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "nft list tables failed ({}): {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    let tables = sandbox_table_uids(&String::from_utf8_lossy(&out.stdout));
    if tables.is_empty() {
        return Ok(Vec::new());
    }
    let stale = stale_uids(&tables, &crate::cell_uid_pool::allocated(), &live_process_uids()?);
    let mut removed = Vec::new();
    for uid in stale {
        let table = table_name(uid);
        run_nft(&format!("add table inet {table}\ndelete table inet {table}\n"))?;
        k2_core::log_debug!("[sandbox] removed stale egress table {table} (no live cell)");
        removed.push(uid);
    }
    Ok(removed)
}

/// Feed a ruleset to `nft -f -` (stdin). Returns `Err` on spawn failure,
/// non-zero exit, or signal — the caller treats any error as fail-closed.
#[cfg(target_os = "linux")]
fn run_nft(ruleset: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("nft")
        .arg("-f")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    // SAFETY: stdin is piped (`unwrap` cannot fail here).
    child
        .stdin
        .take()
        .expect("nft stdin piped")
        .write_all(ruleset.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!(
                "nft -f failed ({}): {}",
                out.status,
                stderr.trim()
            ),
        ));
    }
    Ok(())
}

// ── Non-Linux: no-ops (nft is Linux-only; the microVM branch is dead here) ───
#[cfg(not(target_os = "linux"))]
pub(crate) fn ensure_egress_policy(_cell_uid: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub(crate) fn remove_egress_policy(_cell_uid: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
#[allow(dead_code)]
pub(crate) fn sweep_stale_tables() -> std::io::Result<Vec<u32>> {
    Ok(Vec::new())
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn ruleset_is_fail_closed_and_uid_scoped() {
        let rs = build_ruleset(999);
        // uid-scoped by a POSITIVE match: only this uid's sockets are
        // policed; everything else (other uids, socket-less RSTs) falls to
        // the base chain's accept policy.
        assert!(
            rs.contains("meta skuid 999 jump cell_policy"),
            "only the cell's own uid may enter the policy: {rs}"
        );
        assert!(
            !rs.contains("skuid !="),
            "a negative skuid match drops socket-less packets (kernel RSTs): {rs}"
        );
        // Fail-closed: the chain ends in an unconditional drop for k2cell.
        assert!(
            rs.trim_end().contains("counter drop"),
            "k2cell egress must fall through to drop (fail-closed): {rs}"
        );
        // Allowlist = exactly 443 + DNS(53).
        assert!(rs.contains("tcp dport 443 counter accept"), "443 must be allowed");
        assert!(rs.contains("udp dport 53 counter accept"), "DNS/udp must be allowed");
        assert!(rs.contains("tcp dport 53 counter accept"), "DNS/tcp must be allowed");
        // Loopback host services explicitly dropped (no daemon-loopback reach).
        assert!(rs.contains("ip daddr 127.0.0.0/8 drop"), "loopback v4 must be dropped");
        assert!(rs.contains("ip6 daddr ::1 drop"), "loopback v6 must be dropped");
        // No port 80 / arbitrary-port allow leaked in.
        assert!(!rs.contains("dport 80 "), "must not allow port 80");
        // OUTPUT hook (egress control).
        assert!(rs.contains("hook output"), "must hook output");
        // Idempotent + PER-SESSION: add-then-delete-then-define the per-uid table.
        assert!(rs.starts_with("add table inet k2sandbox_999\ndelete table inet k2sandbox_999\n"));
    }

    #[test]
    fn per_session_tables_are_distinct_per_uid() {
        // Two cells with distinct uids get DISTINCT tables — so teardown of one
        // never removes the other's policy (no shared-policy compromise).
        let a = build_ruleset(60000);
        let b = build_ruleset(60001);
        assert!(a.contains("table inet k2sandbox_60000"));
        assert!(b.contains("table inet k2sandbox_60001"));
        assert!(!a.contains("k2sandbox_60001"), "cell A must not touch cell B's table");
        assert!(!b.contains("k2sandbox_60000"), "cell B must not touch cell A's table");
        // Each chain scopes ONLY its own uid; all other uids pass untouched.
        assert!(a.contains("meta skuid 60000 jump cell_policy"));
        assert!(b.contains("meta skuid 60001 jump cell_policy"));
    }

    #[test]
    fn remove_targets_only_this_uids_table() {
        // The teardown ruleset names exactly this cell's per-uid table.
        let table = table_name(60042);
        assert_eq!(table, "k2sandbox_60042");
    }

    #[test]
    fn install_refuses_uid_zero() {
        // Never key the policy on uid 0 (would scope root/the daemon itself).
        let err = ensure_egress_policy(0).expect_err("uid 0 must be refused");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
    }

    #[test]
    fn ordering_drops_loopback_before_port_allows() {
        // The loopback drops MUST precede the 443/53 accepts, else 443/53 to a
        // host-loopback service would be allowed.
        let rs = build_ruleset(999);
        let lo = rs.find("127.0.0.0/8 drop").expect("loopback rule present");
        let p443 = rs.find("tcp dport 443").expect("443 rule present");
        assert!(lo < p443, "loopback drop must come before the 443 allow: {rs}");
    }

    /// The 0.45.0 build-box hang: every verdict that can DROP lives in
    /// `cell_policy`, which only the cell's own uid can reach. The base
    /// chain holds the jump and nothing else, so a packet with no socket
    /// (a kernel RST for a closed localhost port) is accepted.
    #[test]
    fn base_chain_only_jumps_for_the_cell_uid() {
        let rs = build_ruleset(60007);
        let base_start = rs.find("chain cell_egress {").expect("base chain");
        let policy_start = rs.find("chain cell_policy {").expect("policy chain");
        assert!(base_start < policy_start);
        let base = &rs[base_start..policy_start];
        let rules: Vec<&str> = base
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("chain") && *l != "}")
            .collect();
        assert_eq!(
            rules,
            vec![
                "type filter hook output priority 0; policy accept;",
                "meta skuid 60007 jump cell_policy",
            ],
            "{rs}"
        );
        let policy = &rs[policy_start..];
        assert!(policy.contains("ip daddr 127.0.0.0/8 drop"));
        assert!(policy.contains("counter drop\n\t}"), "policy ends in the fail-closed drop: {rs}");
    }

    #[test]
    fn listing_parse_finds_only_sandbox_tables() {
        let listing = "table ip filter\n\
                       table inet k2sandbox_60001\n\
                       table ip nat\n\
                       table inet k2sandbox_60000\n\
                       table inet k2sandbox_nope\n\
                       table ip k2sandbox_60009\n\
                       table inet k2sandbox_60001\n";
        assert_eq!(sandbox_table_uids(listing), vec![60000, 60001]);
        assert!(sandbox_table_uids("").is_empty());
    }

    #[test]
    fn stale_means_not_allocated_and_no_process() {
        let live: std::collections::HashSet<u32> = [60002].into_iter().collect();
        assert_eq!(
            stale_uids(&[60000, 60001, 60002, 60003], &[60001], &live),
            vec![60000, 60003],
            "allocated (booting) and process-owning uids keep their tables"
        );
    }

    #[test]
    fn live_process_uids_include_this_process() {
        let me = unsafe { libc::geteuid() };
        assert!(live_process_uids().expect("read /proc").contains(&me));
    }

    /// Live nft check (root + nft only, so it is `#[ignore]`d and on the
    /// gate allowlist; `scripts/test-gate-linux.sh` runs it explicitly as
    /// root): with a cell table installed, a connect to a CLOSED localhost
    /// port is refused at once instead of hanging. Uses a uid at the top of
    /// the pool range that no cell uses, and removes the table after.
    #[test]
    #[ignore = "needs root + nft; run by scripts/test-gate-linux.sh"]
    fn refused_localhost_port_is_refused_while_a_cell_table_exists() {
        let uid = 60127;
        ensure_egress_policy(uid).expect("install cell table (root + nft)");
        struct Remove(u32);
        impl Drop for Remove {
            fn drop(&mut self) {
                remove_egress_policy(self.0).expect("remove cell table");
            }
        }
        let _remove = Remove(uid);
        // A port that was bound and released: closed, and ours.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
            l.local_addr().expect("addr").port()
        };
        let t = std::time::Instant::now();
        let err = std::net::TcpStream::connect_timeout(
            &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
            std::time::Duration::from_secs(5),
        )
        .expect_err("nothing listens there");
        assert_eq!(
            err.kind(),
            std::io::ErrorKind::ConnectionRefused,
            "a closed localhost port must be REFUSED (got {err:?} after {:?})",
            t.elapsed()
        );
        assert!(t.elapsed() < std::time::Duration::from_secs(1), "{:?}", t.elapsed());
    }
}
