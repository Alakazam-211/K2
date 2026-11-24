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
//! - [`SKILL_VERSION_K2_AGENT`] — K2 planner agent (PRDs, etc.).
//! - [`SKILL_VERSION_CUSTOM_AGENT`] — Custom mode single-agent.
//! - [`SKILL_VERSION_TEMPLATE`] — Sub-agent template delegated to by
//!   a manager.
//! - [`SKILL_VERSION_WORKSPACE`] — version tier for the loadable
//!   `k2-cli` skill that ships with every workspace.
//!
//! **The brand is part of the key too.** The text names the command an
//! agent runs and the software it talks about, two separate words of the
//! brand file: the command (`[cli]`: its name and its aliases with their
//! modes) and the product (`[names] product`). A file is current only when
//! it was written for this build's two as well as this version, so a brand
//! change alone rewrites every unmodified file, like a version bump. Their
//! key is stamped as `skill_brand`, except for the brand every file was
//! written under before the stamp existed (product `K2`, command `k2` with
//! `k2so` silent): that build writes no stamp, so its files stay as they
//! were, byte for byte, and a file with no stamp is read as written for
//! that brand.

use std::fs;
use std::path::Path;
use std::sync::OnceLock;

use crate::brand::{AliasMode, CLI_ALIAS_MODES, CLI_NAME, PRODUCT_NAME};
use crate::fs_atomic::atomic_write_str;

// ── Managed region markers ────────────────────────────────────────────

pub const SKILL_BEGIN_MARKER: &str = "<!-- K2:MANAGED:BEGIN -->";
pub const SKILL_END_MARKER: &str = "<!-- K2:MANAGED:END -->";

/// The line [`wrap_managed_skill`] writes under the managed region: what a
/// person writes below it is theirs. It names no product, so a person sees
/// the same line in every brand's build.
pub const SKILL_YOURS_LINE: &str =
    "<!-- Content below this line is yours: it is never changed. -->";

/// The line files were written with before [`SKILL_YOURS_LINE`]. It is
/// still read as the same line, in every brand, and never written: the
/// next upgrade of such a file writes the new one.
pub const SKILL_YOURS_LINE_OLD: &str =
    "<!-- Content below this line is yours — K2 will never modify it. -->";

/// Every "yours" line a file may hold, the one written first.
pub const SKILL_YOURS_LINES: [&str; 2] = [SKILL_YOURS_LINE, SKILL_YOURS_LINE_OLD];

// ── Version constants ────────────────────────────────────────────────

