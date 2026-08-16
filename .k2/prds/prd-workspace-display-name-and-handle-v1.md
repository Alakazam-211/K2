# PRD — Workspace Agent Name vs Handle

**Date:** 2026-08-17 · **Owner:** Rosson · **Status:** Locked for implement (reviewed against live code; patches applied)  
**Product:** daemon (`k2-core` + `k2-daemon`) first; `k2` CLI; thin renderer (Settings Agent tab, Workspace tab, nav already follows `projects.name`)  
**Related:** `prd-sidecar-identity-and-addressing-v1.md` (session handles already split), `prd-federation-passport-dual-auth-and-colon-addressing.md` (`agent::host` vs wire `fp::ws_uuid::agent`), `prd-cross-server-agent-comms.md`, `prd-session-addressed-msg-read-talk-v1.md` (D12), `agent-display-name.md` (**superseded** for lockstep / “display is label-only / dupes OK”)  
**Research:** 2026-08-17 — federated delivery, display-name write path, Workspace-tab Agent Name. Review vs live tree same day (diagnosis confirmed; A+B does not resurrect a pre-rename token).

---

## 1. Problem

Workspace Settings → Agent → **Agent Display Name** is one string written to three places:

1. AGENT.md `display_name:`
2. AGENT.md `name:` (roster / `[from …::host]`)
3. `projects.name` (sidebar + local `k2 msg`)

That lockstep was added so federated stamps would follow a rename. It did the opposite of what we need:

- **Pretty names** (`Sales Team`, `QA Bot`) became the **address**.
- Roster publishes `name:` **lowercased** (`sales team`) — spaces kept.
- `workspace_remote_connections` stores a frozen `agent` + `host` and matches **exact** (case-insensitive) strings.
- Reverse connections were stored as the **folder basename**, not the display name.

So after a rename, local `k2 msg "Sales Team"` works (`projects.name` updated) and federated `k2 msg …::host` dies **before inject**:

| What the user tries | What happens |
|---|---|
| `k2 msg sales::host` (old handle) | Roster **NONE** — that token is gone |
| `k2 msg "sales team"::host` (new roster spelling) | Roster hits, send gate **403 not_connected** — row still says `sales` |
| Peer replies using reverse row `cortana::us` | Our roster says `sales team` — **NONE** |

Inbound delivery is **not** the bug. `/cli/federation/inbound` resolves `signal.to.workspace` (the **UUID**) and calls the same `deliver_live` as local `k2 msg`. If the send gets out, the right pinned Chat still gets it. Federation cannot target a sidecar; it always hits canonical.

A second, local UI bug: the left **Workspace** tab → Status → **Agent Name** does not read Agent Display Name. It shows a `k2so_agents_list` **skills/folder basename** (`agent`, `k2so-agent`, an old slug). Nav updates; that field does not.

---

## 2. Product split (locked)

Two fields, different jobs.

| | **Agent Name** (display) | **Handle** |
|---|---|---|
| What | What humans see | What machines address |
| Example | `Sales Team` | `sales-team` |
| Allowed | Capitals, spaces, punctuation except `/` and `:` | `[a-z0-9-]+`, unique **on this host** |
| Used by | Nav, Workspace tab, Chat / Inbox pane headers | `k2 msg`, roster, `name::host`, `k2 whoami`, sidecar prefix `handle/reviewer` |
| Change cost | Anytime, no warning | **Warn:** existing federated connections **will break** until the other side reconnects or updates the handle |

**Display Name = wallpaper. Handle = the street address.**

- Changing Agent Name **must not** change the handle. That is the federation-stability rule going forward.
- Changing Handle is a **new address**. Peers’ connection rows, roster lookups, and any memorized `k2 msg old-handle::host` still point at the old token. We may keep the previous handle as a **temporary alias** so it is not an instant 403, but that is a grace period, not a promise. The UI still warns.

Do **not** invent Title Case. If the live name is already `sales`, display stays `sales` and handle is `sales`.

**Already-broken federated links stay broken until re-pair (locked).** If they already changed Agent Display Name, the peer’s connection row still has the old token (`sales`) and lockstep already overwrote `name:` — we cannot recover `sales` unless it is the folder basename. `slug("sales")` is `sales`, not `sales-team`. That 403 exists **today**. Migrate does not have to heal it. After this ships they add the connection once against the handle; that row sticks. Later Display Name edits do not require another pair.

---

## 3. What exists today (code)

### 3.1 Name stores (conflated)

