//! Skill upgrade protocol (universal).
//!
//! Every generated SKILL.md is wrapped with frontmatter
//! (`k2_skill`, `skill_version`, `skill_checksum`) and MANAGED
//! markers. On startup, [`ensure_skill_up_to_date`] compares the
//! stamped version + checksum to the current generator output; if
//! the managed region is unmodified we rewrite it in place when the
//! generator version advances, and if the user has edited it we drop
//! the new version alongside as `.proposed` instead of stomping their
//! work.
//!
//! **Bumping any `SKILL_VERSION_*` constant forces every workspace's
//! next startup to re-evaluate that tier.** That's the whole point:
//! ship a better skill, bump the constant, it rolls out automatically
//! to all unmodified files.
//!
//! Four-tier system matching the Agent Skills Settings UI:
//!
//! - [`SKILL_VERSION_MANAGER`] — Workspace Manager (workspace's manager-mode primary agent).
//! - [`SKILL_VERSION_K2SO_AGENT`] — K2SO planner agent (PRDs, etc.).
//! - [`SKILL_VERSION_CUSTOM_AGENT`] — Custom mode single-agent.
//! - [`SKILL_VERSION_TEMPLATE`] — Sub-agent template delegated to by
//!   a manager.
//! - [`SKILL_VERSION_WORKSPACE`] — version tier for the loadable
//!   `k2-cli` skill that ships with every workspace.

use std::fs;
use std::path::Path;

use crate::fs_atomic::atomic_write_str;

// ── Managed region markers ────────────────────────────────────────────

pub const SKILL_BEGIN_MARKER: &str = "<!-- K2:MANAGED:BEGIN -->";
pub const SKILL_END_MARKER: &str = "<!-- K2:MANAGED:END -->";

// ── Version constants ────────────────────────────────────────────────

/// Bumped to 2 in 0.39.0: Phase 2.1 A25 canonical CLI verbs replace
/// the pre-Phase-2 deprecated surface (`k2so delegate`, `work *`,
/// `agents create`, etc.). All four tiers re-roll on next startup so
/// unmodified SKILL.md files stop telling agents to run dead commands.
///
/// Manager / K2SO-agent / custom-agent bumped to 3 in 0.40.x: skills
/// now teach `feedback ask` (durable agent→human questions) and the
/// `project` surface (project-group shared chat + the
/// `[project:<name>]` reply rule).
///
/// Manager / K2SO-agent / custom-agent bumped to 4 in 0.40.30: skills
/// now teach the API read-back contract — "Respond to your API caller"
/// (`k2 respond` / `k2 respond --final` when the session was launched
/// through the K2 API and `K2_HOOK_TOKEN` is staged).
///
/// Manager / K2SO-agent / custom-agent bumped to 5 in 0.40.x (K2 Mail
/// S8): skills now teach the `k2 mail` surface — minting, reading with
/// the untrusted-content markers, the `wait` loop pattern, and send
/// governance (off | approval | on; queued IS success).
///
/// Manager / K2SO-agent / custom-agent bumped to 6 in 0.40.42 (K2 Mail
/// OAuth, O6): the assistant-inbox note now covers OAuth-linked Gmail /
/// Microsoft accounts (human-only setup; the agent never runs the flow)
/// and corrects the linked-send story — drafting is always available,
/// sending needs a human-granted `send` level, and OAuth inboxes stay
/// draft-only in Phase 1.
///
/// Manager / K2SO-agent / custom-agent bumped to 7 in 0.40.42 (SMTP
/// XOAUTH2 landed): a Gmail-OAuth inbox now sends over SMTP at the
/// `send` level just like an app-password inbox — only Microsoft-OAuth
/// stays draft-only (until Graph send lands). The assistant-inbox note
/// corrects the "OAuth inboxes are draft-only" claim accordingly.
pub const SKILL_VERSION_MANAGER: u32 = 8;
pub const SKILL_VERSION_K2SO_AGENT: u32 = 8;
pub const SKILL_VERSION_CUSTOM_AGENT: u32 = 8;
pub const SKILL_VERSION_TEMPLATE: u32 = 3;
/// K2 Canonical Agent skill (canonical-agents feature). v1 = first ship.
/// v2 = authored persona file is `.k2/agent/ROLE.md`.
pub const SKILL_VERSION_CANONICAL_AGENT: u32 = 2;
/// Bumped to 5 in 0.39.0: same Phase 2.1 A25 verb refresh applied to
/// the workspace-root template body (Cli tools section, workflow
/// docs). Was 4 since 0.32.7 (SOURCE sub-regions adoption).
/// Bumped to 6 in 0.40.x: the loadable k2-cli skill gains the
/// "Ask a human" (`k2 feedback`) and "Projects" (`k2 project`)
/// sections.
/// Bumped to 7 in 0.40.30: the loadable k2-cli skill gains the
/// "Respond to your API caller" (`k2 respond [--final]`) section.
/// Bumped to 8 in 0.40.30 (W6): the loadable k2-cli skill gains the
/// "Agent presets" section (`k2 preset`, `hire --agent`, and the
/// docs/agent-contract.md pointer for custom/local-LLM agents).
/// Bumped to 9 in 0.40.x: the Projects section gains project-membership
/// management on the agent plane (`k2 agent hire --project`,
/// `set --add-project/--remove-project`, `get <ws> projects`, and the
/// first-member-becomes-PoC rule).
/// (10 was taken on main in 0.40.41.) Bumped to 11 in 0.40.x
/// (K2 Mail S8): the loadable k2-cli skill gains the "Email"
/// (`k2 mail`) section — minting, untrusted-content markers, the
/// `wait` loop, send governance.
/// Bumped to 13 in 0.40.64: k2-cli skill gains a lean "Always-on context"
/// section (`k2 agent context …`, hire `--context`).
/// Bumped to 14: Who am I (`k2 whoami`) + heartbeat sidecar hint (D6/D22).
/// Bumped to 15: `--set sales/reviewer` for heartbeat delivery.
/// Bumped to 16: Who am I is a fact sheet (not “msg the primary”);
/// send-work uses handles + sidecar addresses.
/// Bumped to 17: `k2 connections list --users` (humans on this box).
pub const SKILL_VERSION_WORKSPACE: u32 = 17;