/// Bumped to 2 in 0.39.0: Phase 2.1 A25 canonical CLI verbs replace
/// the pre-Phase-2 deprecated surface (`k2 delegate`, `work *`,
/// `agents create`, etc.). All four tiers re-roll on next startup so
/// unmodified SKILL.md files stop telling agents to run dead commands.
///
/// Manager / K2-agent / custom-agent bumped to 3 in 0.40.x: skills
/// now teach `feedback ask` (durable agent→human questions) and the
/// `project` surface (project-group shared chat + the
/// `[project:<name>]` reply rule).
///
/// Manager / K2-agent / custom-agent bumped to 4 in 0.40.30: skills
/// now teach the API read-back contract — "Respond to your API caller"
/// (`k2 respond` / `k2 respond --final` when the session was launched
/// through the K2 API and `K2_HOOK_TOKEN` is staged).
///
/// Manager / K2-agent / custom-agent bumped to 5 in 0.40.x (K2 Mail
/// S8): skills now teach the `k2 mail` surface — minting, reading with
/// the untrusted-content markers, the `wait` loop pattern, and send
/// governance (off | approval | on; queued IS success).
///
/// Manager / K2-agent / custom-agent bumped to 6 in 0.40.42 (K2 Mail
/// OAuth, O6): the assistant-inbox note now covers OAuth-linked Gmail /
/// Microsoft accounts (human-only setup; the agent never runs the flow)
/// and corrects the linked-send story — drafting is always available,
/// sending needs a human-granted `send` level, and OAuth inboxes stay
/// draft-only in Phase 1.
///
/// Manager / K2-agent / custom-agent bumped to 7 in 0.40.42 (SMTP
/// XOAUTH2 landed): a Gmail-OAuth inbox now sends over SMTP at the
/// `send` level just like an app-password inbox — only Microsoft-OAuth
/// stays draft-only (until Graph send lands). The assistant-inbox note
/// corrects the "OAuth inboxes are draft-only" claim accordingly.
///
/// Manager / K2-agent / custom-agent bumped to 9: skills teach the
/// `k2 db` / `k2 store` sidecar (Linux; fail-closed `db_agent_access`;
/// owner enable; create/dsn/migrate/dump/store).
/// Bumped to 10: restore + publish-subdomain off-box hint (D30).
/// Bumped to 11: `db_agent_access` is create-only; list/dsn/store use
/// ownership or sql_grants.
/// Bumped to 12 (0.43.2, ticket HTML brief): "Ask a human" teaches
/// `k2 tickets template` + `ask --html` and `k2 study ticket-brief`.
/// Bumped to 13 (0.43.2, assignee policy): "Ask a human" teaches
/// `--assign <user>` (a user on this server; required in a future update).
/// Bumped to 14 (template to 4) in 0.46.11: `k2 checkin` is retired, so
/// the tiers teach `inbox`, `organization bulletin` / `update` and `done`.
/// Bumped to 15 (template to 5) in 0.46.13: the skills teach `k2 <verb>`
/// (never the retired `k2so` CLI name) and say "K2", not "K2SO".
/// Bumped to 16 in 0.46.15: the email, database and heartbeat notes name
/// the Settings pages by their current labels (Cores → Email Hosting,
/// Email Link, Cores → Database, the agent's Heartbeats tab).
/// Bumped to 17 in 0.46.17: "Ask a human" shows the repeatable
/// `--option "..."` (a card's `--options` splits on `;`, never a comma).
pub const SKILL_VERSION_MANAGER: u32 = 17;
pub const SKILL_VERSION_K2_AGENT: u32 = 17;
pub const SKILL_VERSION_CUSTOM_AGENT: u32 = 17;
pub const SKILL_VERSION_TEMPLATE: u32 = 5;
/// K2 Canonical Agent skill (canonical-agents feature). v1 = first ship.
/// v2 = authored persona file is `.k2/agent/ROLE.md`.
/// v3 (0.46.13) = says "K2" where it said "K2SO".
pub const SKILL_VERSION_CANONICAL_AGENT: u32 = 3;
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
/// v34 = a widget's `k2.layout.*` fold (still only just after the person's
/// click in it) is kept per Garden on this computer like the person's own,
/// so the Kanban reopens with its Thread as the person left it (Rosson
/// 2026-10-24, for 0.46.4).
/// v35 = a custom widget's code may be 512 KB raw (was 256 KB; the bundle
/// stays 3 MB) (Baden's Board via Appa 2026-10-23, for 0.46.4).
/// v36 = `k2.store.patch` / `k2 store patch --widget`: a field-level merge
/// that keeps the fields it doesn't name, with `ifRev` (catalog v21; Appa
/// 2026-10-23, for 0.46.4).
/// v37 = `k2.open(url)` (catalog v21 `links.open`, cap `links:open`): a link
/// in the person's browser after their click, K2's card the first time per
/// site in each Garden; `k2 zen links` (Baden's Board via Appa 2026-10-23,
/// for 0.46.4).
/// v38 = widgets stacked in a column are separate tiles; `joined = true` on
/// a `[[layout.column]]` keeps one panel (Rosson 2026-10-23, for 0.46.4).
/// v39 = the data-source cards are `@3` (no "Ask an agent") and
/// `k2.sources.ask` answers `retired` until 0.47.0 (Rosson 2026-10-23, for
/// 0.46.4).
/// v40 = the Agents widget's order and pins: the person drags and pins per
/// Garden on this computer (`agents-order.json`); agents read them with
/// `k2 zen agents-order` and never set them (Baden via Appa 2026-10-22, for
/// 0.46.4).
/// v41 = box usage's Garden card `k2:box-disk@1` and the garden-local
/// `app.resources` verb (disk guard Slice B, 0.46.4).
/// v42 = 0.46.4's first start moved the data-source cards from `@1`/`@2` to
/// `@3` once in each Garden file it safely could; locked Gardens and ones
/// with errors keep them and are listed in `k2 zen news` (for 0.46.4).
/// v43 = widget size and shared libraries (prd-widget-size-and-shared-libs-v1,
/// 0.46.5): a widget's version is its code AND its manifest (caps, reads,
/// sources, libraries), so a manifest change needs a sign; an agent not on
/// every placing Garden's list gets `sign_needs_person` and K2's `zen.sign`
/// card (`k2 zen widget sign <w> --ask`); code warns above 512 KB and stops
/// at 1 MB, the page is 3.5 MB; `k2 zen guide libs`; ask for a library's
/// line (`uplot@1`), never a patch.
/// v44 = the activity SDK (0.46.5): `k2.thread.onActivity(address, cb)` gives
/// a widget with thread:read + activity:read the App guest's Thread turn
/// (counts and step kinds, never text); `activity:read` is a bridge cap; the
/// `k2-activity@1` library draws K2's activity bar. Never infer activity
/// from messages.
/// v45 = Garden files and history (0.46.6, prd-garden-board-needs-v1):
/// blobs, `k2.pick` / `k2.onDrop`, `k2.download`, `store.history|restore|
/// snapshot`, the restore echo rule, `restoreSkips`, and the agent's
/// `k2 store history|snapshot|restore --widget` (always a prefix).
/// (Integrator: renumber if another held branch took 45.)
/// v46 = the Board Garden (Rosson 2026-11-01): `k2:board@1` behind the Board
/// core (Beta, off by default; adding the Garden turns it on), its boards in
/// `data:k2:board`, and agents work on them with `k2 board` (never raw
/// `k2 store` writes): the grant line, the cascade, typed refusals, people
/// by stable key. `k2.store.grantees()` (catalog v43).
/// (Integrator: renumber if another held branch took 46.)
/// Bumped to 47 (0.46.8): a context line can't hold `[guest` either.
/// v48 = the Compute Garden (Rosson 2026-11-03): `k2:canvas@1`, its Tiles,
/// and the garden-local `compute.nodes` (cap `compute:nodes`, catalog v49).
/// (Integrator: renumber if another held branch took 48.)
/// v49 = catalog v50 (0.46.9): secret cards in a Garden open K2's own sheet
/// (`k2.thread.answer(address, cardId)`, `doc.secret` is status and expiry
/// only), `k2.thread.download` (K2 saves the original Thread file, no bytes
/// through the widget, `canDownload`), and `k2.agents.end` /
/// `k2.agents.archive` for sidecars.
/// (Integrator: renumber if another held branch took 49.)
/// v50 (0.46.10): K2 is MIT; the widget kit libraries (k2-help, k2-activity,
/// k2-thread, k2-markdown, k2-png, k2-diff) move up one patch version under
/// MIT, so the library list shows new versions and licences.
/// (Integrator: renumber if another held branch took 50.)
pub const SKILL_VERSION_ZEN: u32 = 50;
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
/// Bumped to 35 (0.46.5 activity SDK): the Study topics are the CLI's whole
/// list (review §4.1: 16 of 49 before) and point at `k2 study live`.
/// Bumped to 36 (0.46.5, compute job tiers): the k2-cli skill gains a
/// "Compute nodes" block with the Priority courtesy rule (Priority is
/// optional; one submitter per server at a time, not enforced).
/// Bumped to 37 (0.46.6, mobile development): the Study topics gain `sim`.
/// Bumped to 38 (0.46.6, ticket attachments): `k2 tickets … --file` and how
/// a person's files reach the agent (`Attached: <abs path>`).
/// Bumped to 39 (0.46.6, compute): the Compute nodes block leads with
/// `--dirty` and shows `--repo`/`--with`/`--cache`.
/// Bumped to 40 (0.46.6, box moves carry the vault): the Study topics gain `box-move`.
/// Bumped to 41 (0.46.6, compute job tiers v2): `--mem` always, `--priority
/// --why`, `--tier batch`, `--wait` streams, submitter turns.
/// Bumped to 42 (0.46.6, Sew for Apps): the Study topics gain `app-chat`.
/// Bumped to 43 (0.46.6, compute auto-placement): `k2 compute run` with no
/// node named; K2 picks the node and prints why.
/// Bumped to 44 (0.46.7, Chat / Voice Hosting guide): the Study topics gain
/// `chat-voice` and `chat-voice-rooms`.
/// Bumped to 45 (0.46.7, merge batch B, one bump for all of it): the Study
/// topics gain `publish-visibility` and `publish-dev` (publish visibility),
/// `tauri` and `app-errors` (Tauri core TC-29); `k2 app status`,
/// `k2 explain` and `k2 capabilities`.
/// Bumped to 46 (0.46.7, the multiplayer chain, one bump for all of it): the
/// k2-cli skill gains a "Live docs" block (`k2 doc`, edit live docs through
/// it; live docs are switched per kind of file, `k2 doc cores`, `core_off`).
/// Bumped to 47 (0.46.8): the Study topics gain `voice-rooms` and `board`.
/// Bumped to 48 (0.46.8): the `[guest …]` stamp on an App guest's Thread post.
/// Bumped to 49 (0.46.8): `k2 mail forward-message` in the mail verbs.
/// Bumped to 50 (0.46.8): the Study topics gain `chat-refresh`.
/// Bumped to 51 (0.46.8): the k2-cli skill names `k2 sidecar archive`.
/// Bumped to 52 (0.46.10): K2 is MIT-licensed (was Fair Source, FSL).
/// Bumped to 53 (0.46.11): the Study topics gain `organization` (the Org Chart).
/// Bumped to 54 (0.46.11, prd-popup-close-and-card-dismiss-v1): agents
/// `k2 thread withdraw` their own obsolete cards (`void` is the old name),
/// and a person's dismiss turn means "not needed now", never a denial.
/// Bumped to 55 (0.46.11): the Projects block names `k2 project view`.
/// Bumped to 56 (0.46.12): hiring and sidecars are two switches; the
/// Sessions block names "Let this agent manage its sidecars" and "Let this
/// agent hire agents".
/// Bumped to 57 (0.46.13): the Study topics list names ci-release, ci-stats
/// and agent-templates, as `k2 study` does.
/// Bumped to 58 (0.46.13): the list names provenance too.
/// Bumped to 59 (0.46.13): `k2 thread ask` prints the card's number, and a
/// card's answer names it (`chose "<option>" on card #<seq> ("…")`).
/// Bumped to 60 (0.46.13): the Study topics list names suite-apps.
/// Bumped to 61 (0.46.13): the Study topics list names app-transfers-http.
/// Bumped to 62 (0.46.15): the mail and database notes name the Settings
/// pages by their current labels (Cores → Email Hosting, Cores → Database).
/// Bumped to 63 (0.46.16): the Study topics list names postal.
/// Bumped to 64 (0.46.17): `k2 thread ask` leads with the repeatable
/// `--option "…"` (commas inside are fine); `--options` is for short labels.
/// Bumped to 65 (0.46.17): `--options` splits on `;` only, never a comma;
/// tickets ask shows the repeatable `--option "..."`.
/// Bumped to 66 (0.46.17): a "When the person asks → use" table at the top
/// (reminders, schedules, secrets, decisions, files, telling the person),
/// and K2's reminders and heartbeats, never the harness's own scheduler.
/// Bumped to 67 (0.46.18): `k2 thread <addr> --reply-to '#N' "…"` replies
/// to a message or card, to point the person at it.
/// Bumped to 68 (0.46.18, both 67 lines landed together): the Compute nodes block passes `--expect` (how
/// long the job takes; `unknown` only on a first run).
pub const SKILL_VERSION_WORKSPACE: u32 = 68;

