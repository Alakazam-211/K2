# Roadmap — PRDs, feedback, polish

**Canonical mini-index for unfinished work.**  
**Created:** 2026-08-08 · **Updated:** 2026-08-22 (remote E2E storm mini-PRDs)  
**Sources:** `0.40.x-to-1.0-weekend-roadmap.md`, wiki `_Index`, host-session / Connect / polish PRDs, model-override PRD, **Julie/Scout K2-ASKS-2026-08-08**

**How to use**

- Living backlog only — not a ship calendar, not a second wiki.
- **One line + pointer** (PRD path, wiki note). Detail stays in the PRD.
- Ship → move to **Shipped archive** with version (or delete).
- New work → add a row here so nothing lives only in chat.

**Not goals**

- Re-document everything already on main.
- Replace individual PRDs.

---

## Closed / do not re-open as open work

| Item | Note |
|------|------|
| K2SO → K2 rebrand, Connect, tunnel/monetization baseline, port exposure | Shipped under 0.40.x train |
| Hermes as seeded preset | Basic CLI seed exists |
| **Mobile chat renderer** | **Done** (Rosson 2026-08-08) — only **Mobile TTS** remains from that pair |
| **1.0 graduation for mobile** | **Done** (Rosson 2026-08-08) |
| Companion terminal path, soft-resync/grid-stall train, host-session reap 0.40.87, etc. | Shipped; residuals only if listed under **Open** |

Historical plan (do not edit as live backlog):  
`.k2/prds/0.40.x-to-1.0-weekend-roadmap.md`

---

## Scout asks — 2026-08-08 (Julie · ticket ecf2bea9 · Adam/Rosson)

**Not urgent-today.** Source on Scout: `/home/k2/AI Projects/Julie/K2-ASKS-2026-08-08.md`  
**K2 durable copy:** `.k2/prds/scout-asks-2026-08-08.md`

| P | Item | PRD / pointer |
|---|------|----------------|
| **1** | **Host-session STATUS** (alive/dead/last-activity by session id) — reconciler safety | `.k2/prds/prd-v1-host-session-status-v1.md` |
| **2** | **Verified per-seat stop** — `enabled:false` did **not** gate spawns (13/14d) | `.k2/prds/prd-verified-seat-stop-control-v1.md` |
| 3 | **PROJECT.md scaffold** — commented template must not impersonate content | Needs PRD / context-stack slice |
| 4 | **Composed-artifact staleness warning** | wiki Feature - Skill Gen Residue Scout |
| 5 | **SKILL.md regenerate** + `skill_checksum` content-bound | wiki Feature - Skill Gen Residue Scout |
| 6 | **Redaction/retention** for credential-bearing spawn prompts at rest | Needs security PRD |
| **7** | **Inactivity reap gated on done-ness** (**z3thon**, not Adam) — ~5–10 min quiet **after** done | `.k2/prds/scout-asks-2026-08-08.md` §7 · `.k2/prds/prd-host-session-completion-lifecycle-v1.md` · depends on **1** + done-signal |
| **8** | **BUG: ticket/feedback `[from]` / author = daemon owner (`Adam`), not acting human (z3thon)** — governance | `.k2/prds/prd-message-from-attribution-actor-v1.md` · `.k2/prds/scout-asks-2026-08-08.md` §8 |

Headline pair (**1–2**): status before kill · stop that actually stops.  
**Item 7** closes the no-wall-clock-caps arc (post-done idle reap).  
**Item 8** is attribution authority — not cosmetic.

---

## Open A — From old weekend roadmap (kept)