| Store | Role today |
|---|---|
| AGENT.md `display_name:` | UI helper first pick (`agent_display_name`) |
| AGENT.md `name:` | `resolve_agent_name` → roster `agent`, outbound `[from]` |
| `projects.name` | Nav + local `resolve_workspace` / `k2 msg` / whoami `K2_PRIMARY` |
| Folder basename | Fallback; **reverse-connection** source agent |
| `workspace_session_handles` | Sidecar only (`sales/1`) — **not** the workspace agent |
| `chat_session_names.custom_name` | Session display; slug = sidecar handle |
| `app_settings.owner_display_name` | Human owner — unrelated |

There is **no** `projects.handle` column. **No** unique index on `projects.name`. Path is unique.

Canonical PTY map key is already **bare `project_id`**. Infra does **not** need the handle in the map key. Do not re-key live sessions.

### 3.2 Local `k2 msg` (`resolve_msg_target` / `resolve_workspace`)

CLI peels `agent::host` first. Bare token:

1. Absolute path  
2. Project UUID  
3. Exact `projects.name`  
4. `name COLLATE NOCASE`  
5. Unique folder basename (0 or 2+ → miss)

Collision on exact/nocase name takes **first `rowid`** (not fail-closed). `/v1/w/<slug>` **is** fail-closed on ambiguous name.

### 3.3 Federated send → receive → pinned Chat

```
k2 msg <agent>::<host>
  → Trusted peer for host (subdomain)
  → GET /cli/federation/peer-roster  (agent string, lowercase exact → workspace UUID)
  → POST /cli/federation/send
       gate: workspace_remote_connections (LOWER(agent), LOWER(host))
       envelope to = fp::workspace_uuid::typed-agent
       envelope from = resolve_agent_name(local) lowercased
  → HTTPS POST /cli/federation/inbound
       verify sig / trust / inbound cap
       to.workspace UUID → projects.path
       deliver_live(path)  // same as local; wake=true
```

Wire `to.name` is **unused** on receive. UUID is the only delivery key.

### 3.4 Connection table (migration 0055)

`workspace_remote_connections`: `(source_project_id, remote_addr, host, agent, peer_fingerprint)`.  
UNIQUE `(source_project_id, remote_addr)`. **No remote workspace UUID column.**  
`exists()` matches `LOWER(agent)` + `LOWER(host)`.

Reverse add (`src/renderer/lib/federation.ts`) uses `workspaceBasename(sourcePath)`, not `resolve_agent_name`. That is why folder `Cortana` + display `Sales Team` produces reverse `cortana::us` while we advertise `sales team`.

### 3.5 Workspace tab

`WorkspacePanel` “Agent Name” = `k2so_agents_list[].name` = **directory basename** under `.k2/skills/` (or legacy `.k2so/agents/`). No `agentDisplayName()`, no `sync:projects`. 30s poll only.

Pinned Chat / Inbox **tab titles** stay `Chat` / `Inbox` (intentional). Pane **headers** already call `agentDisplayName()`.

### 3.6 Sidecar pattern to copy