/// The `k2-permissions` skill (permission cards, Beta; PRD rev 3.1 §11.4,
/// vs-live C8): its own constant, so the k2-cli skill (`_WORKSPACE`) and
/// `_ZEN` stay byte-identical in every switch state. Written only while
/// both permission-card switches are on for the workspace.
/// v1 = 0.46.3, the first release.
/// v2 = 0.46.6: Apps gain the password-reset card (`apps.user-password-reset`).
/// v3: "Your own service or App" (`vault.share-service`).
/// v4: "Logins on your own App" (`app.login-grant`).
/// v5 = 0.46.7: published sites' cards (`publish.public`, `publish.pin`).
/// v6 = 0.46.7: "Chat in your own App" (`apps.chat-key`, `sew.space-governance`).
/// v7 = 0.46.8: "Your own chat account" (`chat.manage` first, `chat.account`) and "Email sending"
/// (`mail.send-mode` from `k2 mail sending`, `mail.stop-sending`).
/// v8 = 0.46.8: a box login's access to your own App (`apps.user-grant-box`).
/// v9 = 0.46.8: "Refreshing your own chat" (`chat.refresh`).
/// v10 = 0.46.9: "System packages on compute nodes" (`compute.packages`,
/// `compute.packages-policy`).
/// v11 = 0.46.10: "GitHub access for compute jobs" (`compute.git-access-setup`).
/// v12 = 0.46.10: "A secret another agent sent you" (`vault.incoming.accept`).
/// v13 = 0.46.11: "A person's secret, to another server" (`vault.send`) and
/// "A server secret you need" (`vault.grant-server`).
/// v14 = 0.46.11: the opening line (cards are on by default; who turned on
/// the agent's own switch, by its Settings name).
/// v15 = 0.46.15: "Your compute node list" (`compute.allow-nodes`).
/// v16 = 0.46.15: the switches by their current Settings labels ("Let agents
/// ask with permission cards", "Let this agent refresh its pinned chat").
/// v17 = 0.46.17: `--to` and "Who is asked" (`permission.approvers`), and a
/// login's role moved in place (`k2 app grant role`, `app.login-grant op=role`).
/// v18 = 0.46.17: "Your compute job limits" (`compute.grant`).
/// v19 = 0.46.17: "CI on K2 Compute" (`ci.preview`, `ci.preview-off`).
/// v20 = 0.46.17: "Recording in this server's chat" (`voice.recording-start-level`,
/// `voice.recording-dm`).
/// v21 = 0.46.17: "Focus groups" (`focus-groups.create`, `.rename`, `.assign`).
/// v22 = 0.46.17: "Another workspace's database" (`db.read`: `k2 db browse`,
/// `k2 db request-read`).
/// v23 = 0.46.17: `--assign` (the same flag as `--to`) and "Agents that
/// take your secrets without a card" (`vault.accept-from`).
/// v24 = 0.46.17: a test login's rooms (`apps.test-guest-rooms`,
/// `k2 app user rooms-ok`).
/// v25 = 0.46.17: never raise or answer `vault.owner-confirm` (the owner's
/// own terminal's vault confirm, answered on their phone).
/// v26 = 0.46.17: "LLM tokens" (`llm.tokens-refresh`; `access_expired` needs
/// nothing).
/// v27 = 0.46.18: a site name another workspace holds (`publish.transfer`);
/// "Your compute node list": new nodes join it by themselves, and another
/// workspace's list for an agent that manages it (`compute.allow-nodes-for`);
/// "Your email address cap" (`mail.address-cap`).
/// v28 = 0.46.18: "A compute node's pinned jobs only mode"
/// (`compute.node.pinned-only`).
pub const SKILL_VERSION_PERMISSIONS: u32 = 28;

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

