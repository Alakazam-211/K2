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
//! first rule `meta skuid != <uid> accept` lets all non-cell traffic pass
//! UNTOUCHED, so the daemon's own egress, owner traffic, and the box's services
//! are unaffected. The vsock cell-channel is AF_VSOCK/AF_UNIX, not INET, so this
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
//! packet is delivered only if NO chain drops it. Each chain's first rule
//! (`meta skuid != <uid> accept`) passes every OTHER uid's traffic untouched, so
//! cell A's packet is policed by chain A (default-DROP unless 443/53) while
//! chain B simply accepts it — the per-uid tables stack without interfering, and
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

/// The PER-SESSION table name for `cell_uid` — `k2sandbox_<uid>`. Namespaced so
/// we never collide with a box's existing firewall, and per-uid so install +
/// teardown target EXACTLY this cell's state without touching any other cell.
#[cfg(target_os = "linux")]
fn table_name(cell_uid: u32) -> String {
    format!("k2sandbox_{cell_uid}")
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
/// Rule order (THIS cell's uid ONLY — every other uid is accepted untouched by
/// the first rule, so the daemon/owner/root AND other cells are never affected;
/// each other cell has its OWN per-uid table that polices it):
///   1. non-this-uid       → accept (pass through, do NOT police)
///   2. loopback dst        → drop  (no host-local services, incl. the daemon's
///                                  loopback TCP listener)
///   3. tcp dport 443       → accept (Anthropic / LLM, TLS) to non-loopback
///   4. udp/tcp dport 53    → accept (DNS) to non-loopback
///   5. (fall-through)      → drop  (fail-closed: all other cell egress)
/// The chain policy is `accept` ONLY so rule 1 can pass NON-cell traffic; a
/// packet from this cell's uid never reaches the policy — it always hits 2/3/4/5.
#[cfg(target_os = "linux")]
fn build_ruleset(cell_uid: u32) -> String {
    let table = table_name(cell_uid);
    format!(
        "add table inet {table}\n\
         delete table inet {table}\n\
         table inet {table} {{\n\
         \tchain {CHAIN} {{\n\
         \t\ttype filter hook output priority 0; policy accept;\n\
         \t\tmeta skuid != {uid} accept\n\
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
    run_nft(&ruleset)
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

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn ruleset_is_fail_closed_and_uid_scoped() {
        let rs = build_ruleset(999);
        // uid-scoped: non-k2cell passes untouched.
        assert!(
            rs.contains("meta skuid != 999 accept"),
            "must let non-k2cell traffic pass untouched: {rs}"
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
        assert!(a.contains("meta skuid != 60000 accept"));
        assert!(b.contains("meta skuid != 60001 accept"));
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
}
