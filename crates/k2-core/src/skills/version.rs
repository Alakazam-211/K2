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
///
/// Manager / K2SO-agent / custom-agent bumped to 9: skills teach the
/// `k2 db` / `k2 store` sidecar (Linux; fail-closed `db_agent_access`;
/// owner enable; create/dsn/migrate/dump/store).
/// Bumped to 10: restore + publish-subdomain off-box hint (D30).
/// Bumped to 11: `db_agent_access` is create-only; list/dsn/store use
/// ownership or sql_grants.
/// Bumped to 12 (0.43.2, ticket HTML brief): "Ask a human" teaches
/// `k2 tickets template` + `ask --html` and `k2 study ticket-brief`.
/// Bumped to 13 (0.43.2, assignee policy): "Ask a human" teaches
/// `--assign <user>` (a user on this server; required in a future update).
pub const SKILL_VERSION_MANAGER: u32 = 13;
pub const SKILL_VERSION_K2SO_AGENT: u32 = 13;
pub const SKILL_VERSION_CUSTOM_AGENT: u32 = 13;
pub const SKILL_VERSION_TEMPLATE: u32 = 3;
/// K2 Canonical Agent skill (canonical-agents feature). v1 = first ship.
/// v2 = authored persona file is `.k2/agent/ROLE.md`.
pub const SKILL_VERSION_CANONICAL_AGENT: u32 = 2;
/// `k2-zen` skill (prd-zen-mode-v1 Z18). Written only when this daemon
/// has `~/.k2/zen/` (Z68). v1 = first ship. v2 = theme bundles, `[font]`,
/// terminal palette, backgrounds, `k2 zen theme` (Omarchy additions).
/// v3 = Zen Gardens: `k2 zen garden`, `--garden`, layout + built-in widgets.
/// v4 = setup makes Garden 1 + Garden 2; two required controls.
/// v5 = the `nav-rail` widget (Garden 1's left rail) and the `app:navigate` verbs.
/// v6 = the rail switches Garden 1's view inside Zen (no longer leaves Zen);
/// `app.current` / `app.subscribeCurrent`, `focusGroups.*`.
/// v7 = `k2 zen garden template` + Start with the default (`gardens:template`); the `default` theme is `basic`.
/// v8 = `[[widget]] slot = "top"`: the nav rail as a row in the top band; `nav-rail` `orientation`.
/// v9 = freeform chrome (prd-zen-freeform-chrome FC38): Zen controls are
/// widgets; `slot` `bottom`/`menu`, `align`, `edge`, `menu`; the `menu` kind.
/// v10 = `k2 zen guide` (prd-zen-freeform-chrome FC33–FC35): the user guide's
/// pages and pipeable example files.
/// v11 = `agents.list()` / `agents.subscribe(cb)` rows carry the daemon's
/// `activity` (prd-daemon-activity-and-thread-working-v1 S5).
/// v12 = custom widgets (prd-zen-user-widgets-v2 UW42, UWB2, UWB25): the
/// folder, manifest, placing, "you can't grant", the `k2` object, and
/// "what's available"; the verb lists come from the verb catalog. Garden
/// sync's text (synced vs own copy, `k2 zen sync` / `k2 zen news`, never
/// write sync.json/news.json/.defaults/) rides the same v12.
/// v13 = using the whole canvas (0.45.3): `k2.canvas.setHitRegions` and
/// `k2.window.startDrag` for full-canvas Gardens.
/// v14 = `agents.list()` / `agents.subscribe(cb)` rows carry the agent's
/// `color` and `image` (0.45.5 agent identity; catalog version 5).
/// v15 = locked Gardens (prd-garden-lock-and-sync-v1 §6): "A locked Garden:
/// don't edit it. Ask the person."; `.locks/` joins the never-write list.
/// v16 = Garden v3 data sources (contract-sources-v1 §6, §11.4): the
/// `sources` manifest key, `sources:read` (and that it drops `thread:post`),
/// the `sources.*` verbs, the five built-in cards and their presets,
/// `k2 zen widget show`, and runtime errors without text for cards that
/// read sources (catalog version 6).
/// v17 = Thread choice cards from `thread.read` / `thread.subscribe` carry
/// `answeredBy` (`{kind, login, name}`) and `expiresAt` (catalog version 7).
/// v18 = Garden data (prd-gardens-local-services-v1 §7, GL39;
/// contract-garden-data-v1 §11.4): `k2.store` for data (never inlined),
/// agents' `k2 store --widget`, validate --widget signs, project grants are
/// a convenience, the grant admin's bounds, `bin/board`, Lock keeps data
/// writable, and the board-lite example.
/// v19 = Garden 1 from K2 Companion (0.45.6): the Agents widget's `pins`
/// and `settings` props and the Conversation's `activity-drawer`.
/// v20 = the Kanban Garden (0.45.7): New Garden lists it, `k2:kanban` in the
/// examples, and its board's keys (`board/<garden id>/col|card/<id>`) for
/// agents writing with `k2 store --widget k2:kanban`.
/// v21 = the widget dev loop (0.46.1 S3): keep a widget outside `~/.k2`
/// (`validate --dir`, `widget preview --dir`, `widget put --dir`, a staged
/// Garden page with `validate --garden --from`); validate signs only with
/// `--sign` and "pending" exits 0 (`--strict` keeps 2).
/// v22 = Garden polish (0.46.1, Rosson 2026-10-17): a sources card keeps
/// `thread:post` / `store:write` it asks for (validate notes the reach),
/// manifest `reads` + `{from}` for a neighbour's data, a maker agent with
/// the Gardens switch reaches its own widget's data, 10 custom widgets per
/// page and `[[widget]] weight` (catalog version 10).
/// v23 = the widget SDK (0.46.1, catalog 11): typed K2Error with
/// `retryable`, handler-object subscriptions that re-subscribe on their own,
/// `k2.shape` guards, `bad_args` / `bad_shape`, and pinned chart libraries
/// (`uplot@1.6.32`).
/// v24 = the `k2:*@2` data-source cards (0.46.1): several sources per card
/// (`config.sources`), soft crit, named lists, K2 Web's ops data.
/// v25 = Thread read state in Zen (0.46.1, thread-read-v1): the Agents
/// widget's `unread` prop and the Conversation's `mute` prop (Garden 1 turns
/// it on); conversations mark their Thread read while looked at.
/// v26 = the Kanban's Thread panel (k2:kanban@2, 0.46.1): K2's Thread look
/// in K2 Companion's colours, `look = "garden"` to follow the Garden theme,
/// a synced Kanban Garden moves to @2 and an own copy keeps @1.
/// v27 = k2:kanban@3 (0.46.3): the Kanban's Thread panel never moves a
/// reader who scrolled up ("new messages" pill); a synced Kanban Garden
/// moves to @3, an own copy keeps its board.
/// v28 = k2:kanban@4 (0.46.3 Thread parity): the Kanban's Thread panel shows
/// a message's files and pictures, links workspace paths and takes files
/// with + ; a synced Kanban Garden moves to @4, an own copy keeps its board.
/// v29 = Garden gaps (0.46.3): a folder put without the switch is ONE
/// proposal for the whole folder; a `k2.sources.subscribe(address, cb,
/// onError?)` K2 refused for a retryable reason subscribes again by itself;
/// frames follow the Garden's light/dark (manifest `colorScheme: "own"`
/// opts out); an agent's change to the person's widget sends a "Keep as
/// mine" card on the person's Thread with that agent (inbox note without one).
/// v30 = Garden columns and the agent list (0.46.3 r10, catalog v20):
/// `[[layout.column]]` `collapsible` / `collapsed` / `label`, the Agents
/// widget's `list-from` (mode `list`) with `k2.agents.show`, `k2.layout.*`
/// and the visible context chip (`k2.conversation.setContext`).
/// v31 = Garden access is how Gardens work (0.46.3 r11, Rosson 2026-10-23:
/// no switch): the retired verbs answer with a notice until 0.47.0 and
/// `k2 study garden-changes` says what to update; a widget's data lives
/// under its Garden in Settings → Gardens.
/// v32 = the Conversation's `pick` (Fleet Ops 2, Rosson 2026-10-24): 2 to 8
/// agents, and the header is the drop-down that switches between them.
/// v33 = `pick = "all"` (any agent, a search field; beside a custom widget
/// they share one pick) and the Kanban on `k2:kanban@5`: K2's own
/// Conversation in a folded "Thread" column, Send board to the agent it
/// shows (Rosson 2026-10-24, for 0.46.4).
pub const SKILL_VERSION_ZEN: u32 = 33;
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
/// Bumped to 18: `k2 agent context add users:roster` (opt-in people layer).
/// Bumped to 19: overlay `k2 thread` (text write/read; not PTY).
/// Bumped to 20: overlay `k2 thread ask` / `k2 thread secret` (do not wait).
/// Bumped to 21: loadable k2-cli skill gains Database (`k2 db` / `k2 store`).
/// Bumped to 22: restore + publish-subdomain off-box hint (D30).
/// Bumped to 23: `[thread:<addr>]` reply rule — answer with `k2 thread`,
/// not the TUI (same shape as `[project:<name>]` → `k2 project msg`).
/// Bumped to 24: `k2 agent context add skin:roster` (opt-in Skin Access guests).
/// Bumped to 25: `db_agent_access` is create-only; list/dsn/store use
/// ownership or sql_grants.
/// Bumped to 26: hire `--focus-group` / `--color` (hex or rgb).
/// Bumped to 27: thread bodies render markdown (CLI nudge + skill).
/// Bumped to 28: human word app (`k2 app` / `k2 study apps` / `apps:roster`);
/// leftover `k2 skin` / `k2 skin-token` / `skin:roster`; pass prefix still `k2skn_`.
/// Bumped to 29 (0.43.2, ticket HTML brief): "Ask a human" teaches
/// `k2 tickets template` + `ask --html` and `k2 study ticket-brief`.
/// Bumped to 30 (0.43.2, assignee policy): "Ask a human" teaches
/// `--assign <user>` and `k2 tickets assign`.
/// Bumped to 31 (k2 sidecar v1): the k2-cli skill gains a Sidecars block.
/// Bumped to 32 (0.45.5, Thread cards): prose never closes a card; agents
/// void their own obsolete cards with `k2 thread void`.
/// Bumped to 33 (0.46.1): "Your picture and color" (`k2 agent identity`).
/// Bumped to 34 (0.46.1, thread-attachments-v1): `k2 thread … --file
/// path[:line[-end]]` and the attach-the-files-you-mention rule; the
/// AGENTS.md Tooling line names `--file`.
pub const SKILL_VERSION_WORKSPACE: u32 = 34;