| Item | Status | Pointers / notes |
|------|--------|------------------|
| **Script system** | Open · needs design/PRD | Weekend plan: `0.40.x-to-1.0-weekend-roadmap.md` §0.40.X.4. Workspace macros / `k2 script run` / permissions TBD. **No dedicated PRD yet** — write one when scoped. |
| **Research folder jailing** (silo writes) | Open · beta intent | Weekend plan §0.41.X.1. Constrain agent FS to a research subfolder. Related sandbox PRDs (full jail ≠ this): `.k2/prds/prd-session-sandboxing-v1.md`, `.k2/prds/prd-sandboxed-agent-endpoints.md` — jailing may be a thin workspace-mode slice, not full microVM. |
| **TTS summaries** | Open · product shape locked (Rosson) | Weekend plan §0.42.0 (TTS half). **Queue in the CLI tool**, ingest from **multiple chats/agents** — not a single-pane-only chime. No dedicated TTS PRD yet; pair later with queue design. Related stream notes: `.k2/prds/kessel-t1.md`, `.k2/notes/kessel-hard-learnings.md`. |
| **Changes tab** redesign | Open · UX debt | Weekend plan §0.42.X.2. Scope: release notes vs workspace activity vs both — still TBD. No dedicated PRD yet. |
| **Mobile TTS** | Open | Weekend plan §0.42.X.4 (TTS half only). **Chat renderer done.** Companion PRDs: `.k2/prds/prd-companion-terminal-first-class.md`, `.k2/prds/prd-companion-v2-servers-projects-feedback-push.md`, `.k2/prds/prd-mobile-push-reverse-ticket.md`. |
| **Local LLM plug-and-play** | Open | Weekend plan §1.1.0 (`AiTarget`, wizard, catalog, starter model). Design: `.k2/notes/custom-agents-local-llm-design.md`. Related settings surface later; no single SSOT PRD named yet — promote from notes when scheduled. |

**Explicitly dropped from old roadmap keep-list** (do not track here unless re-added): Brain-as-0.41.0 mega, Kessel JSONL chat renderer as open product, Owner/Manager grouping, Alakazam Engine SKILLs, full 1.0 marketing graduation, dormant-session notifications as a separate epic, Companion simplified chat UI (chat renderer done).

---

## Open B — New roadmap (keep all; PRD pointers)

### B1 · Models & agents

| Item | Status | PRD / wiki |
|------|--------|------------|
| **Workspace default model + API model override** | Proposed | `.k2/prds/prd-workspace-default-model-and-api-model-override-v1.md` |
| **Settings API keys UI** (workspace API tab + global tokens) | Locked / confirm polish | `.k2/prds/prd-settings-api-keys-ui-v1.md` · wiki Feature - Settings API Keys UI |
| **Tab session messaging** (inject/wake by sessionId) | Locked · implement with sidecar PRD | `.k2/prds/prd-session-addressed-msg-read-talk-v1.md` · wiki Feature - Tab Session Messaging |
| **Sidecar identity + addressing** (`K2_CELL`, `sales/reviewer`, `k2 whoami`, no spawn prompt) | Ready for review | `.k2/prds/prd-sidecar-identity-and-addressing-v1.md` · wiki Feature - Sidecar Identity and Addressing |
| **Workspace Agent Name vs Handle** (display free; handle slugged/unique; federated rename break) | Locked for implement | `.k2/prds/prd-workspace-display-name-and-handle-v1.md` · wiki Feature - Workspace Agent Name and Handle |
| **Active window: need wakes, idle sleeps** (80-ws RAM; 0.40.57 undo; heartbeat/ticket/msg = N-hour clock) | On main `04232894`+`968acaf0` · unreleased | `.k2/prds/prd-active-window-wake-and-reap-v1.md` · wiki Feature - Active Reaper RAM Revival |
| **Code knowledge graph CLI** | Parked | `.k2/prds/prd-code-knowledge-graph-cli-v1.md` |
| **Workspace KB / Brain map + publish** | Open PRD | `.k2/prds/prd-workspace-kb-brain-map-and-publish.md` |
| **HTML dashboard** (pinned HTML across workspaces) | Open PRD | `.k2/prds/prd-html-dashboard.md` |
| **Context hamburger / catalog** | Open PRDs | `.k2/prds/prd-context-hamburger-v1.md`, `.k2/prds/prd-context-hamburger-catalog-marketplace-addendum.md` |
| **Agent feedback notifications** | Open PRD | `.k2/prds/prd-agent-feedback-notifications.md` |