`slugify_custom_name`: trim, lowercase, whitespace runs → single `-`, reject `/` `:` `\` NUL / controls. Unique per workspace. Display (`Reviewer`) ≠ handle (`reviewer`). **Do not change this function** — live sidecar addresses are `k2---marketing` / `scout_v3`. Workspace handles use a **new** helper (§6).

### 3.7 API layer and pinned tabs (stay in one piece)

Pinned Chat / host-sessions are **not** keyed on the display name.

| Surface | Live key | After this PRD |
|---|---|---|
| Canonical PTY / `v2_session_map` | bare `project_id` (`canonical_key_for`) | unchanged |
| `workspace_sessions.session_id` | provider conversation UUID | unchanged |
| Federated inbound | `signal.to.workspace` = `projects.id` | unchanged |
| Pinned Chat / Inbox **tab titles** | `"Chat"` / `"Inbox"` | unchanged |
| Host-session id | `/v1/w/<slug>/host-sessions/<uuid>` | UUID unchanged |
| `/v1/w/<slug>` | `projects.name` (exact / NOCASE, fail-closed) then folder basename (`v1_sandboxes.rs` `resolve_workspace_slug`) | **add handle + aliases** as first matches; keep name + basename so existing `/v1/w/Sales%20Team` and `/v1/w/<folder>` keep working |

`attachAgentName` / renderer `agentName` on the pinned item is routing sugar; `deliver_live` and resume use `project_id` + saved session id. Do **not** remapping pinned tabs or host-session rows.

Wiki public-chat slugs and `connections add <token>` use the same name/basename resolvers — teach them handle the same way as §9.5.

---

## 4. Goals / non-goals

### Goals

1. Split **Agent Name** (display) from **Handle** (address).
2. Migrate every existing workspace **without losing the pretty name** and **without breaking live PTYs, paths, or UUIDs**.
3. Slug handles: spaces → `-`, all lowercase, collapse hyphen runs.
4. Host-local handle uniqueness (case-insensitive), fail-loud on new writes.
5. Federated send/roster/connection gate **slug-match** spellings that are the same token (`sales team` ↔ `sales-team`). Do **not** claim this resurrects a pre-rename token (`sales` vs `sales-team`); those links re-pair once.
6. Agent Name edits never rewrite handle or connection rows.
7. Handle edits warn that federated connections will break; previous handle may be kept as a grace alias.
8. Workspace tab **Agent Name** shows the display name.
9. Sidecar addresses become `handle/reviewer` (first segment is the workspace **handle**).

### Non-goals (this PRD)

- Renaming the folder on disk.
- Re-keying `v2_session_map` / `workspace_sessions` off `project_id`.
- Per-sidecar AGENT.md / inbox / display name.
- Making federation address a sidecar (`sales-team/reviewer::host`) — still canonical only.
- Forcing both peers to upgrade in lockstep (mixed-version must work).
- Inventing prettier display names than what is already live.
- Changing wire form `fp::workspace_uuid::agent`.
- Owner display name (`Your name` in General).
- Remapping pinned tabs, `workspace_sessions`, or host-session UUIDs.
- Auto-healing federated rows whose stored agent is a **different** token than the slugged display (the already-broken `sales` → `Sales Team` case). Re-pair is the heal.

---

## 5. Locked decisions

| # | Decision |
|---|----------|
| D1 | **Two fields.** Display (Agent Name) and Handle. Not lockstep after migration. |
| D2 | **Copy first, slug second.** Freeze today’s `agent_display_name()` into display. Then derive handle from that string. Never slug the only copy. |
| D3 | **Do not invent Title Case.** Preserve the live pretty string exactly. |
| D4 | **New `slugify_address_token` for workspace handles.** Sidecar keeps live `slugify_custom_name` (do not change it). Workspace slug = sidecar rules **plus** hyphen-collapse and `_` → `-`. `K2 - Marketing` → `k2-marketing`. |
| D5 | **`projects.name` stays the pretty Agent Name** so nav / Active bar keep working with no renderer rewrite on day one. |
| D6 | **New `projects.handle`** (NOT NULL after backfill, unique case-insensitive on this host). AGENT.md `name:` is rewritten to the handle (roster already reads `name:`). AGENT.md `display_name:` is the pretty name. **Create paths** (`Project::create`, `projects_create`, lifecycle INSERT, `k2 agent hire`) must mint a handle (slug of requested name or folder; same `-2` suffix). A NOT NULL column with no create default breaks `k2 workspace open`. |
| D7 | **Local `k2 msg` / whoami / `K2_PRIMARY` / sidecar prefix use handle.** `workspace_address_name` must read `projects.handle`, not `projects.name`. Display remains an **alias** for local resolve (quoted `k2 msg "Sales Team"` still works). |
| D8 | **Roster `agent` is the handle.** Also publish `aliases[]` (pre-slug lowercase, folder basename, previous handle if any). |
| D9 | **Match federated agent tokens by slug**, not raw string. `Sales Team` = `sales team` = `sales-team`. Also match `aliases`. Same matcher in **CLI and** `src/renderer/lib/federation.ts` (`resolveRemoteWorkspacePath`). |
| D10 | **Connection gate uses the same matcher** for tokens that slug to each other. Lazy-rewrite a row only when roster handle/alias **matches that row**. Do not guess a single roster entry onto an unrelated stored token (`sales` ↛ `sales-team`). |
| D11 | **Handle change is explicit** (Settings + `k2 workspace set-handle`). Confirm copy: existing federated connections **will break** until the other side updates. Previous handle → grace alias (not a promise). |
| D12 | **Agent Name change is silent** — display + `projects.name` only. No roster identity change. No connection rewrite. |
| D13 | **Collision on migrate:** first workspace (stable `projects.rowid`) keeps the slug; later ones get `slug-2`, `slug-3`, … Log each suffix. Never steal. Never fail daemon boot. Alias inserts are **OR IGNORE** (two folders named `Cortana` must not roll back 0103). |
| D14 | **Workspace tab** reads `agent_display_name()` (display), not `k2so_agents_list` folder names. |
| D15 | **Uniqueness is host-local, handle-only.** Two workspaces may share a display (`Sales`) if handles differ (`sales`, `sales-2`). `Scout` vs `scout` is one handle. |
| D16 | **No folder rename. No UUID change. No PTY remap.** Pinned tabs and host-session UUIDs stay. |
| D18 | **`[from]` stamps the handle** (CLI default, `humanize_chat_from`, principal helper). Display is chrome only. |
| D19 | **`/v1/w/<slug>` + wiki slugs + `connections add`** use the same resolve order as §9.5 (handle, alias, name, basename). Fail-closed on ambiguity stays. Existing pretty-name and folder slugs keep working because `projects.name` stays pretty. |
| D20 | **Already-broken stays broken until re-pair.** Migrate will not resurrect a pre-rename handle that lockstep deleted. That is accepted. |

---

## 6. Slug algorithm

New helper `slugify_address_token` (workspace handles only). **Do not** extend live `slugify_custom_name` — that would retarget existing sidecar addresses (`workspace/k2---marketing`, `workspace/scout_v3`).

```
trim
reject if empty, or contains / : \ NUL or other C0 controls
lowercase (Unicode lower, then treat as address token)
split on whitespace, drop empty, join with '-'
collapse runs of '-'  (also trim leading/trailing '-')
result must be non-empty and match ^[a-z0-9]+(-[a-z0-9]+)*$
  — if leftover punctuation remains (e.g. "QA Bot!"), strip chars
    outside [a-z0-9-] then collapse hyphens again