// ── Content checksumming ─────────────────────────────────────────────

/// 64-bit FNV-1a hex. Deterministic across Rust versions (unlike
/// `DefaultHasher`), so a checksum written today still matches its
/// content read from disk months later. Not cryptographic — we only
/// need "has this text changed" detection, not adversarial integrity.
pub fn skill_checksum_hex(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in bytes {
        h ^= *b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", h)
}

// ── Wrap + parse ─────────────────────────────────────────────────────

/// Build the final file contents for a generated skill. `body` is the
/// raw generator output (H1 + sections); this function wraps it with
/// upgrade-tracking frontmatter, managed markers, and a user-editable
/// tail placeholder.
///
/// `extra_frontmatter` is appended to the managed frontmatter block —
/// used by the harness-canonical writer to add `name:` / `description:`
/// fields that Claude Code and Pi expect, without losing our upgrade
/// metadata.
pub fn wrap_managed_skill(
    skill_type: &str,
    version: u32,
    body: &str,
    extra_frontmatter: Option<&str>,
) -> String {
    let trimmed = body.trim();
    let checksum = skill_checksum_hex(trimmed.as_bytes());
    let extras = extra_frontmatter
        .map(|s| format!("\n{}", s.trim_end()))
        .unwrap_or_default();
    format!(
        "---\nk2_skill: {skill_type}\nskill_version: {version}\nskill_checksum: {checksum}{extras}\n---\n\n{begin}\n{trimmed}\n{end}\n\n<!-- Content below this line is yours — K2 will never modify it. -->\n",
        begin = SKILL_BEGIN_MARKER,
        end = SKILL_END_MARKER,
    )
}

pub struct ParsedSkill {
    pub k2_skill: Option<String>,
    pub skill_version: Option<u32>,
    pub skill_checksum: Option<String>,
    /// Frontmatter lines OTHER than our upgrade keys — preserved on
    /// rewrite so harness-specific fields like `name:` / `description:`
    /// survive unchanged.
    pub extra_frontmatter: String,
    /// The trimmed bytes between the two markers. `None` when the
    /// file has no markers (legacy, pre-upgrade-protocol) or we
    /// couldn't find both markers.
    pub managed_region: Option<String>,
    /// Everything after the closing marker (user tail).
    pub after_end: String,
    pub has_markers: bool,
}

pub fn parse_skill(content: &str) -> ParsedSkill {
    let mut parsed = ParsedSkill {
        k2_skill: None,
        skill_version: None,
        skill_checksum: None,
        extra_frontmatter: String::new(),
        managed_region: None,
        after_end: String::new(),
        has_markers: false,
    };

    // Frontmatter — extract our upgrade keys + preserve the rest.
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let fm_block = &content[3..3 + end];
            let mut extras = String::new();
            for line in fm_block.lines() {
                if let Some((key, value)) = line.split_once(':') {
                    let k = key.trim();
                    let v = value.trim();
                    match k {
                        "k2_skill" => parsed.k2_skill = Some(v.to_string()),
                        "skill_version" => parsed.skill_version = v.parse().ok(),
                        "skill_checksum" => parsed.skill_checksum = Some(v.to_string()),
                        _ if !k.is_empty() && !v.is_empty() => {
                            extras.push_str(&format!("{}: {}\n", k, v));
                        }
                        _ => {}
                    }
                }
            }
            parsed.extra_frontmatter = extras.trim_end().to_string();
        }
    }

    // Managed-region extraction.
    if let Some(begin_idx) = content.find(SKILL_BEGIN_MARKER) {
        if let Some(end_rel) = content[begin_idx..].find(SKILL_END_MARKER) {
            parsed.has_markers = true;
            let region_start = begin_idx + SKILL_BEGIN_MARKER.len();
            let region_end = begin_idx + end_rel;
            parsed.managed_region =
                Some(content[region_start..region_end].trim().to_string());
            let after_end_start = region_end + SKILL_END_MARKER.len();
            parsed.after_end = content[after_end_start..].to_string();
        }
    }

    parsed
}