### B2 · Host sessions / sandboxes / API cells

| Item | Status | PRD / wiki |
|------|--------|------------|
| **Host-session hard wall kills busy work** | Backlog | wiki Bug - Host Session Hard Wall Kills Busy Work · related S9 / completion: `.k2/prds/prd-host-session-completion-lifecycle-v1.md`, `.k2/prds/prd-0.40.87-host-session-reap-scout-v1.md` |
| **Host-session CLI startup hang** | Open | `.k2/prds/prd-host-session-cli-startup-hang-v1.md` |
| **Host-session initial-prompt loss / never-born** | **P0** · preferred: **launch-param prompt** (env/0600, fire-once) + router tree · re-send never-born 0/3 futile · F4 loud `delivered`/`resumed`/`turnStarted` | `.k2/prds/prd-host-session-initial-prompt-loss-v1.md` §7–§8 · Julie consolidated 2026-08-10 |
| **Host-session hung / orphan cleanup** | Open | `.k2/prds/prd-host-session-hung-orphan-cleanup-v1.md` |
| **Host-session status endpoint** | **P1 Scout** · ready to implement · **PR B** after unbake | `.k2/prds/prd-v1-host-session-status-v1.md` · **`prd-caps-recovery-consensus-addendum-v1.md`** (`latest_seq` naming) · `.k2/prds/scout-asks-2026-08-08.md` · wiki Feature - Host Session Completion Lifecycle Scout |
| **Verified per-seat stop control** | **P1 Scout** · draft PRD | `.k2/prds/prd-verified-seat-stop-control-v1.md` · evidence: `enabled:false` non-gating (Julie 2026-08-08) |
| **Inactivity reap after done** (5–10 min quiet post-done) | **Scout #7** · Adam | `.k2/prds/scout-asks-2026-08-08.md` §7 · `.k2/prds/prd-host-session-completion-lifecycle-v1.md` · needs done definition + last-activity (status #1); **not** Working hard wall |
| **Host-session kill route** | Load-bearing PRD | `.k2/prds/prd-v1-host-session-kill-v1.md` (shipped train — residual only if listed) |
| **Host-session capabilities / envelope** | Shipped envelope + **PR A resource unbake** next | `.k2/prds/prd-v1-host-session-capabilities-v1.md`, `docs/host-session-capability-envelope.md`, **`prd-caps-recovery-consensus-addendum-v1.md`** (grammar + Layer B) |
| **Sandbox / sandboxed agent endpoints** | Open PRD family | `.k2/prds/prd-sandboxed-agent-endpoints.md`, `.k2/prds/prd-session-sandboxing-v1.md`, `.k2/prds/prd-sandbox-workspace-scoped-sessions.md`, `.k2/prds/prd-sandbox-addendum-hosted-sessions.md` (+ p1–p4 / guest-creds specs as needed) |

### B3 · Terminal / remote / Connect client