// ── The brand's key ──────────────────────────────────────────────────

/// The key of a brand's names as generated text uses them: the product's
/// name, the command's name, then each alias with its mode, in the brand
/// file's order. The product and the command are separate values (a
/// partner's are unrelated words); a change to any of them is a new key.
pub fn brand_names_key(product: &str, name: &str, aliases: &[(&str, AliasMode)]) -> String {
    let mut text = format!("product={product};name={name}");
    for (alias, mode) in aliases {
        let mode = match mode {
            AliasMode::Silent => "silent",
            AliasMode::Notice => "notice",
            AliasMode::Tombstone => "tombstone",
        };
        text.push_str(&format!(";alias={alias}:{mode}"));
    }
    skill_checksum_hex(text.as_bytes())
}

/// The key of the brand every skill was written under before the key was
/// stamped: product `K2`, command `k2`, `aliases = [{ name = "k2so", mode =
/// "silent" }]`. A file with no `skill_brand` line was written for it, and a
/// build with it writes none.
pub const UNSTAMPED_BRAND_KEY: &str = "2497424e3775b5de";

/// This build's key: its brand's product and `[cli]` table.
pub fn brand_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| brand_names_key(PRODUCT_NAME, CLI_NAME, CLI_ALIAS_MODES))
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
///
/// The frontmatter carries this build's brand key as `skill_brand`, unless
/// it is [`UNSTAMPED_BRAND_KEY`].
pub fn wrap_managed_skill(
    skill_type: &str,
    version: u32,
    body: &str,
    extra_frontmatter: Option<&str>,
) -> String {
    wrap_managed_skill_for(skill_type, version, body, extra_frontmatter, brand_key())
}

/// [`wrap_managed_skill`] for the brand key `brand`, which need not be this
/// build's (tests write another brand's file with it).
pub fn wrap_managed_skill_for(
    skill_type: &str,
    version: u32,
    body: &str,
    extra_frontmatter: Option<&str>,
    brand: &str,
) -> String {
    let trimmed = body.trim();
    let checksum = skill_checksum_hex(trimmed.as_bytes());
    let stamp = if brand == UNSTAMPED_BRAND_KEY {
        String::new()
    } else {
        format!("\nskill_brand: {brand}")
    };
    let extras = extra_frontmatter
        .map(|s| format!("\n{}", s.trim_end()))
        .unwrap_or_default();
    format!(
        "---\nk2_skill: {skill_type}\nskill_version: {version}\nskill_checksum: {checksum}{stamp}{extras}\n---\n\n{begin}\n{trimmed}\n{end}\n\n{yours}\n",
        begin = SKILL_BEGIN_MARKER,
        end = SKILL_END_MARKER,
        yours = SKILL_YOURS_LINE,
    )
}

/// What follows the managed region, split into the "yours" lines our
/// writes put at its top (the old line or the new one) and the person's
/// part: `(how many "yours" lines,
/// the person's part)`. Several lines are the old upgrade's doing: each
/// upgrade kept the line and wrote it again. The person's part is the
/// bytes after the last of them, as they are; with none, the text after
/// the leading blank lines.
fn split_footer(after_end: &str) -> (usize, &str) {
    let mut count = 0;
    let mut rest = after_end;
    while let Some(r) = SKILL_YOURS_LINES
        .iter()
        .find_map(|line| rest.trim_start().strip_prefix(line))
    {
        count += 1;
        rest = r
            .strip_prefix("\r\n")
            .or_else(|| r.strip_prefix('\n'))
            .unwrap_or(r);
    }
    if count == 0 {
        (0, after_end.trim_start())
    } else {
        (count, rest)
    }
}