reject if empty after strip
```

Examples:

| Input (display or old name:) | Handle |
|---|---|
| `sales` | `sales` |
| `Sales` | `sales` |
| `Sales Team` | `sales-team` |
| `  Code Review  ` | `code-review` |
| `QA Bot` | `qa-bot` |
| `K2 - Marketing Manager` | `k2-marketing-manager` |
| `scout_v3` | `scout-v3` if `_` is stripped to `-`, else `scoutv3` — **lock: `_` → `-`**, then collapse |
| `sales/1` | **reject** (`/`) — cannot be a workspace handle |

`:` stays banned (federated `name::host`). `/` stays banned (sidecar `handle/reviewer`).

---

## 7. Data model after migration

### 7.1 `projects` (new column)

```
projects.handle TEXT NOT NULL
CREATE UNIQUE INDEX projects_handle_nocase
  ON projects (handle COLLATE NOCASE);
```

Backfill before adding NOT NULL (or add nullable, fill, then tighten). Path unique stays. `projects.name` remains pretty.

### 7.2 AGENT.md frontmatter

| Field | After migrate |
|---|---|
| `display_name:` | Pretty Agent Name (copied from live resolve if missing) |
| `name:` | **Handle** (slug). Roster / `resolve_agent_name` keep reading this. |
| `handle:` | Optional mirror of `name:` — **not required in v1** if `name:` is the handle. Prefer not adding a third field unless we want AGENT.md to self-describe; implementer may add `handle:` as an alias write for grep-ability. Default: **`name:` = handle**, no extra key. |

### 7.3 `workspace_remote_connections`

No new required column in v1. Matching slug-equates tokens (`sales team` ↔ `sales-team`). Lazy rewrite only when the stored agent already matches a roster handle or alias.

**Accepted gap:** a stored `sales` next to a new handle `sales-team` stays 403. That link was already dead. User re-pairs once.

**Follow-up (not blocking):** add `remote_workspace_id` and key the gate on `(source_project_id, peer_fingerprint, remote_workspace_id)`. Wire already has the UUID. That is the long-term “name is not the key” end state — not required to ship the split.

### 7.4 Aliases (runtime, not a new table in v1)

For each local workspace, computed:

1. Canonical handle (`projects.handle`)  
2. `slug(old)` already equals handle  
3. Pre-slug lowercase of the **pre-migration** `name:` / display (e.g. `sales team`) — persist this **once** at migrate time so we do not lose it when display later changes  
4. Folder basename, lowercased  
5. Previous handle after an explicit handle change (overwrite or keep last-N=1)

Persist pre-migration aliases on the project row so they survive a later Agent Name edit:

```
projects.handle_aliases TEXT  -- JSON array of lowercase tokens, or
-- a tiny project_handle_aliases(project_id, alias) table
```

**Lock:** small table `project_handle_aliases(project_id, alias TEXT, UNIQUE(alias COLLATE NOCASE))` so a host-wide alias cannot point at two workspaces. Migrate writes aliases with **`INSERT OR IGNORE`** (or `ON CONFLICT DO NOTHING`); log skipped collisions; **never** fail the 0103 transaction because two workspaces share a basename. Explicit handle change inserts the old handle the same way. Writers also refuse a new handle that collides with someone else’s alias.

---

## 8. Migration flow (daemon boot, one shot)

Run as the next SQL/data migration after 0102 (number assigned at implement: **0103**). Idempotent. Must not fail the daemon if a single workspace is weird.

```
for each projects row, stable order by rowid:
  1. pretty = agent_display_name(path)
       (display_name: → name: → projects.name → basename → "agent")
  2. if AGENT.md missing display_name: or empty:
       write display_name: pretty
  3. if projects.name is empty or UUID-shaped:
       set projects.name = pretty
     else leave projects.name as-is if it already equals pretty
     if projects.name != pretty (rare desync):
       prefer pretty from agent_display_name; set projects.name = pretty
  4. candidate = slug(pretty)
  5. if candidate taken by an earlier row (handle or alias):
       candidate = candidate-2, candidate-3, … first free
  6. set projects.handle = candidate
  7. rewrite AGENT.md name: = candidate
     (do NOT rewrite display_name: except step 2)
  8. insert aliases OR IGNORE (skip if equal to handle, skip empties):
       - lowercase(pretty) with spaces still spaces   # "sales team"
       - slug is handle                               # skip
       - folder basename lowercased                   # "cortana"
       - previous name: lowercased if different
       (do not invent a pre-lockstep token we no longer have)
  9. invalidate agent_display_name cache for path