| Item | Status | PRD / wiki |
|------|--------|------------|
| **Remote terminal soft-resync** | PRD + partial ship (0.40.82–88) | `.k2/prds/prd-remote-terminal-soft-resync-v1.md` · wiki Feature - Remote Terminal Soft Resync · wiki Bug - Remote Terminal Stuck Ready Dead Grid |
| **Terminal scroll / attach size** | PRD | `.k2/prds/prd-terminal-scroll-attach-size-v1.md` · wiki Feature - Terminal Scroll Attach Size |
| **Remote Pace / Kessel** | PRDs | `.k2/prds/prd-remote-pace-kessel-v1.md`, `.k2/prds/kessel-t1.md` |
| **Headless Connect onboarding** | PRD | `.k2/prds/prd-headless-connect-onboarding-v1.md` · wiki Feature - Headless Connect Onboarding |
| **Connection resilience** | PRD | `.k2/prds/0.40.48-connection-resilience.md` · wiki Ops - Client Connect Flap GH57 |
| **Connect page version update** | PRD | `.k2/prds/prd-connect-page-version-update.md` |
| **Remote drop no duplicate copies** | PRD | `.k2/prds/prd-remote-drop-no-duplicate-copies-v1.md` |
| **Remote projects mutate latency** | PRD · Phase 1–2 landed; Phase 3 → embed PRD | `.k2/prds/prd-remote-projects-mutate-latency-v1.md` |
| **Remote E2E storms (remaining)** | CORS + embed **on main** for 0.40.106; one-WS + hello next train | `.k2/prds/prd-remote-e2e-storm-audit-v1.md` · CORS `3c958f9c` `.k2/prds/prd-cors-oneshot-while-connected-v1.md` · embed `d14aaa95` `.k2/prds/prd-projects-list-embed-workspaces-v1.md` · one events-WS `.k2/prds/prd-one-session-events-ws-per-path-v1.md` · Hello coalesce `.k2/prds/prd-hello-snapshot-coalesce-v1.md` |
| **Hosted web / edge delivery residual** | PRD | `.k2/prds/prd-hosted-web-client-and-edge-delivery-v1.md` |

### B4 · Platform / naming / migrations

| Item | Status | PRD / wiki |
|------|--------|------------|
| **K2SO naming Endgame Stage B** (`k2so.db` → `k2.db` writer flip) | NEXT | wiki Convention - K2SO Naming Cleanup · `.k2/prds/prd-k2so-endgame-v1.md`, `.k2/prds/prd-k2so-cleanup-v1.md` |
| **Linux ship CI on AX41** | **Locked for 0.40.104** | `.k2/prds/prd-linux-ci-on-ax41-v1.md` · wiki [[CI - Linux Ship on AX41]] — tag `daemon-binaries` + `app-linux` on `k2-sandbox-01` (`k2-linux`); checks stay on GitHub |
| **Linux daemon RSS / charter-watch OPEN** | **Locked for 0.40.104** | `.k2/prds/prd-linux-daemon-rss-charter-watch-v1.md` · wiki [[Bug - Linux Daemon RSS Charter Watch]] — RPMAVS 59 GB OOM; ignore inotify OPEN; bound channel; update fetch timeout |
| **Linux headless / self-update / tunnel** | PRD family | `.k2/prds/prd-linux-headless-daemon.md`, `.k2/prds/prd-linux-remote-self-update-and-tunnel-v1.md` |
| **Server migration** | PRD | `.k2/prds/prd-server-migration-v1.md` |
| **FS unzip / files drawer backlog** | PRDs | `.k2/prds/prd-fs-unzip-v1.md`, `.k2/prds/prd-fs-files-drawer-backlog-v1.md` |
| **File viewer preview expansion** | PRD | `.k2/prds/prd-file-viewer-preview-expansion-v1.md` |
| **Msg inbox file delivery** | PRD | `.k2/prds/prd-msg-inbox-file-delivery-v1.md` |
| **Message from attribution actor** | PRD | `.k2/prds/prd-message-from-attribution-actor-v1.md` |
| **Wiki public chat API** residual | PRD | `.k2/prds/prd-wiki-public-chat-api-loopback-v1.md` |
| **Capability-scoped floor** | PRD | `.k2/prds/prd-capability-scoped-floor.md` |
| **API skip-permissions default** | PRD | `.k2/prds/prd-api-skip-permissions-default-on-v1.md` |

### B5 · Cloud / federation / ops (pointers)