// ── Universal upgrade step ────────────────────────────────────────────

#[derive(Debug)]
pub enum SkillUpgradeOutcome {
    Created,
    UpToDate,
    Upgraded,
    MigratedLegacy,
    UserModified,
}

fn log_if_err<E: std::fmt::Display>(label: &str, path: &Path, res: Result<(), E>) {
    if let Err(e) = res {
        log_debug!("[{}] {}: {}", label, path.display(), e);
    }
}

/// The universal upgrade step. Every skill writer routes through this
/// — no per-skill one-off ensure/migrate helpers.
///
/// - missing file → create with wrapped body
/// - current version AND type match → no-op (file on disk is fine)
/// - no markers → legacy file; wrap the new content ABOVE existing
///   content (preserving user's custom content, if any, below)
/// - markers + checksum match → rewrite managed region, preserve tail
/// - markers + checksum differs → user edited, emit `.proposed`
///   sibling instead of overwriting
pub fn ensure_skill_up_to_date(
    skill_path: &Path,
    skill_type: &str,
    current_version: u32,
    fresh_body: &str,
    extra_frontmatter: Option<&str>,
) -> SkillUpgradeOutcome {
    if let Some(parent) = skill_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if !skill_path.exists() {
        let wrapped =
            wrap_managed_skill(skill_type, current_version, fresh_body, extra_frontmatter);
        log_if_err(
            "ensure_skill_up_to_date create",
            skill_path,
            atomic_write_str(skill_path, &wrapped),
        );
        return SkillUpgradeOutcome::Created;
    }

    let existing = fs::read_to_string(skill_path).unwrap_or_default();
    let parsed = parse_skill(&existing);

    // Fast path: already on the current contract.
    if parsed.has_markers
        && parsed.k2_skill.as_deref() == Some(skill_type)
        && parsed.skill_version == Some(current_version)
    {
        return SkillUpgradeOutcome::UpToDate;
    }

    // Legacy: file has no markers at all. Two sub-cases:
    //   (a) our own pre-0.32.4 generator output (replace entirely —
    //       keeping it would duplicate the content we're about to
    //       write), or
    //   (b) user-custom content with no K2 signature (preserve as
    //       tail so nothing is lost).
    // Distinguish by the first H1 after any legacy frontmatter:
    // starts with "# K2 " → ours, otherwise user content.
    if !parsed.has_markers {
        let after_fm: &str = if existing.starts_with("---") {
            existing[3..]
                .find("---")
                .map(|end| {
                    existing[3 + end + 3..]
                        .trim_start_matches(|c: char| c.is_whitespace())
                })
                .unwrap_or(&existing)
        } else {
            existing.trim_start_matches(|c: char| c.is_whitespace())
        };
        let first_h1 = after_fm.lines().find(|l| l.starts_with("# ")).unwrap_or("");
        let is_our_legacy_output = first_h1.starts_with("# K2 ");

        let wrapped =
            wrap_managed_skill(skill_type, current_version, fresh_body, extra_frontmatter);
        let final_content = if is_our_legacy_output {
            wrapped
        } else if after_fm.trim().is_empty() {
            wrapped
        } else {
            format!("{}\n{}\n", wrapped.trim_end(), after_fm.trim_end())
        };
        log_if_err(
            "ensure_skill_up_to_date migrate legacy",
            skill_path,
            atomic_write_str(skill_path, &final_content),
        );
        return SkillUpgradeOutcome::MigratedLegacy;
    }

    // Markers present. Compare checksum of the current managed region
    // against the stamped checksum. Match → safe auto-upgrade.
    let actual_checksum = skill_checksum_hex(
        parsed
            .managed_region
            .as_deref()
            .unwrap_or("")
            .trim()
            .as_bytes(),
    );
    let stamped = parsed.skill_checksum.as_deref().unwrap_or("");
    if actual_checksum == stamped {
        let wrapped =
            wrap_managed_skill(skill_type, current_version, fresh_body, extra_frontmatter);
        let tail = parsed.after_end.trim();
        let final_content = if tail.is_empty() {
            wrapped
        } else {
            format!("{}\n{}\n", wrapped.trim_end(), tail)
        };
        log_if_err(
            "ensure_skill_up_to_date upgrade",
            skill_path,
            atomic_write_str(skill_path, &final_content),
        );
        return SkillUpgradeOutcome::Upgraded;
    }

    // User has modified the managed region. Don't overwrite — drop
    // the proposed new version next to the file so the user can diff
    // and merge when they're ready.
    let proposed_path = skill_path.with_extension("md.proposed");
    let wrapped = wrap_managed_skill(skill_type, current_version, fresh_body, extra_frontmatter);
    log_if_err(
        "ensure_skill_up_to_date propose",
        &proposed_path,
        atomic_write_str(&proposed_path, &wrapped),
    );
    log_debug!(
        "[skill-upgrade] {} user-modified; wrote {} alongside",
        skill_path.display(),
        proposed_path.display()
    );
    SkillUpgradeOutcome::UserModified
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_deterministic() {
        assert_eq!(skill_checksum_hex(b"hello"), skill_checksum_hex(b"hello"));
        assert_ne!(skill_checksum_hex(b"hello"), skill_checksum_hex(b"hellox"));
    }

    #[test]
    fn wrap_roundtrips_through_parse() {
        let body = "# K2 Agent: foo\n\nBody content.";
        let wrapped = wrap_managed_skill("custom_agent", 7, body, None);
        let parsed = parse_skill(&wrapped);
        assert_eq!(parsed.k2_skill.as_deref(), Some("custom_agent"));
        assert_eq!(parsed.skill_version, Some(7));
        assert!(parsed.has_markers);
        assert_eq!(parsed.managed_region.as_deref(), Some(body));
    }

    #[test]
    fn wrap_preserves_extra_frontmatter_keys() {
        let wrapped = wrap_managed_skill(
            "agent_template",
            1,
            "body",
            Some("name: frontend-eng\ndescription: react + ts"),
        );
        let parsed = parse_skill(&wrapped);
        assert!(parsed.extra_frontmatter.contains("name: frontend-eng"));
        assert!(parsed.extra_frontmatter.contains("description: react + ts"));
        // Upgrade keys should NOT appear in extras.
        assert!(!parsed.extra_frontmatter.contains("skill_version"));
        assert!(!parsed.extra_frontmatter.contains("k2_skill"));
    }

    #[test]
    fn parse_skill_no_markers_returns_has_markers_false() {
        let parsed = parse_skill("# Heading\nNo fences here.");
        assert!(!parsed.has_markers);
        assert!(parsed.managed_region.is_none());
    }

    #[test]
    fn managed_markers_use_k2_brand() {
        // Brand cutover: markers carry the `K2:` prefix, not `K2SO:`.
        assert_eq!(SKILL_BEGIN_MARKER, "<!-- K2:MANAGED:BEGIN -->");
        assert_eq!(SKILL_END_MARKER, "<!-- K2:MANAGED:END -->");
    }

    #[test]
    fn frontmatter_key_is_k2_skill() {
        // The upgrade-tracking frontmatter key is `k2_skill` on write.
        let wrapped = wrap_managed_skill("workspace", 1, "# K2 body", None);
        assert!(wrapped.contains("k2_skill: workspace"));
        assert!(!wrapped.contains("k2so_skill"));
    }

    #[test]
    fn version_constants_exported() {
        // Guardrail: these are part of the public API — dropping one
        // would break every caller. Test just reads them to prevent
        // accidental removal.
        let _ = SKILL_VERSION_MANAGER;
        let _ = SKILL_VERSION_K2SO_AGENT;
        let _ = SKILL_VERSION_CUSTOM_AGENT;
        let _ = SKILL_VERSION_TEMPLATE;
        let _ = SKILL_VERSION_WORKSPACE;
    }
}