/// `wrapped` (a fresh file, which ends with one "yours" line) with the
/// person's part of the old file's `after_end` below it, untouched.
fn with_persons_part(wrapped: String, after_end: &str) -> String {
    let (_, persons) = split_footer(after_end);
    if persons.trim().is_empty() {
        wrapped
    } else {
        wrapped + persons
    }
}

/// A current file whose footer holds the "yours" line more than once (the
/// old upgrade's doing) gets one line again. Only that footer changes:
/// the managed region above and the person's part below stay as they are.
fn collapse_footer(skill_path: &Path, existing: &str, parsed: &ParsedSkill) {
    if !parsed.has_markers {
        return;
    }
    let (count, persons) = split_footer(&parsed.after_end);
    if count < 2 {
        return;
    }
    let head = &existing[..existing.len() - parsed.after_end.len()];
    let fixed = format!("{head}\n\n{SKILL_YOURS_LINE}\n{persons}");
    log_if_err(
        "collapse the skill footer",
        skill_path,
        atomic_write_str(skill_path, &fixed),
    );
}

pub struct ParsedSkill {
    pub k2_skill: Option<String>,
    pub skill_version: Option<u32>,
    pub skill_checksum: Option<String>,
    /// The brand key the file was written for (`skill_brand`); `None` when
    /// it has no stamp. [`ParsedSkill::brand_key`] reads it.
    pub skill_brand: Option<String>,
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

impl ParsedSkill {
    /// The brand key the file was written for: its stamp, or
    /// [`UNSTAMPED_BRAND_KEY`] when it has none.
    pub fn brand_key(&self) -> &str {
        self.skill_brand.as_deref().unwrap_or(UNSTAMPED_BRAND_KEY)
    }