then, for each workspace_remote_connections row:
  10. new_agent = slug(row.agent) if slug succeeds
      else leave row (cannot parse)
  11. if new_agent != row.agent:
        rewrite agent + remote_addr (preserve :: vs @ shape)
        on UNIQUE conflict (already have the slugged addr): drop the
        duplicate old row (same source + same peer, old spelling)

emit SyncProjects once at end
```

**Never** rewrite `projects.path`, `projects.id`, session rows, or v2 map keys.

**Unregistered paths** (AGENT.md but no `projects` row): skip SQL; if we touch the file at all, only fill `display_name:` if we already would have. Uniqueness is among registered projects.

### 8.1 Why this order

If we slug `name:` / `projects.name` first, the sidebar becomes `sales-team` and we lose `Sales Team`. Copy into Agent Name first, then put the slug only in handle/`name:`.

### 8.2 Mixed-version peers

| Us | Them | What happens |
|---|---|---|
| Upgraded | Old (roster `sales team`) | New CLI slug-matches `sales-team` ↔ `sales team`. Send works if **our** stored row slugs to the same token. |
| Old CLI | Upgraded (roster `agent` = `sales-team`) | Old CLI is exact `agent.lower() == want` and **does not read `aliases[]`**. Unspaced `sales` still works when handle is `sales`. Spaced `k2 msg "sales team"::host` is **NONE** until they type `sales-team` or upgrade. Those links were already broken. |
| Both upgraded | Both | Roster `sales-team`. Rows whose agent slugs to that handle heal. A leftover `sales` row (pre-pretty rename) stays dead until **re-pair**. |

**Do not** delete aliases in v1. A later cleanup PRD can expire them.

---

## 9. Runtime matching (federation + local)

### 9.1 Normalize

`norm(token) = slug(token)` if slug succeeds, else `trim.to_ascii_lowercase()`.

### 9.2 Roster lookup (`k2 msg handle::host` / CLI Python matcher)

A roster entry matches `want` when any of:

- `norm(want) == entry.agent` (handle, already slugged)
- `norm(want) == norm(alias)` for any `entry.aliases[]`
- `norm(want) == norm(entry.workspace_name)` — convenience only; if **ambiguous**, fail **AMBIG** (same as today)

Zero matches → `NONE` (list published handles).  
Two+ workspace UUIDs → `AMBIG`.

**Lock D17:** roster JSON `agent` = handle, plus `aliases[]`. New CLI and the renderer (`federation.ts` `resolveRemoteWorkspacePath`) slug-match handle + aliases. Old CLI (`cli/k2` exact `agent.lower()`) does **not** read aliases — only `want == handle` works there (unspaced names). We do **not** keep `agent: "sales team"` as the primary field.

One roster row per workspace UUID. Duplicate aliases across workspaces are skipped (OR IGNORE), never two rows for one UUID.

### 9.3 Send gate (`WorkspaceRemoteConnection::exists`)

Today: `LOWER(agent) = LOWER(?2) AND LOWER(host) = LOWER(?3)`.

After:

1. Split want → `(agent, host)`, normalize host as today.  
2. Load source rows for that host (or all source rows and filter host).  
3. Hit if `norm(want.agent) == norm(row.agent)`. That is enough for `sales team` vs `sales-team` **on a stored copy of the pretty/spaced form**.  
4. Stored `sales` vs want `sales-team`: **no match** (D20). Re-pair.

Folder-basename reverse rows (`cortana`) do **not** slug to `sales-team`. Those need **aliases on the receiver’s roster** (`cortana` listed). Sender looks up `cortana` → alias → UUID. Gate: stored `cortana` matches want `cortana`. Works without rewriting.

### 9.4 Lazy heal

When `peer-roster` is fetched and a stored connection’s agent matches that entry’s handle **or** aliases (slug-equal):

- If `row.agent != entry.agent`, update `agent` + `remote_addr` to the canonical handle.  
- UNIQUE conflict → keep the canonical row, delete the stale spelling.

Do **not** attach an unmatched leftover (`sales`) to “the only agent on that host.” Heal is best-effort. Re-pair is the supported heal for D20 rows.

### 9.5 Local resolve (`resolve_workspace`)

Add, in order:

1. Path  
2. Project UUID  
3. `projects.handle` exact / NOCASE  
4. `project_handle_aliases.alias` (unique index → 0 or 1)  
5. `projects.name` exact / NOCASE (display alias)  
6. Unique folder basename  

If display name collides across two workspaces, **handle still unique** — users should type the handle. Name collision: fail-closed (**change from today’s first-rowid**). Loud `AMBIG` with both handles. This is the one local behavior change; it is correct.

Same order for `resolve_workspace_slug` (`/v1/w/<slug>`), wiki `slug_candidates`, and `connections::resolve_target_project_id`.

### 9.6 `k2 whoami` / `K2_PRIMARY`

Print **handle**, not display. Implement by switching `workspace_address_name` to `projects.handle` (today it returns `projects.name` if it has no `/` `:`). Sidecar address `handle/reviewer`. Display may appear as a separate `name:` line later; not required in v1.

### 9.7 `[from]` stamps

**Lock D18:** stamp **handle** for anything a peer might reply to. One helper; point all of these at it:

- CLI `k2 msg` default `--from` (today GETs `/cli/workspace/agent-display-name` — pretty)
- `humanize_chat_from` (UUID → today `agent_display_name`)
- `display_from_for_principal` / `caller_workspace` (today `workspace_address_name` = pretty after D5)

Display stays UI chrome only (nav, Workspace tab, pane headers).  
Sidecar: `[from handle/reviewer]`.  
Federated inbound: `[from handle::host]` (`federated_from_label`).

---

## 10. Writers after migrate

### 10.1 Agent Name (display) — Settings + existing route

`GET /cli/workspace/set-agent-display-name?project=&name=` (or POST if we finally fix the GET-mutation; **do not block on that**).

- `validate_display_name` as today (non-empty, ≤64, no `/` `:`, no controls, trim).  
- **Stops rewriting `name:` and `projects.handle`.**  
- Writes `display_name:` + `projects.name`.  
- Cache bust + `SyncProjects` + live session **label** (look up by **bare `project_id`**, not `project_id:old_name` — that lookup is currently wrong).  
- No uniqueness on display.

Renderer `AgentDisplayNameField` help text: “Shown in the nav and Workspace tab. Does not change the handle or federated address.” Drop the current “for UI and federated addresses / must be unique on this server” copy. Client `validate` must also ban `:` (daemon already does).

### 10.2 Handle — new

`POST /cli/workspace/set-handle` (POST, 405 if GET) + `k2 workspace set-handle <ws> <handle>`.

- Validate slug (or slug the input and require it already be the slug — **lock: accept pretty input, store slug, echo the slug**).  
- Uniqueness vs other handles **and** aliases.  
- Rewrite `projects.handle`, AGENT.md `name:`.  
- Insert **previous** handle into `project_handle_aliases`.  
- Do **not** rewrite peers’ DBs.  
- `SyncProjects`.  
- CLI / Settings: blocking confirm —

  > Changing the handle changes this agent’s address (`old::host` → `new::host`). Existing federated connections will break until the other side reconnects or updates the handle.

- No confirm bypass in the GUI. CLI: `--i-know-this-breaks-federation` required (fail-loud otherwise).

### 10.2b Create / hire (must mint a handle)

`Project::create`, `projects_create`, workspace `lifecycle` raw INSERT, `k2 workspace open`, `k2 agent hire --name`:

- Display = requested pretty name (or folder basename).  
- Handle = `slugify_address_token` of that name (or basename), with `-2` suffix on collision.  
- After slice C, `hire` / `agent set --name` update **display** only; **first create** also seeds handle. A later `--name` must not rotate the handle.

### 10.3 `POST /cli/projects/update { name }`

Today can desync `projects.name` with no uniqueness. After this PRD:

- `{ name }` updates **display only** (same as 10.1) if we keep the route.  
- Must **not** write handle.  
- Prefer Settings / `set-agent-display-name` as the only UI.

### 10.4 Disabled sidebar Rename

Stays “coming soon” **or** becomes display-name edit (same as 10.1). **Not** a folder rename. If enabled in this slice, it is Agent Name, not handle.

---

## 11. UI / flow

### 11.1 Settings → Agent tab

```
Identity
  Agent Name     [ Sales Team        ]     // display, save as today
  Handle         [ sales-team        ]     // new, muted, copyable
                 sales-team::box.k2.dev    // live federated form when we have a host
                 [ Change handle… ]        // opens confirm