/// The `k2-permissions` skill (permission cards, Beta; PRD rev 3.1 §11.4,
/// vs-live C8): its own constant, so the k2-cli skill (`_WORKSPACE`) and
/// `_ZEN` stay byte-identical in every switch state. Written only while
/// both permission-card switches are on for the workspace.
/// v1 = 0.46.3, the first release.
pub const SKILL_VERSION_PERMISSIONS: u32 = 1;

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

/// [`ensure_skill_up_to_date`] for a body that changes without a version
/// bump (prd-garden-access-v1 G0: the `k2-zen` skill follows the
/// `gardenAccess` switch). Same version and type, but the managed region
/// isn't `fresh_body`: rewrite it when the person didn't edit it (the
/// stamped checksum matches; their tail below the marker stays), else drop
/// the `.proposed` sibling. Everything else is `ensure_skill_up_to_date`.
pub fn refresh_skill_body(
    skill_path: &Path,
    skill_type: &str,
    current_version: u32,
    fresh_body: &str,
    extra_frontmatter: Option<&str>,
) -> SkillUpgradeOutcome {
    let parsed = fs::read_to_string(skill_path).ok().map(|s| parse_skill(&s));
    let same_contract = parsed.as_ref().is_some_and(|p| {
        p.has_markers && p.k2_skill.as_deref() == Some(skill_type) && p.skill_version == Some(current_version)
    });
    let Some(parsed) = parsed.filter(|_| same_contract) else {
        return ensure_skill_up_to_date(skill_path, skill_type, current_version, fresh_body, extra_frontmatter);
    };
    let region = parsed.managed_region.as_deref().unwrap_or("").trim();
    if region == fresh_body.trim() {
        return SkillUpgradeOutcome::UpToDate;
    }
    let wrapped = wrap_managed_skill(skill_type, current_version, fresh_body, extra_frontmatter);
    if skill_checksum_hex(region.as_bytes()) != parsed.skill_checksum.as_deref().unwrap_or("") {
        let proposed_path = skill_path.with_extension("md.proposed");
        log_if_err("refresh_skill_body propose", &proposed_path, atomic_write_str(&proposed_path, &wrapped));
        return SkillUpgradeOutcome::UserModified;
    }
    // The person's tail, without the "yours" line `wrap_managed_skill`
    // writes again (a flip back and forth never grows the file).
    const YOURS: &str = "<!-- Content below this line is yours — K2 will never modify it. -->";
    let tail = parsed.after_end.trim();
    let tail = tail.strip_prefix(YOURS).unwrap_or(tail).trim();
    let final_content = if tail.is_empty() { wrapped } else { format!("{}\n{}\n", wrapped.trim_end(), tail) };
    log_if_err("refresh_skill_body", skill_path, atomic_write_str(skill_path, &final_content));
    SkillUpgradeOutcome::Upgraded
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