    /// The file is on the current contract for this type, version and
    /// brand key: it has its markers and all three match.
    fn is_current(&self, skill_type: &str, version: u32, brand: &str) -> bool {
        self.has_markers
            && self.k2_skill.as_deref() == Some(skill_type)
            && self.skill_version == Some(version)
            && self.brand_key() == brand
    }
}

pub fn parse_skill(content: &str) -> ParsedSkill {
    let mut parsed = ParsedSkill {
        k2_skill: None,
        skill_version: None,
        skill_checksum: None,
        skill_brand: None,
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
                        "skill_brand" => parsed.skill_brand = Some(v.to_string()),
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
/// - current version, type AND brand key match → no-op (file on disk
///   is fine)
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
    ensure_skill_up_to_date_for(
        skill_path,
        skill_type,
        current_version,
        fresh_body,
        extra_frontmatter,
        brand_key(),
    )
}

/// [`ensure_skill_up_to_date`] for the brand key `brand` (tests stand in
/// for another brand's build with it).
pub fn ensure_skill_up_to_date_for(
    skill_path: &Path,
    skill_type: &str,
    current_version: u32,
    fresh_body: &str,
    extra_frontmatter: Option<&str>,
    brand: &str,
) -> SkillUpgradeOutcome {
    let wrap = |body: &str| {
        wrap_managed_skill_for(skill_type, current_version, body, extra_frontmatter, brand)
    };
    if let Some(parent) = skill_path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if !skill_path.exists() {
        let wrapped = wrap(fresh_body);
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
    if parsed.is_current(skill_type, current_version, brand) {
        collapse_footer(skill_path, &existing, &parsed);
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

        let wrapped = wrap(fresh_body);
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
        // The person's part stays as it is, under one "yours" line.
        let final_content = with_persons_part(wrap(fresh_body), &parsed.after_end);
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
    let wrapped = wrap(fresh_body);
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
/// the `.proposed` sibling. Everything else (another version or brand key
/// included) is `ensure_skill_up_to_date`.
pub fn refresh_skill_body(
    skill_path: &Path,
    skill_type: &str,
    current_version: u32,
    fresh_body: &str,
    extra_frontmatter: Option<&str>,
) -> SkillUpgradeOutcome {
    refresh_skill_body_for(
        skill_path,
        skill_type,
        current_version,
        fresh_body,
        extra_frontmatter,
        brand_key(),
    )
}

/// [`refresh_skill_body`] for the brand key `brand`.
pub fn refresh_skill_body_for(
    skill_path: &Path,
    skill_type: &str,
    current_version: u32,
    fresh_body: &str,
    extra_frontmatter: Option<&str>,
    brand: &str,
) -> SkillUpgradeOutcome {
    let existing = fs::read_to_string(skill_path).ok();
    let parsed = existing.as_deref().map(parse_skill);
    let same_contract = parsed
        .as_ref()
        .is_some_and(|p| p.is_current(skill_type, current_version, brand));
    let Some(parsed) = parsed.filter(|_| same_contract) else {
        return ensure_skill_up_to_date_for(
            skill_path,
            skill_type,
            current_version,
            fresh_body,
            extra_frontmatter,
            brand,
        );
    };
    let region = parsed.managed_region.as_deref().unwrap_or("").trim();
    if region == fresh_body.trim() {
        if let Some(existing) = existing.as_deref() {
            collapse_footer(skill_path, existing, &parsed);
        }
        return SkillUpgradeOutcome::UpToDate;
    }
    let wrapped = wrap_managed_skill_for(
        skill_type,
        current_version,
        fresh_body,
        extra_frontmatter,
        brand,
    );
    if skill_checksum_hex(region.as_bytes()) != parsed.skill_checksum.as_deref().unwrap_or("") {
        let proposed_path = skill_path.with_extension("md.proposed");
        log_if_err("refresh_skill_body propose", &proposed_path, atomic_write_str(&proposed_path, &wrapped));
        return SkillUpgradeOutcome::UserModified;
    }
    // The person's part, without the "yours" line `wrap_managed_skill`
    // writes again (a flip back and forth never grows the file).
    let final_content = with_persons_part(wrapped, &parsed.after_end);
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

    // ── The brand key ─────────────────────────────────────────────────

    /// A key no brand in these tests is built with.
    fn other_key() -> String {
        brand_names_key("Globex Desk", "zq", &[])
    }

    fn scratch_skill(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "k2-skill-brand-key-{tag}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("make the scratch folder");
        dir.join("SKILL.md")
    }

    #[test]
    fn the_unstamped_key_is_the_k2_table() {
        assert_eq!(
            brand_names_key("K2", "k2", &[("k2so", AliasMode::Silent)]),
            UNSTAMPED_BRAND_KEY
        );
    }

    /// The k2 brand's file holds the unstamped names, so a k2 build writes
    /// no `skill_brand` line and its skills stay byte for byte as before.
    /// A change to brand/k2's product or `[cli]` table fails here: it would
    /// stamp and rewrite every k2 workspace's skills.
    #[test]
    fn the_k2_brand_file_is_the_unstamped_table() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../brand/k2/brand.toml");
        let text = fs::read_to_string(&path).expect("read brand/k2/brand.toml");
        let doc: toml::Table = toml::from_str(&text).expect("brand/k2/brand.toml is TOML");
        let product = doc["names"]["product"]
            .as_str()
            .expect("names.product is a string");
        let cli = doc["cli"].as_table().expect("[cli] is a table");
        let name = cli["name"].as_str().expect("cli.name is a string");
        let aliases: Vec<(&str, AliasMode)> = cli["aliases"]
            .as_array()
            .expect("cli.aliases is a list")
            .iter()
            .map(|a| {
                let mode = match a["mode"].as_str().expect("an alias's mode") {
                    "silent" => AliasMode::Silent,
                    "notice" => AliasMode::Notice,
                    "tombstone" => AliasMode::Tombstone,
                    other => panic!("unknown alias mode {other:?}"),
                };
                (a["name"].as_str().expect("an alias's name"), mode)
            })
            .collect();
        assert_eq!(
            brand_names_key(product, name, &aliases),
            UNSTAMPED_BRAND_KEY
        );
    }

    #[test]
    fn this_builds_key_is_its_brand_table() {
        assert_eq!(
            brand_key(),
            brand_names_key(PRODUCT_NAME, CLI_NAME, CLI_ALIAS_MODES)
        );
    }

    /// The product and the command are separate values: a change to
    /// either alone, or to an alias or its mode, is a new key.
    #[test]
    fn the_key_changes_with_the_product_the_name_an_alias_or_a_mode() {
        let keys = [
            brand_names_key("K2", "k2", &[("k2so", AliasMode::Silent)]),
            brand_names_key("Globex Desk", "k2", &[("k2so", AliasMode::Silent)]),
            brand_names_key("K2", "k5", &[("k2so", AliasMode::Silent)]),
            brand_names_key("K5", "k5", &[("k2", AliasMode::Notice)]),
            brand_names_key("K5", "k5", &[("k2", AliasMode::Tombstone)]),
            brand_names_key("K5", "k5", &[]),
            brand_names_key(
                "K5",
                "k5",
                &[("k2", AliasMode::Notice), ("k2so", AliasMode::Silent)],
            ),
            // The two words are not run together: "ab" + "c" is not "a" + "bc".
            brand_names_key("ab", "c", &[]),
            brand_names_key("a", "bc", &[]),
        ];
        for (i, a) in keys.iter().enumerate() {
            assert_eq!(a.len(), 16, "{a}");
            for b in &keys[i + 1..] {
                assert_ne!(a, b);
            }
        }
    }

    /// Under the unstamped key the file is today's shape, byte for byte.
    #[test]
    fn the_unstamped_key_writes_todays_bytes() {
        let wrapped =
            wrap_managed_skill_for("workspace", 3, "# K2 body\n", None, UNSTAMPED_BRAND_KEY);
        let want = format!(
            "---\nk2_skill: workspace\nskill_version: 3\nskill_checksum: {}\n---\n\n{SKILL_BEGIN_MARKER}\n# K2 body\n{SKILL_END_MARKER}\n\n{SKILL_YOURS_LINE}\n",
            skill_checksum_hex(b"# K2 body")
        );
        assert_eq!(wrapped, want);
        assert_eq!(parse_skill(&wrapped).skill_brand, None);
        assert_eq!(parse_skill(&wrapped).brand_key(), UNSTAMPED_BRAND_KEY);
    }

    #[test]
    fn another_key_is_stamped_and_read_back() {
        let key = other_key();
        let wrapped = wrap_managed_skill_for(
            "workspace",
            3,
            "body",
            Some("name: x\ndescription: y"),
            &key,
        );
        assert!(
            wrapped.contains(&format!("\nskill_brand: {key}\nname: x\n")),
            "{wrapped}"
        );
        let parsed = parse_skill(&wrapped);
        assert_eq!(parsed.skill_brand.as_deref(), Some(key.as_str()));
        assert_eq!(parsed.brand_key(), key);
        assert_eq!(parsed.extra_frontmatter, "name: x\ndescription: y");
        assert_eq!(parsed.managed_region.as_deref(), Some("body"));
    }

    /// A file written for one brand, at the current version, is
    /// rewritten for another: the brand change alone refreshes it.
    #[test]
    fn a_key_change_alone_refreshes_an_unedited_file() {
        let path = scratch_skill("change");
        let old = other_key();
        let new = brand_names_key("Initech Hub", "zr", &[("zq", AliasMode::Notice)]);
        fs::write(
            &path,
            wrap_managed_skill_for("workspace", 9, "run `zq msg`", None, &old),
        )
        .expect("write the old brand's file");
        let out = ensure_skill_up_to_date_for(&path, "workspace", 9, "run `zr msg`", None, &new);
        assert!(matches!(out, SkillUpgradeOutcome::Upgraded), "{out:?}");
        let parsed = parse_skill(&fs::read_to_string(&path).expect("read the skill"));
        assert_eq!(parsed.brand_key(), new);
        assert_eq!(parsed.skill_version, Some(9));
        assert_eq!(parsed.managed_region.as_deref(), Some("run `zr msg`"));
        // Written for this key now: the next start leaves it alone.
        let again = ensure_skill_up_to_date_for(&path, "workspace", 9, "run `zr msg`", None, &new);
        assert!(matches!(again, SkillUpgradeOutcome::UpToDate), "{again:?}");
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// Today's files (no stamp) are rewritten by a build of another
    /// table, and a stamped file goes back to no stamp under the
    /// unstamped table.
    #[test]
    fn unstamped_and_stamped_files_refresh_both_ways() {
        let path = scratch_skill("both");
        let key = other_key();
        fs::write(
            &path,
            wrap_managed_skill_for("k2-cli", 4, "a", None, UNSTAMPED_BRAND_KEY),
        )
        .expect("write today's file");
        let out = ensure_skill_up_to_date_for(&path, "k2-cli", 4, "b", None, &key);
        assert!(matches!(out, SkillUpgradeOutcome::Upgraded), "{out:?}");
        let text = fs::read_to_string(&path).expect("read the skill");
        assert_eq!(parse_skill(&text).brand_key(), key);

        let out = ensure_skill_up_to_date_for(&path, "k2-cli", 4, "a", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::Upgraded), "{out:?}");
        let text = fs::read_to_string(&path).expect("read the skill");
        assert!(!text.contains("skill_brand:"), "{text}");
        assert_eq!(parse_skill(&text).managed_region.as_deref(), Some("a"));
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// A person's edit is kept under a brand change too: the new text goes
    /// beside it as `.proposed`, stamped with the new key.
    #[test]
    fn an_edited_file_gets_a_proposed_sibling_on_a_key_change() {
        let path = scratch_skill("edited");
        let key = other_key();
        let edited = wrap_managed_skill_for("workspace", 2, "theirs", None, UNSTAMPED_BRAND_KEY)
            .replace("theirs", "theirs, edited");
        fs::write(&path, &edited).expect("write the edited file");
        let out = ensure_skill_up_to_date_for(&path, "workspace", 2, "ours", None, &key);
        assert!(matches!(out, SkillUpgradeOutcome::UserModified), "{out:?}");
        assert_eq!(fs::read_to_string(&path).expect("read the skill"), edited);
        let proposed =
            fs::read_to_string(path.with_extension("md.proposed")).expect("read the proposed file");
        assert_eq!(parse_skill(&proposed).brand_key(), key);
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// `refresh_skill_body` (same version, another body) also refreshes on
    /// a key change alone, even when the body is the same.
    #[test]
    fn refresh_skill_body_refreshes_on_a_key_change() {
        let path = scratch_skill("refresh");
        let key = other_key();
        fs::write(
            &path,
            wrap_managed_skill_for("k2-zen", 5, "same", None, UNSTAMPED_BRAND_KEY),
        )
        .expect("write the file");
        let out = refresh_skill_body_for(&path, "k2-zen", 5, "same", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::UpToDate), "{out:?}");
        let out = refresh_skill_body_for(&path, "k2-zen", 5, "same", None, &key);
        assert!(matches!(out, SkillUpgradeOutcome::Upgraded), "{out:?}");
        let text = fs::read_to_string(&path).expect("read the skill");
        assert_eq!(parse_skill(&text).brand_key(), key);
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    // ── The footer: one "yours" line, the person's part untouched ────

    /// What a person wrote under the "yours" line: a blank line first, no
    /// newline at the end, so any trim would show.
    const PERSONS: &str = "\nmy notes\n\n- keep this  \n  indented";

    fn yours_count(text: &str) -> usize {
        text.matches(SKILL_YOURS_LINE).count()
    }

    /// A file through five upgrades: after each one the "yours" line is
    /// there once, the managed text is the new version's, and the
    /// person's part is byte for byte what they wrote.
    #[test]
    fn five_upgrades_keep_one_yours_line_and_the_persons_part() {
        let path = scratch_skill("five");
        let first = wrap_managed_skill_for("workspace", 1, "body v1", None, UNSTAMPED_BRAND_KEY);
        fs::write(&path, format!("{first}{PERSONS}")).expect("write the first version");
        for v in 2..=6 {
            let body = format!("body v{v}");
            let out = ensure_skill_up_to_date_for(
                &path,
                "workspace",
                v,
                &body,
                None,
                UNSTAMPED_BRAND_KEY,
            );
            assert!(
                matches!(out, SkillUpgradeOutcome::Upgraded),
                "v{v}: {out:?}"
            );
            let text = fs::read_to_string(&path).expect("read the skill");
            assert_eq!(yours_count(&text), 1, "v{v}:\n{text}");
            assert!(
                text.ends_with(&format!("{SKILL_YOURS_LINE}\n{PERSONS}")),
                "v{v}:\n{text}"
            );
            let parsed = parse_skill(&text);
            assert_eq!(parsed.skill_version, Some(v));
            assert_eq!(parsed.managed_region.as_deref(), Some(body.as_str()));
        }
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// With nothing below the line, an upgrade writes the fresh file.
    #[test]
    fn an_upgrade_with_no_persons_part_writes_the_fresh_file() {
        let path = scratch_skill("bare");
        fs::write(
            &path,
            wrap_managed_skill_for("workspace", 1, "a", None, UNSTAMPED_BRAND_KEY),
        )
        .expect("write the file");
        ensure_skill_up_to_date_for(&path, "workspace", 2, "b", None, UNSTAMPED_BRAND_KEY);
        assert_eq!(
            fs::read_to_string(&path).expect("read the skill"),
            wrap_managed_skill_for("workspace", 2, "b", None, UNSTAMPED_BRAND_KEY)
        );
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// A file the old upgrade left with the line four times: a start at the
    /// same version collapses them to one, upgrading or not, and leaves the
    /// managed region and the person's part as they were.
    #[test]
    fn several_yours_lines_collapse_to_one() {
        let path = scratch_skill("collapse");
        let fresh = wrap_managed_skill_for("workspace", 3, "same", None, UNSTAMPED_BRAND_KEY);
        let extra = format!("{SKILL_YOURS_LINE}\n").repeat(3);
        let old = format!("{fresh}{extra}{PERSONS}");
        assert_eq!(yours_count(&old), 4);
        fs::write(&path, &old).expect("write the old file");
        let out =
            ensure_skill_up_to_date_for(&path, "workspace", 3, "same", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::UpToDate), "{out:?}");
        assert_eq!(
            fs::read_to_string(&path).expect("read the skill"),
            format!("{fresh}{PERSONS}")
        );

        // refresh_skill_body's same-text path does the same.
        fs::write(&path, &old).expect("write the old file again");
        let out = refresh_skill_body_for(&path, "workspace", 3, "same", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::UpToDate), "{out:?}");
        assert_eq!(
            fs::read_to_string(&path).expect("read the skill"),
            format!("{fresh}{PERSONS}")
        );

        // And an upgrade from it writes the line once.
        fs::write(&path, &old).expect("write the old file again");
        ensure_skill_up_to_date_for(&path, "workspace", 4, "new", None, UNSTAMPED_BRAND_KEY);
        let text = fs::read_to_string(&path).expect("read the skill");
        assert_eq!(yours_count(&text), 1, "{text}");
        assert!(
            text.ends_with(&format!("{SKILL_YOURS_LINE}\n{PERSONS}")),
            "{text}"
        );
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// A person's own copy of the line further down is theirs: only the
    /// lines at the top of the footer are ours.
    #[test]
    fn a_yours_line_inside_the_persons_part_stays() {
        let path = scratch_skill("inner");
        let persons = format!("notes\n{SKILL_YOURS_LINE}\nmore\n");
        let first = wrap_managed_skill_for("workspace", 1, "a", None, UNSTAMPED_BRAND_KEY);
        fs::write(&path, format!("{first}{persons}")).expect("write the file");
        ensure_skill_up_to_date_for(&path, "workspace", 2, "b", None, UNSTAMPED_BRAND_KEY);
        let text = fs::read_to_string(&path).expect("read the skill");
        assert!(
            text.ends_with(&format!("{SKILL_YOURS_LINE}\n{persons}")),
            "{text}"
        );
        assert_eq!(yours_count(&text), 2);
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    /// A file written with the old "yours" line (it names the product) is
    /// read as before: left alone while current, and an upgrade writes the
    /// new line once, with the person's part as it was. Several old lines,
    /// the old upgrade's doing, collapse to one new line.
    #[test]
    fn an_old_yours_line_is_read_and_an_update_writes_the_new_one() {
        assert!(
            !SKILL_YOURS_LINE.contains("K2"),
            "the new line names no product"
        );
        let path = scratch_skill("old-line");
        let fresh = wrap_managed_skill_for("workspace", 1, "a", None, UNSTAMPED_BRAND_KEY);
        let old = fresh.replace(SKILL_YOURS_LINE, SKILL_YOURS_LINE_OLD) + PERSONS;
        fs::write(&path, &old).expect("write the old file");
        let out =
            ensure_skill_up_to_date_for(&path, "workspace", 1, "a", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::UpToDate), "{out:?}");
        assert_eq!(fs::read_to_string(&path).expect("read the skill"), old);

        let out =
            ensure_skill_up_to_date_for(&path, "workspace", 2, "b", None, UNSTAMPED_BRAND_KEY);
        assert!(matches!(out, SkillUpgradeOutcome::Upgraded), "{out:?}");
        let text = fs::read_to_string(&path).expect("read the skill");
        assert_eq!(
            text,
            format!(
                "{}{PERSONS}",
                wrap_managed_skill_for("workspace", 2, "b", None, UNSTAMPED_BRAND_KEY)
            )
        );
        assert!(!text.contains(SKILL_YOURS_LINE_OLD), "{text}");

        let several = fresh.replace(SKILL_YOURS_LINE, SKILL_YOURS_LINE_OLD)
            + &format!("{SKILL_YOURS_LINE_OLD}\n{SKILL_YOURS_LINE}\n{SKILL_YOURS_LINE_OLD}\n")
            + PERSONS;
        fs::write(&path, &several).expect("write the file with several lines");
        ensure_skill_up_to_date_for(&path, "workspace", 1, "a", None, UNSTAMPED_BRAND_KEY);
        assert_eq!(
            fs::read_to_string(&path).expect("read the skill"),
            format!("{fresh}{PERSONS}")
        );
        fs::remove_dir_all(path.parent().expect("a folder")).ok();
    }

    #[test]
    fn version_constants_exported() {
        // Guardrail: these are part of the public API — dropping one
        // would break every caller. Test just reads them to prevent
        // accidental removal.
        let _ = SKILL_VERSION_MANAGER;
        let _ = SKILL_VERSION_K2_AGENT;
        let _ = SKILL_VERSION_CUSTOM_AGENT;
        let _ = SKILL_VERSION_TEMPLATE;
        let _ = SKILL_VERSION_WORKSPACE;
    }
}