```

Agent Name save: no modal.  
Handle save: modal with D11 copy, show old → new address, require type-to-confirm or checkbox.

### 11.2 Workspace tab (the local name bug)

Replace `primaryAgent.name` from `k2so_agents_list` with `agentDisplayName(projectPath)` (display). Subscribe to `sync:projects`. Optional second line: handle in muted mono.

### 11.3 Nav / Active / Icon rail

Keep `project.name` (display). After migrate they already hold the pretty string.

### 11.4 Chat / Inbox pane headers

Already `agentDisplayName()` — remain display.

### 11.5 Pinned tab strip

Stay `Chat` / `Inbox`. Out of scope.

---

## 12. CLI

| Command | After |
|---|---|
| `k2 msg <handle>` | Canonical, local |
| `k2 msg "<display>"` | Alias via `projects.name` |
| `k2 msg <handle>::<host>` | Federated |
| `k2 msg <handle>/<sidecar>` | Unchanged grammar; first segment is handle |
| `k2 whoami` | `workspace` / `address` / `primary` = handle |
| `k2 workspace set-agent-name` | Display only (fix stale help that says it does/doesn’t touch `name:`) |
| `k2 workspace set-handle` | New; confirm flag required |
| `k2 agent set --name` | Display only |
| `k2 connections list` | Show `handle::host`; after heal, new handle |

---

## 13. What must not break

| Thing | Why it’s safe |
|---|---|
| Folder path / clone / git | Untouched |
| `projects.id` UUID | Untouched; inbound federation still keys on it |
| Live canonical PTY | Map key is `project_id` |
| Local `k2 msg` by path or UUID | Same |
| Sidecar table 0102 | First segment becomes handle; ordinals/slugs unchanged |
| Owner display name | Separate |
| Mixed-version federation for **unspaced** names | Handle often equals old token (`sales`) |
| Mixed-version + **spaced** names | New clients slug-match; old CLI was already failing |
| `/v1/w/<pretty-or-folder>/host-sessions` | `projects.name` + basename still resolve; handle added |
| Pinned Chat session / host-session UUID | `project_id` + session id, not the name |

---

## 14. Tests (must fail loud)

No `unwrap_or` in assertions. No skip-if-missing.

1. **Backfill:** workspace with `display_name: Sales Team`, `name: Sales Team`, `projects.name = Sales Team` → display stays `Sales Team`, handle `sales-team`, `name:` `sales-team`, aliases include `sales team` + basename.  
2. **No display_name:** only `name:` / `projects.name` → copy into `display_name:`, then slug handle.  
3. **Already slugged:** `sales` / `sales` → handle `sales`, no bogus `-2`.  
4. **Collision:** two “Sales Team” (if they exist) or `Sales Team` + `sales-team` → second handle `sales-team-2`, both boot.  
5. **`set-agent-display-name`** after migrate does **not** change `projects.handle` or `name:`.  
6. **`set-handle`** without confirm flag → error; with flag → handle changes, old handle aliased.  
7. **Local resolve:** `k2 msg sales-team`, `k2 msg "Sales Team"`, UUID, path. Ambiguous display → AMBIG with both handles.  
8. **Roster:** `agent` is handle; aliases contain pre-slug + basename.  
9. **Send gate:** stored `sales team` + want `sales-team` → connected. Stored `cortana` + roster alias `cortana` → connected. Stored `sales` + want `sales-team` → **not** connected (D20).  
10. **Lazy heal:** after roster fetch, a **matching** row’s `agent` becomes handle. Unrelated leftover `sales` is left alone.  
11. **Inbound:** still UUID → `deliver_live`; ignore `to.name`.  
12. **Workspace tab:** display name, not skills folder.  
13. **Label push:** rename display updates live label via **bare `project_id`** lookup.  
14. **Sidecar:** `handle/reviewer` still resolves; `Sales Team/reviewer` rejected (`/`). Live sidecar slugs (`scout_v3`) unchanged.  
15. **Uniqueness:** second `set-handle sales-team` → error naming the other workspace.  
16. **Alias collision:** two `Cortana` folders → 0103 still applies; second basename alias skipped.  
17. **Create path:** `projects/create` / hire inserts `handle`; missing handle does not 500.  
18. **`/v1/w/sales-team`** and `/v1/w/Sales%20Team` both resolve the same workspace when name is `Sales Team` and handle is `sales-team`.  
19. **`[from]`:** CLI default and `humanize_chat_from` stamp handle, not display.  
20. **Renderer roster:** `resolveRemoteWorkspacePath("sales team")` hits handle `sales-team` via alias/slug.

---

## 15. Implement slices (suggested)

| Slice | Scope | Done when |
|---|---|---|
| **A — schema + migrate** | 0103 `handle` + aliases table; boot backfill §8; create-path handle; tests 1–4, 16–17 | Display copied; handle slugged; daemon still boots on alias collisions |
| **B — readers** | resolve_workspace + `/v1` + wiki + connections; roster D8/D9; CLI **and** renderer slug-match; whoami/`workspace_address_name`; `[from]` helper; send gate D10 | Slug-equal federated names work; `/v1/w/<handle>` works; already-broken `sales` rows still 403 |
| **C — writers** | split set-display vs set-handle; stop lockstep; confirm copy D11; hire seeds handle only on create | Display rename does not break federation |
| **D — UI** | Agent tab Handle field + warning; Workspace tab display; help text (not “federated address”) | Workspace tab matches nav |
| **E — CLI + skill** | `set-handle`, k2-cli whoami/msg docs, stale comments | Skill says handle is the address |

Do not ship C without B (otherwise we split fields and matching still exact-strings).  
A+B does **not** claim to fix pre-rename leftover tokens. Re-pair after C is the documented heal.

---

## 16. Docs to update at implement

- `WHATS_NEW.md` (0.40.100 in-progress): display vs handle; migrate; federation.  
- `agent-display-name.md` — banner: **superseded** by this PRD.  
- `workspace_routes.rs` comment that set-display “does not touch `name:`” — already stale; after C it becomes true again.  
- `k2 agent set --name` / Custom persona help.  
- `cli/k2` skill: handle is the address; display is cosmetic.  
- Wiki: [[Feature - Workspace Agent Name and Handle]] (pointer).

---

## 17. Out of scope follow-ups

- Key `workspace_remote_connections` on remote workspace UUID (true name-independent gate).  
- Guess-heal a leftover `sales` row onto the only roster agent on that host.  
- Expire aliases after N days / after both sides seen on new handle.  
- Enable sidebar Rename as display-only.  
- Federated sidecar addresses.  
- POST-ify `set-agent-display-name`.  
- Changing live sidecar `slugify_custom_name` results.

---

## 18. One-page flow (existing workspace)

```
today:  display_name = name: = projects.name = "Sales Team"
        folder = Cortana
        roster agent = "sales team"
        peer reverse row = cortana::us
        our connection row = maybe "sales" or "sales team"

migrate A:
        display_name = "Sales Team"          # copied / kept
        projects.name = "Sales Team"         # nav
        handle = name: = "sales-team"
        aliases = ["sales team", "cortana"]

runtime B:
        they k2 msg cortana::us        → alias → our UUID → deliver_live
        they k2 msg "sales team"::us   → new CLI slug-match; old CLI NONE (already broken)
        they k2 msg sales-team::us     → handle
        leftover peer row agent=sales  → still 403; re-pair once
        we k2 msg <their-handle>::them → slug-match their roster
        display rename to "Sales"      → nav "Sales"; handle still sales-team
        handle change to "revenue"     → WARN; alias sales-team; peers must update
        /v1/w/sales-team and /v1/w/Sales%20Team → same pinned workspace
```

Pretty name is free; handle is the address; migration copies the first and slugs the second; slug-equal spellings keep working; already-dead pre-rename tokens re-pair once and then stay fixed.