| Item | Status | PRD / wiki |
|------|--------|------------|
| **Cloud federations** | PRD | `.k2/prds/prd-cloud-federations-v1.md` |
| **Cross-server agent comms** | PRD | `.k2/prds/prd-cross-server-agent-comms.md` |
| **Federation passport / trust / UX** | PRD family | `.k2/prds/prd-federation-passport-dual-auth-and-colon-addressing.md`, `.k2/prds/prd-federation-trust-domain.md`, `.k2/prds/prd-federation-ux-permissions.md` |
| **Geo edge tunnel** | PRD | `.k2/prds/prd-geo-edge-tunnel-v1.md` |
| **Tunnel disable/unpair / frpc resilience / teardown** | PRD family | `.k2/prds/prd-tunnel-disable-unpair-v1.md`, `.k2/prds/prd-tunnel-frpc-restart-resilience-v1.md`, `.k2/prds/prd-tunnel-teardown-and-eviction-v1.md` · wiki Ops - Tunnel Port-Desync… |
| **Cloud server upgrade / backups / bare-metal rental** | PRDs | `.k2/prds/prd-cloud-server-upgrade-v1.md`, `.k2/prds/prd-cloud-backups-v1.md`, `.k2/prds/prd-bare-metal-rental-v1.md` |
| **Fleet observability / agent-ops** | PRD | `.k2/prds/prd-observability-agent-ops.md` |
| **Relay redundancy** | PRD | `.k2/prds/prd-relay-redundancy-v1.md` |
| **Billing lifecycle / dedicated tier** | PRDs | `.k2/prds/prd-billing-lifecycle-v1.md`, `.k2/prds/prd-dedicated-tier-permission-model-v1.md` |

---

## Open C — Feedback / polish (chat → durable)

| Item | Source | Notes / pointer |
|------|--------|-----------------|
| Active navbar section scroll | Rosson | ActiveBar / IconRail fix in tree; confirm release cut if not yet |
| Grid-stall focus / no-frame flood | Rosson | **0.40.88** focus gate + heal kept; reopen only on regress |
| **Feedback ticket author stamped as owner display name (`Adam`) not acting person** | Julie / Scout · z3thon answering tickets | **BUG · governance** · §8 scout asks · `.k2/prds/prd-message-from-attribution-actor-v1.md` (prior fix may not cover owner-token / multi-Owner path) |
| **cell-cap CLI 404 on /v1 workspace slugs** (name≠basename) | Julie / Scout 2026-08-10 | **Bug fixed on main** · basename in `resolve_workspace` · ship 0.40.90 · effective caps default **15** if never set · not never-born cause |
| **Grid WS “Insufficient resources” death spiral** (events-reopen fan-out → all panes dial → WebKit FD limit → stall heal thrash) | Rosson / Scout console 2026-08-09 | wiki **Bug - Grid WS Insufficient Resources Death Spiral** · soft-resync + stall heal · need global dial budget + backoff |
| **Windows unified title bar** (double chrome → frameless + **Menu** + min/max/close; macOS untouched) | Rosson 2026-08-11 | **Implemented** `dc0b1661` · `.k2/prds/prd-windows-unified-titlebar-v1.md` · smoke on real Win/Linux still open |
| *(add rows here)* | | |

---

## Needs a PRD (kept open without SSOT)

| Item | Action |
|------|--------|
| Script system | Write PRD before implement |
| TTS summaries (multi-agent CLI queue) | Write PRD; product note locked above |
| Changes tab redesign | Write short PRD after scope choice |
| Local LLM plug-and-play | Promote `.k2/notes/custom-agents-local-llm-design.md` → PRD when scheduled |
| Research folder jailing | Thin PRD or section under session-sandboxing |

---

## Shipped archive (append-only)

| When | What |
|------|------|
| 0.40.88 | Compose focus safe across `grid-stall-no-frame` heal |
| 0.40.87 | Host-session finalize / kill / cell cap / `k2 done` + grid-stall Unit E |
| 0.40.85–86 | Remote terminal stuck-ready, soft-resync compose, breadcrumbs |
| 2026-08-08 | Rosson: mobile **chat renderer** + **mobile 1.0 graduation** closed for roadmap tracking |
| … | |

---

## Old plans (read-only)

| File | Role |
|------|------|
| `.k2/prds/0.40.x-to-1.0-weekend-roadmap.md` | May 2025 multi-weekend plan |
| `.k2/prds/secure-tunnel-monetization-roadmap.md` | Earlier tunnel/monetization sequencing |
| `.k2/wiki/_Index.md` | Feature/bug SSOT one-liners |
