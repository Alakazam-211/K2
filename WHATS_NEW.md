# K2 — What's New

User-facing highlights of recent updates. Developer-facing per-version notes
live under [`docs/changelog/`](docs/changelog/) (`release-notes-X.Y.Z.md`).

## 0.40.136 — Skin guests can read that agent's store, as themselves

A logged-in skin guest can list, get, and query **that agent's store** — that workspace only. Store on Documents does not grant store on Sales. Unassigned guests stay Thread-only. The pass stays on the server (cookie), never in the browser. Guests never get a database password. Grid and the terminal stay off.

`k2 publish run --skin` proxies `GET /cli/store/list`, `get`, and `query` on the published origin. Session reads stamp `k2.skin_principal` so dump policies can filter per guest. Named platform tokens cannot use store. Writes stay off.

---

## 0.40.135 — Skin guests can read wiki on the agents you grant

A logged-in skin guest can list and open that agent's wiki — **that workspace only**. Wiki on Documents does not grant wiki on Sales. Unassigned guests stay Thread-only. The pass stays on the server (cookie), never in the browser. Grid and the terminal stay off.

`k2 publish run --skin` proxies `GET /cli/wiki/index` and `GET /cli/wiki/note` on the published origin. Fleet wiki, seed, serve, and public chat stay off. Chat history and Chatter are not in this cut.

---

## 0.40.134 — Skin guests can use Tickets on the agents you grant

A logged-in skin guest can list, open, comment, answer, and resolve Tickets **on that agent only**. Tickets on Documents does not grant Tickets on Sales. Unassigned guests stay Thread-only. The pass stays on the server (cookie), never in the browser. Grid and the terminal stay off.

`k2 publish run --skin` proxies those Ticket routes on the published origin (`/cli/feedback/list`, `show`, `create`, `comment`, `answer`, `resolve`). Waiting-count and assign stay off. Wiki, chat history, and Chatter are not in this cut.

---

## 0.40.133 — Skin guests can use files on the agents you grant

A logged-in skin guest can list, read, write, and watch files **in that agent's folder** — not the whole box. Files on Documents does not grant files on Sales. Unassigned guests stay Thread-only. The pass stays on the server (cookie), never in the browser. Grid and the terminal stay off.

`k2 publish run --skin` now proxies those file routes on the published origin (`read-dir`, `read-file`, `write-file`, and a per-agent live watch). Login still guards Thread and files; `/` and `/assets/*` stay public.

---

## 0.40.132 — Skin access is per agent, then what they may do there

Skin Access roles grant a guest **specific agents**, and **specific functions on each one** (Thread, files). Files on Documents does not grant files on Sales. Guests still log in with a username and password; they never copy a key. Grid and the terminal stay off.

`k2 publish run --skin <dir>` serves your `login.html` when that file is in the folder (otherwise the bundled sign-in). Guests can tap Thread choice cards and fill secrets. A workspace Agent-tab toggle (off by default) lets that agent run existing `k2 skin` / `k2 skin-token` commands.

---

## 0.40.131 — Skin platform tokens are not per-guest keys

Skin Access mint is a **named platform token** (`k2 skin-token create --name vercel --agent sales`), not a key hung off a guest. Guests still sign in; that session is their access. Login does not hand them a copyable key. Grid and the terminal stay off. Existing static mints become platform tokens on first boot (name from the old username).

---

## 0.40.130 — Skin gateway login page and no stale assets

`k2 publish run --skin <dir>` always serves a sign-in page at `/login` (session expiry no longer 404s). Static files send `Cache-Control: no-store` so a published UI updates without a hard refresh. `/` and `/assets/*` stay public — login only guards Thread; do not put private files in that folder.

---

## 0.40.129 — Host a skin UI with k2 publish

`k2 publish run <name> --skin --port <n>` puts a login + Thread site on a nested URL (`https://<name>.<your-host>.k2.dev`). Guests use the username and password from Skin Access. The pass stays on the server — not in the browser. Grid and the terminal stay off. Your own static files: `--skin <dir>`. `k2 skin` is still the guest list, tokens, and rooms; it does not serve the site. Caddy / `skin.` as a special hostname is not required.

---

## 0.40.128 — Granted workspaces run SQL as that workspace’s Postgres role

A workspace granted onto a shared database — and `k2 store` from that workspace — connect as **that workspace’s** Postgres role, not the owner’s agent and not the migrator. Dump RLS keyed to the role now applies to both the agent (`k2 db dsn` / `k2 store`) and a human who shares that DSN. The owner still connects as `{dbname}_agent`. Migrate, dump, and restore stay the migrator. `k2 db dsn --workspace` fetches that workspace’s LOGIN (owner backend), and restore recreates grant roles so dump policies bind on a fresh box.

---

## 0.40.127 — Agents can publish from a cell

`k2 publish run` works from an agent session (scoped cell token), not only from an owner shell. Status, `ps`, and Connect are unchanged.

---

## 0.40.126 — Skin agent display names

GET `/cli/skin/agents` includes `displayName` (guest-facing label; `handle` stays the id). Agent Thread posts stamp the room handle, not `k2`. Skin guest `from` is still the username.

**`k2 msg agent::host --inbox-wake`** uses the same federation plane as live `k2 msg`. Connected peer is the gate — not a Connect token. Wake is a tray pointer, not file bytes in the PTY.

The create-DB grant lives on each workspace’s Agent tab (Settings → Workspaces), not inside Settings → Data.

---

## 0.40.125 — Skin files read/write + live events

A customer key can list, read, write, and watch **that agent's folder** when you mint `files:read` / `files:write` — same rooms as Thread. Missing files cap stays Thread-only. Empty rooms stay dark. Grid/PTY still never. Not host-wide `/cli/sessions/events`. Skin Thread posts inject `[from <username>] [thread:<addr>]` into the agent's terminal; `via=compose` stays 403.

---

## 0.40.124 — Skin agent rooms, study, store 403 names the DB

**Skin Access keys name which agents may use Thread.** Empty list is no Thread, not every agent on the box. New mint requires `--agent`. Existing keys go dark on Thread until you assign rooms (same secret). Overlay WS 403s before upgrade if that conversation is not the allowed agent’s pinned Chat. Sidecar chats (`sales/reviewer`) stay 403. Grid still never.

**Skin Access username/password** is plumbing: the skin POSTs `/cli/skin/login` and gets an HttpOnly `k2_skin_session` cookie. The skin owns the login UI. Static `k2skn_` keys stay for servers. This is not Connect login (`k2_session` / Server Access).

**CLI.** `k2 study` is daemon-optional pages (Fair Source, people lists, errors) — not a second `--help`. `k2 agent list` reports `local_transport_denied` when the OS blocked the socket (daemon health unobserved). `k2 skin hydra on|off`. Linux: Hydra starts on daemon boot if enabled.

**Database.** Unscoped `k2 store` / migrate 403s name the resolved DB (`dbId`, `dbName`, `resolvedVia`). A write grant on a newer DB does not unlock put on an older read grant.

---

## 0.40.123 — Skin SPA door, migrate that sticks, Hydra toggle

**Skin Direct** can serve an on-box UI: if you set a UI port, Caddy proxies `/`, `/assets`, `/_next`, and `/app` to that loopback app (not the whole daemon). Grid, login, and `/v1` stay 403. Mail Enable re-applies Caddy so the mail hostname shows up without a second front-door click. Direct custom domains still need **:80** (or dns-01) for certificates.

**Database.** `k2 db migrate` applies `0001_name.sql` files and says so; empty or `init.sql` is a loud error, not “already applied.” A second run with unchanged files skips. A rewritten already-applied file is refused. The workspace agent role can SELECT `_k2_store` and the migration ledger (repair GRANT on put/migrate). Granting another workspace **write** uses sequence privileges Postgres actually accepts (`USAGE`/`SELECT`/`UPDATE`, not `INSERT`). Settings → Data “Add workspace…” opens down and to the right.

**OIDC.** Settings → Skin Access **Hydra** toggle starts a Linux sidecar if `hydra` is on PATH (loopback 4444/4445). Enabling skins does not start it. Mac shows the Linux banner. Login/consent UI and public OIDC on :443 are next.

---

## 0.40.122 — Database passport, Skin Direct + mail on one :443

**Database.** The workspace flag **Agents can create databases** (Settings → Data) only gates `k2 db create`. If a workspace already owns a DB — or has a grant — its agent can list, fetch a DSN, migrate, dump/restore, and use `k2 store` without flipping that flag. Cross-workspace **read** grants stay read (write still 403). Email Hosting Enable now pulls the Stalwart tarball from Alakazam Labs' pin (same 0.16.10).

**Skin roster.** Workspace agents can `k2 skin user list` / `k2 skin-token list` (no raw secrets). Minting and the front-door stay owner-only; a valid agent passport gets `owner_only` (exit 3), not “invalid token.”

**Box :443.** Skin Direct and Email Hosting no longer coin-flip for the box's port 443. One Caddy (the Skin front door) Host-routes Direct to the daemon (Thread path-filter) and the mail hostname to Stalwart on loopback 8443. Nested `skin.<your-sub>.k2.dev` is still Connect. Mail Enable uses HTTP-01 (not tls-alpn on 443) when Skin Direct will own 443. Do not Enable mail tls-alpn and Skin Direct on the same 443 until you apply this cut.

---

## 0.40.121 — Skin Access, quieter Thread/Chatter, Grok one Return

**Skin Access** is a guest list for custom UIs that talk to agents **without** a terminal. Settings → **Skin Access**: add a username, mint a `k2skn_` pass with Thread read/post. Those tokens never open the grid. A Caddy **front door** (Caddy on PATH) path-filters Thread/overlay/`/boot-status` and can listen as nested `skin.<your-sub>.k2.dev`. Opt-in catalog pack: `k2 agent context add skin:roster`.

**Thread and Chatter** no longer stay rendered behind Terminal. The PTY keeps running when you leave it; overlay tabs load on demand, **25** messages at a time (Load older / scroll up). A message you send on Thread shows in the Thread list (it still wakes the agent in the terminal with `[thread:…]`).

**Message the agent:** Ctrl+B injects STX (with Esc / Ctrl+C / empty Return). **Grok**’s default submit is paste + **one** Return — the extra Return was starting a new turn after steer. Other models stay two Returns unless you changed Submit keys.

Agents told `[thread:addr]` should reply with `k2 thread`, not the TUI. Generated `.k2/agent/SKILL.md` is no longer planted (leftovers on disk are left alone).

---

## 0.40.120 — Chatter tab, database sidecar, compose slash, Settings regroup

Agent sessions are now **Terminal | Thread | Chatter | split**. Chatter is the agent-to-agent mailbox (`k2 msg` / `k2 talk`) with the same bubbles as Thread and **no** compose box. Thread is still the human overlay.

**Message the agent:** `/` typeahead for `/compact` and `/goal`; **+** uses the native file picker. Esc and Ctrl+C from the box cancel the current turn (PTY bytes); empty Return confirms a TUI prompt. Typed Return still sends. Settings → LLMs **Submit keys** is per-model (which keys finish a live inject).

Settings sidebar is grouped: **K2 Server** (Tunnel, Server Access, Connected Servers, API Keys, Companion), Editors / Fonts, Logs, Sidecars (**Database**). Companion has App Store and Play QR codes. Connected Servers: add at the top, then search. Server Access: search the user list; pick a starting role when adding a user.

Linux boxes can grow a **database sidecar** the way they grew mail: `k2 db enable` supervises distro Postgres (loopback only — not a static-IP feature). Agents mint a per-workspace DB with `k2 db create`, apply `.k2/db/migrations`, dump/restore inside the workspace, and store JSON documents with `k2 store`. Settings → **Database** is the owner surface.

Off-box `*.k2.dev` is the existing publish door: `k2 publish subdomain create <label> --target localhost:<port>` (the port is already listening — do not `k2 publish run` Postgres). `k2 db status --json` reports `port` and that hint. There is no `k2 db expose`.

Hire an interview workspace with write in one shot: `k2 agent hire … --db-access write` then `k2 db create` (create is still not implicit on hire). `GET /v1/w/<ws>/db` returns applied migrations + size; `POST /v1/w/<ws>/db/restore` restores onto a fresh workspace. Re-running migrate with the same checksum is a no-op; a rewritten already-applied file is refused.

Postgres is fenced so it cannot starve agents (systemd memory cap + query GUCs). `k2 db doctor` reports RSS vs the cap.

`k2 hostmail enable` now requires `--hostname mail.acme.dev` (the daemon already 400'd an empty body). `k2 db enable` still POSTs `{}`.

---

## 0.40.119 — LAN federated connections use the saved IP:port

After you Pair a LAN peer, picking an agent under a workspace's **Federated connections** no longer looks up `<LAN-IP>:38471.k2.dev` (that host was never in Settings → Connections). The address is `agent::<LAN-IP>:38471` against the server you already signed in.

What's New star drawer: K2 logo on the tag, the card stays centered, the drawer sits behind it.

---

## 0.40.118 — Pair as federated peers over LAN (no tunnel)

Two machines on the same network can **Pair as a federated peer** without buying a `*.k2.dev` tunnel. Add Server as `http://<LAN-IP>:<port>`, enable federation, Pair. `k2 msg sales::10.2.40.28:38471` is the address shape. HTTPS to a private IP is still refused.

What's New has a little drawer on the right — if you're enjoying K2, a star on GitHub helps a lot. Hidden in air-gap builds.

---

## 0.40.117 — Terminal hitch; inbox files keep the note

Quiet agent sessions no longer freeze the window for a second or two while you type in **Message the agent**. The grid still pauses a runaway TUI; catch-up no longer stop-the-worlds the compose bar, and idle panes no longer log a false stall.

`k2 msg --inbox-wake` / `--inbox-silent` with files now keeps the cover note you typed (it used to drop the prose and only attach the files). A leftover tray-teardown bug that printed `exit 1` on a successful send is gone.

Agents extra columns: the divider between columns now stays under the cursor while you drag (including when the window is zoomed). It used to run ahead, pause, then leap.

`k2 mail link add` for an IMAP app-password account no longer dies with `Query returned no rows` before it talks to the server (Gmail OAuth was fine). Pipe the password as before.

**Message the agent** has a `/` picker next to attach — `/compact` and `/goal` go to the TUI as real slash commands, not as typed text.

A paper-note button in the top bar (next to the timer) opens a **K2 cheat sheet** of CLI nouns (`k2 msg`, `k2 thread`, `k2 inbox`, …).

---

## 0.40.116 — Codex yolo; pinned chat uses your LLM launch script

Switching the pinned Chat dropdown now launches that session with the command from **Settings → LLMs**, not a stripped resume line. Codex’s default is `codex --yolo`. If you already customized Codex, that command is left alone.

---

## 0.40.115 — Final air-gapping patch

A last air-gapping patch.

---

## 0.40.114 — Thread tab, agent-to-agent message log

Agent sessions get **Thread | Terminal** tabs (default Terminal; the PTY stays running when you open Thread). The agent can post to the Thread with `k2 thread`, including choice cards and a secret field that never lands in the log. `k2 msg` / `k2 talk` are still injected into the terminal, and a copy is recorded so you can see chatter later — not mixed into the human Thread.

### What to try

1. Open pinned Chat — underline **Thread | Terminal**. Switch to Thread; the agent should keep running in the background.
2. From an agent: `k2 thread <workspace> "hello"` — appears on Thread, not as a TUI line.
3. `k2 thread ask <workspace> "Ship?" --options "Go,Hold"` — card on Thread; tap Go, or type in chat instead (voids the card).
4. `k2 msg` another workspace — that still wakes them; a chatter record is stored (not on the Thread tab).

---

## 0.40.113 — Switching servers no longer keeps the last machine's tabs

Switching **this window** from This Mac to a remote server now clears the tab strip immediately. A leftover screenshot or HTML dashboard from your Mac is not requested on the remote — those `/var/folders/…` 400s stop. Switch back and your Mac tabs restore from **that** machine.

A background window no longer polls HTML/markdown files every 2 seconds while it is unfocused.

### What to try

1. Open a screenshot on This Mac, switch that window to a remote — strip empties, no 400 storm in the console.
2. Switch back — Mac workspaces return, including the screenshot tab (the tmp file itself may already be gone; an error pane is fine).
3. Two windows (Local + remote): blur the remote window; its HTML dashboards stop polling.

---

## 0.40.112 — Linked mail lists without walking the whole mailbox

Linked Gmail/IMAP is live IMAP, not a local store. Unfiltered `k2 mail messages` now fetches the **newest page** of one folder (IMAP sequence numbers). It does not `SEARCH ALL` tens of thousands of UIDs. `--limit` is still 25 (max 200).

Filtered search (`--from`, `--query`, `--unread`) is a **≤30-day window**. `--since` still means **on/after** that date. To look at a month in the past, pair it: `--since 2017-03-01 --before 2017-04-01`. `--since 2017-03-01` alone (years of mail) **errors** — add `--before`. `--since 7d` is still the last seven days through now. `--from`/`--query` with no dates inject last 30 days and **print both bounds** on every result (hit or miss), so empty cannot mean “does not exist.”

SEARCH that matches more than 1000 messages fails loud (`narrow --since/--before`). Hosted (on-box) mail is unchanged.

### What to try

1. `k2 mail messages` on a huge linked Gmail — should return the newest 25 without a 30s stall.
2. `k2 mail messages --from someone` — prints `searched last 30 days…` even if empty.
3. `k2 mail messages --since 2017-03-01` — usage error asking for `--before`, not one silent month.

---

## 0.40.111 — Don't hide your workspaces behind a stub `k2.db`

If both `~/.k2/k2.db` and `~/.k2/k2so.db` exist as real files, K2 now opens the one that actually has your workspaces. A stray empty `k2.db` (touch, `sqlite3 ~/.k2/k2.db`, a test) used to win on name alone — the app looked factory-reset, mail and chats gone. **Nothing was deleted.** The live file is still `k2so.db` until a later rename. This update just stops picking the stub.

Gmail `k2 mail read` no longer reports `daemon_unreachable` when the daemon is up and the mailbox is large. One IMAP session, cached folder STATUS, longer HTTP timeout.

### What to try

1. If workspaces vanished after an update: look in `~/.k2/` — if both `k2.db` and `k2so.db` are real files, restart this build. The sidebar should come back. Do not delete either file.
2. Linked Gmail: `k2 mail read` of a Sent or Inbox id should return a body, not hang until timeout.

---

## 0.40.110 — Air-gapped LAN-only: Caddy in front

An air-gapped box on a private network keeps the daemon on **loopback** and puts **one** Caddy port on the LAN. That port is a high random TCP port — example **38471** — not 80, 443, 8080, or 8443. HTTP and WebSocket both go through it. Add Server is `http://<LAN-IP>:38471`.

Do **not** set `K2_LISTEN=lan` on that image. That flag is the lab path (client talks to the daemon with no proxy). On the MasterControl image it would open a second LAN door next to Caddy.

Cloud / Connect defaults are unchanged. `K2_AIRGAP=1` still has to be on the process **before first start**.

Runbook: `.k2/prds/runbook-airgap-linux-image-v1.md`. Starter Caddyfile: `docs/caddy-airgap-lan.Caddyfile`.

### What to try

1. Golden image: `K2_AIRGAP=1` only, sticky `~/.k2/daemon.port` = `60710`, seed a connect-user, Caddy `38471` → `127.0.0.1:60710`.
2. Laptop Add Server: `http://<LAN-IP>:38471` — not `:60710`, not `https://`, not `:443`.
3. Firewall/IDS: that Caddy port only.

---

## 0.40.109 — Air-gap + LAN listen; server list drops :443

A daemon can run **air-gapped**: set `K2_AIRGAP=1` on the process **before first start** (systemd/launchd env). It will not start a tunnel, phone Connect/cert, or hit GitHub for updates. `k2 connect login`, `k2 publish subdomain`, and `k2 daemon install` refuse the same way. Strings stay in the binary; they just must not run. This is not an installer `--airgap` flag yet.

A second client on the same private network can Add Server with `http://<LAN-IP>:<daemon.port>` when `K2_LISTEN=lan` is set (HTTP on the sticky port, not `https://` and not `:443`). Default for both flags is **off** — cloud users are unchanged.

The top-bar server dropdown no longer appends `:443` on hosted names (`rosson.k2.dev`, not `rosson.k2.dev:443`). A LAN port still shows.

### What to try

1. Top bar server list: hosted rows should be hostname only.
2. Do **not** leave `K2_AIRGAP=1` on a box that should keep `*.k2.dev` — the Connect lease is 3 minutes. Env-only, then unset and restart, recovers tunnel.json / pairing.
3. Golden image: bake/scp the daemon (do not `k2 daemon install` on an air-gapped box), set the env on the unit, pre-write `~/.k2/daemon.port`, seed a connect-user. Runbook: `.k2/prds/runbook-airgap-linux-image-v1.md`.

---

## 0.40.108 — Default model, ticket wake, mail drafts, hire API

A workspace can have a **default model**. Settings → the workspace **Agent** tab has chips plus an optional **force this model when resuming a dead chat**. The host-session API can send `model` on spawn; that wins over the workspace default, which wins over the preset. A live session ignores a model change (no 400).

Answering a **ticket** wakes the workspace agent the same way `k2 msg` does, so the reply lands in the pinned chat instead of a sleeping sandbox tab.

`k2 mail draft` can compose a **new** message (`--to` / `--subject` / `--body`), not only a reply. It still APPENDs to the human's Gmail Drafts and never sends. Listing All Mail and then `read` / `draft <id>` works.

Programs with an API key can **hire** a workspace, write wiki notes, and stack context without the owner token:

```
POST /v1/w
POST /v1/w/<ws>/wiki/notes
GET/POST /v1/w/<ws>/context
```

New workspaces need a `*` workspace grant (or the owner). A draft is still not a send. Spinning up a VM stays on k2-dev-web.

### What to try

1. Settings → a workspace Agent tab: set a default model, spawn a host-session with and without `"model"` in the body.
2. Answer a ticket for a sleeping agent — the pinned chat should wake with the reply.
3. `k2 mail draft --to someone@x --subject "Hello" --body "…"` and open Gmail Drafts.
4. `POST /v1/w` with `path`, optional `wiki` / `context` / `layers`, then `POST /v1/w/<handle>/host-sessions`.

---

## 0.40.107 — Workspace Resources; pinned-chat icon stays put

**Resources** are files you pick, not whatever HTML tab happens to be pinned. In the Files tree, right-click a file → **Add to Resources**. The Files drawer has **Workspace Resources** at the top (Environment and AI Config stay, all three start collapsed). Those same files show in the project **Resources** drawer. Click or drag one onto the project dashboard: HTML still uses the sandboxed page; CSV/PDF/and the rest open in the file viewer **on the dashboard**, not as a new Agents tab. Right-click → **Remove** in either drawer. Existing pinned HTML is copied into Resources once so nothing vanishes; pinning a tab no longer *defines* the list.

The closed pinned-chat session control shows the same provider icon as the dropdown rows, next to the name.

### What to try

1. Files tree → right-click a CSV → Add to Resources. Expand **Workspace Resources**; AI Config and Environment start collapsed.
2. Open a project: that file is in **Resources**. Click it — it should land as a dashboard pane, not jump to Agents. Drag to split.
3. Pin an HTML tab: the tab pins; it should **not** appear as a new Resource. Pins from before this update should already be in the list.
4. Switch the pinned chat to another agent: after the list closes, the icon next to the title should still match that agent.

---

## 0.40.106 — Idle agents sleep; Settings drag stays connected

Workspaces stay **Active for N hours** after something actually needed them (you visited, a Project or ticket woke them, a heartbeat fired, or another agent messaged them). After that they sleep and free RAM; the next need `--resume`s. A daemon reboot does **not** respawn the whole fleet. The Active bar Dismiss works even if that agent is running.

Heartbeat **fires** keep a workspace warm; having a schedule is not immortality. API host-session tabs still use their own completion reaper.

Dragging workspaces around in **Settings → Workspaces / Agents** no longer floods the remote tunnel (eat-echo + a Tickets badge count instead of one list per workspace). A current daemon also returns workspaces **inside** `projects/list`, so boot / host-switch / a peer's color change is one GET, not one per workspace. Rename paints immediately like color.

Waking a sleeping chat (`k2 msg`) now puts the first message on the agent’s command line when the TUI supports it (same as the API). If it doesn’t, we wait 1s after the input box is up (`?2004h`) then paste — or 7s for TUIs that lie about that signal.

If a remote `/cli` call fails while the host still looks connected (`Failed to fetch` / handshake eof), the app tries **once more immediately** (to evict a dead WKWebView socket) and then probes — not five times over ~3.6s. Login and host restart/update still use the longer retry window.

### What to try

1. Lower **Keep workspaces Active for** (Settings → General → Workspaces) on a busy host; unused chats should disappear after N hours + ~15s.
2. Restart a headless daemon: agents should stay down until a visit, message, heartbeat, or ticket reply.
3. Active bar → Dismiss on the workspace you are looking at, even while it is working.
4. On a busy remote host, drag-reorder workspaces in Settings without losing the connection. DevTools: one `projects/list`, not N `workspaces/list`.

---

## 0.40.105 — Whoami tells the truth; API tabs stay dead

A sidecar that gets moved into the pinned Chat is the workspace agent.
`k2 whoami` now says **canonical** and the workspace address (`sales`),
not `sales/2`, matching `k2 msg sales` and `K2_CELL`.

API host-session tabs that were already reaped no longer come back as
open Claudes after a server reboot or daemon update. The live cap still
applies to new API calls; restart was skipping it. Closing those tabs
still **hides** them (the session can resume). Right-click a tab →
**Forcefully reap all tabs** kills them and clears the index so they
cannot revive.

### What to try

1. Pin a sidecar conversation as Chat, then `k2 whoami`: `role:
   canonical`, `address:` the workspace name.
2. After a host reboot, interview / API workspaces should not refill
   with old host-session tabs.
3. Right-click a tab: **Forcefully reap all tabs** is in the menu.
   Chat and Inbox stay.

---

## 0.40.104 — Linux hosts stay up, Message &lt;name&gt;

Headless Linux boxes (for example RPMAVS) were growing to tens of
gigabytes and the kernel killed the daemon about every three hours.
The desktop Mac on the same version did not. Linux file-watch was
treating every *open* of `ROLE.md` as an edit, queuing path strings
without bound. K2 now ignores open/close, only reacts to real
creates/writes/renames, and caps that queue. Remote **Download** of
the daemon also waits up to five minutes for the file instead of
failing at 30 seconds.

The compose bar placeholder is **Message &lt;name&gt;** — the workspace
name from the Agents list — instead of “Message the agent.”

New workspaces no longer get leftover `.harvest-0.32.7-done` and
`.unification-0.37.0-done` receipts. Those were migration stamps for
old layouts, not part of hire/add.

### What to try

1. On a Linux host that used to fall over every few hours: after
   this update, RSS should stay in the same order of magnitude as a
   few minutes after start, not walk toward 60 GB.
2. Open a workspace terminal: the empty compose bar should say
   **Message sales** (or whatever that agent is named).
3. Hire or add a workspace: no `.harvest-0.32.7-done` /
   `.unification-0.37.0-done` in `.k2/`.

---

## 0.40.103 — Your name on Feedback, remotes recover, shared size

A Feedback reply from your phone or another Connect login is stored
and injected as **you**, not the box owner. The owner still frames
with the server name; Alice's comment is `[from alice]`.

If a remote host (for example RPMAVS) starts returning 404s on
**Message the agent** or `/cli/*`, K2 reloads the webview as soon as
the OS still says the host is up — same reset as switching Local and
back, without the bounce. DevTools: filter `[remote-path]`.

**Projects** and **Agents** no longer stack live terminals on top of
each other. The first visit to Projects still opens the dashboard;
coming back is instant. Quiet terminals stay quiet — they do not
tear down and redraw every twenty seconds. Remote dashboards
handshake two panes at once instead of one-by-one.

Two people on the same workspace: opening **Projects** always sizes
those panes. Coming back to **Agents** only takes the size if you
are the only viewer. If someone else is on the agent, this client
does not steal their columns. The viewing pill can say **project
viewer** when you last sized from a dashboard.

The **Message the agent** bar is on screen before the first spawn, so
the terminal is not measured full-height and then shrunk. WebGL in
terminal settings is no longer labeled Experimental.

### What to try

1. From a Connect user (not the host owner), comment on Feedback —
   the thread and the agent's inject should say that user.
2. On a remote host, send from **Message the agent**. If it 404s,
   it should recover without switching to Local and back.
3. Open Projects, leave, come back — dashboards should still be
   there, no serial spawn. Leave a quiet terminal up; it should
   not flicker on a ~20s clock.
4. Two clients on one workspace: open Projects on yours — it
   sizes. Stay on Agents while the other person is on that
   workspace — yours should not reclaim columns.

---

## 0.40.102 — Readable Project and Feedback chat, no sliver window

**Projects** and **Feedback** now render markdown the same way the rest of
K2 chat does: headings, lists, code, and links, with text you can
select. Your own messages stay a wash; everyone else stays inset.
Spacing is tighter so a long thread is easier to scan.

If the last saved window was only a title-bar sliver on the edge of
the screen, relaunch no longer treats that as usable. K2 now requires
a real visible rectangle (at least 400×300 on screen). Anything
smaller, or only a sliver peeking onto a display, centers a normal
1400×900.

### What to try

1. Open a Project chat or Feedback and send a list or a code block —
   it should render, and you can select the text.
2. If a previous update left you with a razor-thin window, quit and
   reopen: you should get a centered 1400×900.

---

## 0.40.101 — Project tags in the nav, honest host switch

The Agents list now shows **which projects** a workspace belongs to
instead of the git branch. Two chips fit the row; the rest collapse
to **+N**. Click a chip to open that project. The outline uses the
same accent as buttons, with the theme's primary text.

Switching to another host (for example RPMAVS) no longer leaves the
previous box's agents painted under the new name. If the remote list
is slow or fails, the roster is empty until that host answers.

The window reopens at its last good size and position. If that frame
is tiny, off-screen, or on a display that is gone, K2 centers a
normal 1400×900. Quit (including ⌘Q) saves the frame.

The **Message the agent** bar stays one line when empty — it no
longer grows to fit the placeholder on launch.

### What to try

1. Agents list: project chips under each name; 1–9 still on the right.
2. Switch host: old names vanish immediately; new roster fills in.
3. Quit and reopen: same window, or a centered default if the last
   frame was unusable.

---

## 0.40.100 — Sidecars, catalog, and workspace polish

### Sidecars — extra chats in a workspace are not a second agent

Opening another agent in **sales** used to start a second PTY that
thought it *was* sales. `k2 msg sales` always hit the primary. A raw
session UUID was the only other address, and people do not type those.

Extra harness sessions are now **sidecars** of the workspace agent
(same AGENT.md / inbox). They learn that from env + **`k2 whoami`**,
not from a spawn prompt (no token burn on open).

```
k2 whoami                 # role, address, primary, session
k2 msg sales              # primary
k2 msg sales/1            # first unnamed extra (durable — closing tabs
                          # does not renumber)
k2 msg sales/reviewer     # renamed in Chats
```

Reply stamp is typeable: `[from sales/reviewer]`. Asleep sidecars
wake **that** chat, not the pinned one. Hyphen (`sales-reviewer`) is
still a workspace name, not a sidecar.

New heartbeats land in the **pinned** chat by default, with a loud
hint to train a sidecar (`k2 heartbeat session <name> --set sales/reviewer`)
instead of dumping a flow on the primary. `--set` accepts a sidecar
handle or the old session UUID + `--provider`.

Claude / Grok / Pi launches get a **whoami fact sheet** on
`--append-system-prompt` (Grok: `--rules`) — the fields, not “run
whoami” / “msg the primary.” Codex / Gemini / Cursor / Hermes stay
env only (they already load AGENTS.md). `k2 whoami` is a lookup.

### Compose bar remembers sends; Esc cancels the turn

Up/Down in the compose box walks the last 50 sends for that
workspace (shared across clients for now). Esc or Ctrl+C from
compose injects the same cancel bytes the terminal would, **without**
stealing focus out of the box.

### Chat history, tab names, and the workspace drawer

Renaming a chat in **Chats** is the name the session tab shows
(`chat-session-tab` — custom name wins, else the provider title).
A session that a heartbeat delivers into shows the same heart-EKG
icon as the workspace drawer, to the left of the date.
API host-session tabs can stay out of the strip: per-workspace
**Hide API sessions** (they remain under Chat history → API).
The workspace drawer has connected-agents / API sections and a
per-workspace completion-sound bell (AND-gated with the global
Settings toggle — mute the workspace, or mute everything).

### Agent Name vs Handle

A workspace now has two names:

- **Agent Name** (display) — what you see in the nav, Workspace tab, and
  chat header. Change it anytime. Capitals and spaces are fine.
- **Handle** — the address: `k2 msg sales-team`,
  `sales-team::box.k2.dev`, `[from sales-team]`, sidecar
  `sales-team/reviewer`. Lowercase, spaces become `-`.

Existing pretty names are copied into Agent Name; the handle is slugged
from that (`Sales Team` → `sales-team`). If a federated link was
already broken by an old rename, re-add the connection once against the
handle — later display edits will not break it. Changing the handle
still needs a confirm: that *is* a new address.

### Wiki seeds on add (opt-out)

New workspaces and `k2 agent hire` now seed `.k2/wiki/Home.md` +
`_Index.md` by default so every agent starts with a knowledge base.
Uncheck **Seed the wiki** in Add Workspace, or pass `--no-wiki` on
`k2 workspace create|open` and `k2 agent hire`. Existing notes are
never overwritten.

### ⌘P opens Projects

The Keybindings row that still said **Review Queue** is **Open
Projects**. ⌘P (View → Projects) opens the Projects page.

### Settings / chrome

Settings gear is gone from the top bar — **GateChrome** + the
existing page tabs / server switcher own that chrome. Heartbeat
and Projects settings match the new delivery + sound fields.

Switching workspaces can auto-focus **Terminal** or **Message
agent** (thin-client only — not a daemon setting). Compose stays
on the visible workspace and keeps your caret.

**Edit with AI** (catalog, persona, theme, PROJECT.md, heartbeats)
follows **Settings → Default AI Agent**. A workspace Default Agent
still applies to ⇧⌘T / new tabs.

### Context, ROLE.md, and who is human

Authored persona is **ROLE.md** (was AGENT.md). AGENTS.md generate
and leftover harness fan-out are separate toggles — generate is on
by default; fan-out is off until you opt in.

`k2 connections list --users` lists humans on this box, not just
agents. An optional **User roster** catalog layer can put that
table in context.

**Settings → Context Catalog** (after Projects) is a host library of
packs (`pack.toml` + `layer.md`). It does not auto-stack into a
workspace — you add a pack to a stack when you want it.

### What to try

1. Open a second Claude (or Grok / Pi) tab in a workspace → run
   `k2 whoami` there → `role: sidecar`, `address: <ws>/1`.
2. Rename that chat to **Reviewer** → `k2 msg <ws>/reviewer "hi"`.
   Bare `k2 msg <ws>` still hits the primary.
3. Compose: send two lines, Up/Down recalls them; Esc cancels a
   turn without leaving the box.
4. Workspace drawer: bell mutes that workspace’s completion chime;
   Hide API sessions keeps `/v1` tabs out of the strip.
5. Settings → Context Catalog: create a pack and edit it with AI.
6. This What's New page scrolls if the notes run long.

---

## 0.40.99 — Windows reinstall replaces a leftover tunnel client

0.40.98 could overwrite `k2.exe` and `k2-daemon.exe`, but an orphaned
`frpc.exe` (the Connect tunnel) still locked `%LOCALAPPDATA%\K2\frpc.exe`.
Setup then showed **Error opening file for writing** on that file.

The installer and in-app updater now stop `frpc.exe` as well before they
copy files. Do **not** click Ignore on that dialog — that leaves a mixed
old tunnel next to a new app.

### What to try

1. **Windows:** with K2 closed (or even with a leftover tunnel still
   running), run `K2_0.40.99_x64-setup.exe`. It should install without
   the frpc file-lock dialog.

## 0.40.98 — Windows reinstall replaces the daemon; Claude launch-bar works

Two Windows dogfood fixes.

### Reinstall actually replaces the daemon

Re-running setup no longer fails with **Error opening file for writing:
…\k2-daemon.exe**. Uninstall left the detached daemon running, so the
next install could not overwrite it. **Ignore** left a new `k2.exe`
talking to the old daemon (Connecting forever).

The installer now stops `k2.exe` and `k2-daemon.exe` before it copies
files, and again before uninstall. Settings → Install & Relaunch was
already fixed in 0.40.97; this covers running `setup.exe` yourself.

### Launch-bar Claude (and other npm CLIs)

Clicking **Claude** in the launch-bar spawned a bare `claude` via
CreateProcess, which does not honor `PATHEXT`. PowerShell finds
`claude.cmd` from npm; the launch-bar did not. Grok’s native
`grok.exe` already worked. The daemon now resolves `.cmd`/`.bat` on
PATH and starts them through `cmd.exe`.

### What to try

1. **Windows:** with K2 (or just the daemon) still running, run
   `K2_0.40.98_x64-setup.exe`. It should install without the file-lock
   dialog. After launch, the daemon version matches the app.
2. **Windows:** click **Claude** in the launch-bar — it should start
   the same way as `claude --dangerously-skip-permissions` in
   PowerShell. Grok should still work.

## 0.40.97 — Terminals stay up on Projects; Windows update actually installs

Two dogfood fixes: remote and local terminals no longer die with a fake
“daemon unreachable” when you open the Projects tab (or a pinned chat),
and Windows in-app update no longer gets stuck Connecting after Download.

### Terminals recover when many panes connect at once

Opening **Projects** (or switching to a **pinned chat** like Cortana)
could leave some terminals on a red `Kessel: ws error (daemon unreachable
after retries)` while others kept working. The daemon was fine — the
client was opening too many terminal connections at once and WebKit
gave up (`Insufficient resources`).

The app now connects those streams one at a time, backs off when the
browser is out of sockets, and retries a visible error pane instead of
leaving it stuck. Same fix on Scout and on a local server.

### Windows: Download no longer breaks the next launch

**Download** used to run the installer immediately, which could not
replace a running `k2-daemon.exe`, skipped **Install & Relaunch**, and
left the app Connecting forever. Download now only downloads. Install
stops the bundled daemon, replaces the files, then relaunches K2.

### What to try

1. **Scout or local:** open Projects with several terminals visible —
   they should paint, not stay red. Click a pinned chat that was red;
   it should recover without a full reload.
2. **Windows (0.40.95/96):** Check for Updates → Download → Install &
   Relaunch. K2 should come back on 0.40.97, not sit on Connecting.

## 0.40.96 — Windows agents, host-session re-wake, compose hotkeys

Three dogfood fixes: Windows agent launch + tunnel install polish, Scout
host-session **re-wake after final**, and macOS/global shortcuts while the
agent message box is focused.

### Windows: agents actually start + no mystery console

- **Agent / preset / pinned-tab launch** finds bare CLI names (`grok`,
  `claude`, …) again — PATH enrichment was Unix-only; Windows now merges
  User+Machine Path, known install dirs, and correct `;` separators.
- **`frpc` ships with the app** (next to `k2.exe`) and is staged under
  `%USERPROFILE%\.k2\bin` on launch — no separate tunnel client install for
  K2 Connect. Resolve also finds `frpc.exe` and the install directory.
- **No black console window** when the app starts the local daemon (daemon
  is a Windows GUI subsystem build; frpc spawn uses `CREATE_NO_WINDOW`).
- Release pipeline: Windows NSIS + **`windows-x86_64` in `latest.json`** so
  Check for Updates works on Windows (missed on 0.40.95).

### Host-sessions / Scout: keep the handle after “done”

After `k2 respond --final` / `k2 done`, Grace still **stops the PTY** (spend
stops) but **keeps** the durable `api-%` index row when a `sessionId` is set.
Answer-driven **dead-resume / re-wake** minutes or hours later no longer
hard-404s solely because the grace reaper wiped the index. Provider JSONL
was always on disk — this is index policy only. Workspace authz and live
cell caps are unchanged. Explicit not-live `/kill` can still clear the row.

### Shortcuts work with the agent compose box focused

**Cmd+Shift+T** (launch default agent), **Cmd+T**, **Cmd+W**, and other app
chords no longer die while the “message the agent” textarea is focused.
Typing keys still stay local to the box; Cmd/Ctrl chords bubble to the
global shortcut owner.

### Housekeeping

- Stop writing `MIGRATION-0.37.0.md` into workspace `.k2` / `.k2so` (GH#58
  untracked-receipt class). Sentinel-only.

### What to try

1. **Windows:** install → launch-bar / pinned agent → bare-name agents open.
2. **Windows:** start K2 → no bare console on the desktop; Connect finds frpc.
3. **macOS:** focus agent compose → **Cmd+Shift+T** still launches a session.
4. **Scout:** host-session `--final` → wait past grace → resume same `sessionId`.

---

## 0.40.95 — Tickets polish + chat archive + host-session TTL clarity

Follow-on to the Windows desktop ship: **Tickets** get a clearer status
board and assignees, **Chats** get a real user archive, and host-session
integrators get honest capability JWT lifetimes.

### Tickets: “needs discussion” + colors + assignees

The Tickets page (agent→human asks) grows one more open status and a
people filter agents can target from the CLI.

| Status | Color | Meaning |
|--------|--------|---------|
| **Waiting** | **Yellow** | Still needs a first human response |
| **Needs discussion** | **Orange** (the old waiting color) | Open follow-up — talk it through |
| Answered / Planned / Closed | unchanged | Same as before |

- Set **Needs discussion** from the ticket card, thread actions, or resolve API.
- **People filter** next to search: All people / Unassigned / each assignee.
- Agents can **suggest who should handle it at create time**:

```bash
k2 tickets ask "Review the pricing sheet" --assign julie
k2 tickets assign <id> --to owner,julie
k2 tickets list --status needs_discussion
```

(`k2 feedback` remains a compatibility alias.)

### Chats: archive, restore, and age highlight

Long chat lists are easier to manage:

- **Orange age highlight** when a chat is **≥ 20 days** old.
- Context menu **Archive** (Claude) moves the session into a bottom
  **Archive** section — storage under `.k2/session-archive/user/…`.
- **Restore** brings it back to the live provider path so resume works again.
- This is separate from the daily **protective backup copy** (settings
  `session_archive_days`) — backup still does not hide chats.
- **Copy Path** resolves correctly for archived sessions.

### Projects rail + compose polish

- Pin **members** and **resources** (A–Z pin order) in the Projects rail;
  collapsed rail keeps context menu + drag.
- Agent compose: **file drop** routes into the workspace (remote upload to
  `.k2/downloads` when needed); textareas stop collapsing on grow.
- Chat / ticket / compose fonts track **Code Editor Appearance** font size.
- Active drawer scrolls; Files drawer errors sit at the bottom.

### Host-sessions (integrators / Scout)

- Capability JWT lifetime is **`min(timeout_secs, 3600)`** — not a fixed
  300s window when you ask for longer.
- Request body accepts **`timeoutSecs`** (camelCase) as well as
  `timeout_secs`, so longer seats are not silently defaulted to 180.
- Envelope doc updated: `docs/host-session-capability-envelope.md`.

### What to try

1. Tickets → set a card to **Needs discussion** (orange); filter by person.
2. From an agent: `k2 tickets ask "…" --assign owner` and open Tickets.
3. Chats → right-click a Claude chat → **Archive** / **Restore**; spot
   orange dates on old threads.

---

## 0.40.94 — Native Windows app (client + local daemon)

K2 is no longer macOS-only on the desktop. This release ships a real
**Windows install** of the same product model you already know: a thin client
that pairs with a **local K2 daemon** (and the tunnel sidecar), not a
browser-only shell.

### Windows: full install, not just a UI

Download **`K2_*_x64-setup.exe`** from the GitHub Release and install like a
normal app (Start Menu / desktop shortcut).

What lands next to each other in the install folder:

| Binary | Role |
|--------|------|
| **`k2.exe`** | Desktop thin client (webview UI) |
| **`k2-daemon.exe`** | Local daemon — projects, sessions, terminals, APIs |
| **`frpc.exe`** | Tunnel client for K2 Connect–style remote access |

- Launch the app from **Programs / Start Menu** (or Desktop) — that is the
  supported path, not a random build folder.
- If the daemon is not already answering, the client **starts the bundled
  daemon** for you (Windows has no launchd; we spawn it as a peer process).
- Same mental model as Mac: **UI is a viewer; the daemon owns the truth.**

### Windows window chrome (frameless)

Windows builds use a **frameless** window with in-app chrome so the top bar
feels like one piece of software:

- **Menu** button (app actions that macOS puts in the system menu bar)
- Custom **minimize / maximize / close** (dense 24px controls, flush to the
  right edge)
- Drag the empty title-bar region to move the window
- Hover / press states work under **WebView2** (global hover fix so buttons
  actually highlight)

### macOS: deliberately unchanged

macOS keeps the familiar experience:

- System **menu bar** (no Windows-style Menu button)
- Native **traffic lights** + spacer
- Decorated window (not frameless)

Linux desktop builds follow the **Windows-style** frameless chrome (Menu +
window controls), not the macOS traffic-light layout.

### Under the hood (still user-relevant)

- Core + daemon **compile and run on MSVC** — this is a real Windows port of
  the daemon stack, not a stub UI.
- Release pipeline can attach the Windows NSIS installer to the same GitHub
  Release as the Mac DMG and Linux packages (built on the sticky Windows
  build host for now; swappable to cloud later).

### What to try first on Windows

1. Install from the NSIS setup → open from Start Menu.
2. Confirm the app connects (local daemon should come up if needed).
3. Use Menu + window controls; resize / maximize; hover on the top bar.

---

## 0.40.93 — Host-sessions stage `K2_SESSION_ID` (sandbox parity)

### Host-sessions: `K2_SESSION_ID` in host-curated env

Cold spawn and dead-resume host-sessions now set **`K2_SESSION_ID`** to the
same string as the API `sessionId` and capability JWT `sub` — matching the
sandbox route so seats can derive paths without decoding a token.

- Live inject does **not** rewrite process env (same as other env staging).
- Envelope note updated: `docs/host-session-capability-envelope.md`.

---

## 0.40.92 — Host-session status + open capability resource namespaces

Integrator-facing train for Scout-class recovery and write-auth. Unit + live
e2e on the build Mac (unbake, status, kill→same-id dead-resume) green before
cut.

### Host-sessions: `GET …/host-sessions/<sessionId>` status

Read-only reconciler status for an owned host-session (kill-floor authz;
uniform 404 for unowned):

| Field | Meaning |
|-------|---------|
| `live` | PTY child process alive |
| `started` | `live` **or** `latest_seq > 0` **or** provider session file exists |
| `phase` | `working` / `grace` / `finished` / `never_started` (+ optional `gone`) |
| `latest_seq` | Drain high-water (**snake_case**, same as messages) |
| `reaper` | `none` / `working` / `grace` |
| `durable` | Durable `api-%` index row present |

**Product lock:** live + no transcript + `latest_seq == 0` → `phase=working`,
`started=true` — **not** `never_started` (avoids false never-born kills).
No side effects (no kill, remint, or reaper stamp).

PRD: `.k2/prds/prd-v1-host-session-status-v1.md` · addendum
`.k2/prds/prd-caps-recovery-consensus-addendum-v1.md`.

### Capabilities: open `namespace:id` resources (unbake `interview:`)

Capability `resource` is no longer restricted to the `interview:` prefix.

- Grammar: `namespace:id` with lowercase namespace; id is one or two path
  segments (`space:plan/space` OK; multi-slash / `..` / uppercase ns rejected).
- `{resource_id}` in audience templates = **id after the first `:`**
  (e.g. `space:plan1/space2` → `plan1/space2`).
- **`interview:<id>` still works** — no forced Scout v1 migration.
- Open namespaces (no server allowlist); app Layer B still binds `resource`
  to the URL / registry.

Docs: `docs/host-session-capability-envelope.md` (Layer B: `sub` required,
`ws` when present; valid JWT ≠ authorized write).

### Kill → dead-resume recovery runbook (Scout-facing)

Operational SSOT for safe-to-kill gates, same-`sessionId` dead-resume,
ownership re-register, backoff, and deliberate donts:

`docs/host-session-kill-resume-recovery.md`  
(linked from the capability envelope note).

---

## 0.40.91 — Pinned chat pick persists + durable host-session spawn queue

### Pinned chat: history dropdown session sticks across refresh / relaunch

Choosing an older conversation from the pinned-chat history dropdown switched
the UI correctly, then ~5s later deferred session adoption stamped the newest
on-disk session over the pick — so refresh or relaunch reloaded the wrong chat.

- Deferred `newest_on_disk` adoption after ensure no longer runs when a known
  session was successfully resumed (dropdown pick or prior SSOT).
- Adoption itself refuses to overwrite a saved `session_id` that still exists
  on disk (belt-and-suspenders for every spawn site).
- Daemon-owned dropdown switch also stamps the layout offline hint so restore
  matches the pick without waiting on ensure.

### Host-sessions: optional durable spawn queue (default OFF)

When a workspace (or principal / daemon) is at its concurrent host-session
ceiling, excess cold/dead-resume spawns can **enqueue** instead of only
waiting briefly then 429.

- Gate: `K2_HOST_SESSION_SPAWN_QUEUE=1` / `host_session_spawn_queue` (OFF
  until integrators poll `queued` + `jobId`).
- Feature ON: nowait acquire → **202** `{queued, jobId, position}` at cap;
  FIFO per workspace; SQLite durable (migration 0096); drain on every
  quota release.
- Cancel: `POST …/queue/<jobId>/cancel`. Feature OFF keeps legacy S8 wait
  + 429.
- PRD: `.k2/prds/prd-host-session-spawn-queue-v1.md`.

### Host-sessions: reap cleans durable api-% index (no residual tab ghosts)

After `k2 respond --final` / Grace (and null-`session_id` orphans on
ChildExit), durable `workspace_tab_sessions` rows for host-minted `api-%`
cells are cleared by **agent_name** as well as session id. Previously
clear-by-session_id alone left NULL-sid rows forever so GUIs could pile
audit Terminal tabs after the TUI was reaped (Scout residual / smoke on
TestingK2SO). Live-kill still **preserves** a stamped session id for
dead-resume.

Optional smoke/dev: `K2_HOST_SESSION_FINAL_GRACE_SECS` (default 10;
`0` = next reaper tick) and `K2_SANDBOX_REAPER_TICK_SECS`.

### Host-sessions: launch-param security note (0.40.90 follow-up)

Interactive first-turn launch-param puts the **prompt string on process
argv** (`/proc/PID/cmdline` is world-readable for the cell lifetime). That
is the reliability trade for Claude/Codex/Grok-style positionals — true
“never on argv” is not available for those CLIs without reintroducing the
never-born race.

**Integrator rule (unchanged D8):** secrets (write JWTs, API keys) must
**not** ride `prompt`. Use `capabilities[]` → staged **0600 cap file** +
`K2_CAPABILITY_TOKEN` env (envelope `docs/host-session-capability-envelope.md`).

---

## 0.40.90 — Host-session first prompt at launch (never-born) + CLI slug parity

### Host-sessions: initial prompt rides the CLI (no post-spawn paste race)

Cold spawn and dead-resume for API / wiki host-sessions now attach the
first-turn prompt as an **interactive CLI launch parameter** for Claude,
Codex, Grok, Gemini, Cursor Agent, and Pi — so the agent process starts
with the turn instead of a detached paste after settle.

- Same F3 router: live cells still inject; unknown ids still 404; same
  `sessionId` on dead-resume.
- **Fire-once:** prompt is ephemeral exec-only (not stored in
  `args_json` / restart recovery).
- Hermes / unknown agents keep the old post-spawn inject path.
- Live lookup requires a living child (phantom map rows fall through to
  dead-resume).
- **Kill → dead-resume:** live kill keeps the durable session index
  (`resumable: true`) so `POST …/host-sessions` with `{"session": id}` can
  re-launch under the same id; Grace after `--final` still clears the row.
- PRD: `.k2/prds/prd-host-session-launch-param-prompt-v1.md` (Julie / Scout
  never-born residual after 0.40.89 settle).

### `k2 workspace host-session-cell-cap` sees the same workspaces as spawn

`k2 workspace host-session-cell-cap get sales-interview` failed with
"Project not found" while `POST /v1/w/sales-interview/host-sessions` worked.
CLI resolve only matched `projects.name`; the v1 path also matches **folder
basename** when the display name differs.

- `resolve_workspace` now matches unique folder basename (NOCASE), same idea
  as `resolve_workspace_slug` (Scout / Julie 2026-08).
- Cap GET/SET and other CLI verbs using `/cli/workspace/resolve` gain the same
  addressing.

---

## 0.40.89 — Host-session Claude cold-start settle (never-born)

### Concurrent claude host-sessions: less typed-not-sent prompt loss

Under several concurrent Claude host-session spawns, the initial prompt could
log `delivered=true` while no turn started (no transcript, PID idle): inject
landed before the TUI accepted input, then a repaint wiped it.

- Raise Claude **post-spawn settle** from **400ms → 1500ms** so host-spawn
  matches the wake path (`wake_headless`: TUI needs ~1s before clean input).
- Root-cause write-up: `.k2/prds/prd-host-session-initial-prompt-loss-v1.md`
  (Primary: turn-start poll + re-inject still open).

---

## 0.40.88 — Compose bar focus no longer stolen by grid heal

### Typing in the agent message bar stays put

On **0.40.87**, the OPEN no-frame heal (≥20s silent paint →
`forceGridResync('grid-stall-no-frame')`) still does the right recovery
work, but the shadow-input keyboard effect re-ran on the phase flip and
**unconditionally** called `el.focus()`, yanking the cursor out of the
agent compose bar mid-type.

- **Heal kept:** one `grid-stall-no-frame` reattach per stall episode
  (plus k1 ack re-probe, dead-WS poll, daemon liveness pings).
- **Focus fixed:** terminal shadow focus is gated — never steals from
  compose / inputs / textareas / contenteditable on reattach or tab
  visibility churn.

---

## 0.40.87 — Host-session finalize, kill, cap, done lifecycle, grid-stall (Scout)

### UDS `k2 respond --final` actually arms Grace

Host-session agents post over `K2_HOOK_SOCK` (per-cell UDS). That path used to
return `{"ok":true}` without calling `sandbox_reaper::on_respond`, so cells
never entered Grace and accumulated forever. UDS `/cli/respond` now matches
the TCP path: `--final` arms Grace.

### Grace expiry does full teardown

Grace no longer only `sess.kill()` + drop the reaper entry. Expiry uses the
same full teardown chokepoint as `/kill` (map unregister + reaper keys).
Teardown is **not** gated on the API caller consuming the respond drain.

### `/kill` works after daemon restart

`POST /v1/w/<ws>/host-sessions/<id>/kill` no longer depends only on the
in-memory owner map (wiped on restart). After workspace auth:

1. Live owner map or kill tombstone
2. Durable `api-{principal}-…` tab row
3. Daemon **Owner** may sweep any durable `api-*` host session in that workspace

Not-live kills clear the durable index (`indexCleared`) so Scout can reap
list orphans. Note: the 404 body is still often `{"error":"no such workspace"}`
for unknown/unowned (uniform, no existence oracle) — not always a bad slug.

### Per-workspace host-session concurrent cap

Each workspace can set its own concurrent live host-session cap
(`host_session_cell_cap`) instead of only the process-wide env default.

- **Default still 15** (global env: `K2_SANDBOX_WORKSPACE_CELL_CAP`)
- **Max 512** (daemon ceiling)
- Settings → workspace **API** tab, plus CLI:

```
k2 workspace api-host-session-cap get <ws>
k2 workspace api-host-session-cap set <ws> 64
k2 workspace api-host-session-cap set <ws> default
```

Also: `POST /cli/workspace/set` with `fields.host_session_cell_cap`.

Raising the cap is runway; agents still need successful `--final` or (in API
cells) `k2 done` so cells actually reaper.

### `k2 done` lifecycle for `/v1` cells (D6 / `K2_API_CELL`)

**In `/v1` host-session and sandbox cells** (spawn sets `K2_API_CELL=1`):
`k2 done` arms Grace and **reaps that session** (no product drain line
required). Same lifecycle as `k2 respond --final`, without a message for the
API caller.

**In persistent workspace agents** (manager, chat, skills — even if they have
`K2_HOOK_SOCK` + a scoped token under COMPAT-58):
`k2 done` is **unchanged** — legacy checkin / “task complete, stay alive.”
It does **not** tear down the agent session.

Do **not** use sock/token presence to infer which meaning applies; only
`K2_API_CELL` (set by the `/v1` spawn doors) selects the reap path.

**0.40.86 trap:** bare `k2 done` was checkin-only even inside API cells —
cells that never called `--final` never entered Grace. 0.40.87 fixes the API
path via `K2_API_CELL`.

### Grid-stall: rich logs + careful OPEN-zombie heal

Remote terminal paint recovery for “ready pane, no frames” episodes:

- **Rich `[grid-stall]` DevTools payload** once per episode (reason, pane,
  sessionId8, readyState, ageMs, lastAckVersion, ackAgeMs, k1, visible,
  phase, reconnectAttempt, documentHidden) plus `[grid-stall] recovered` when
  frames resume.
- **OPEN + no-frame ≥20s** (had a prior frame) → one
  `forceGridResync('grid-stall-no-frame')` per episode — heals half-open OPEN
  zombies without thrashing short idle. Non-OPEN still uses the faster
  ready-ws-not-open path. Latch clears on any frame; 15s k1 ack re-probe kept.
- **Daemon breadcrumbs:** `[grid-pause]` enter/exit once per episode;
  `[grid-liveness]` close on missed pongs (half-open tunnels).

---

## 0.40.86 — DevTools, soft-resync compose bar, louder prod breadcrumbs

### Settings → General: Enable DevTools (desktop)

Settings → **General** → **General** tab now has a **Developer tools**
toggle (local preference only — not daemon `settings.json`). When on, an
**Open DevTools** button opens Chromium DevTools for the focused window
(works in release builds). Useful when diagnosing remote terminal freezes
from console / network.

### Agent compose bar stays during soft-resync

The agent message bar no longer unmounts when a soft-resync briefly flips
the pane `ready` → `connecting` while the daemon session id is still
known. Drafts and focus are less interrupted mid-reconnect.

### Louder prod console breadcrumbs

Always-on (dev + release) console warnings for remote paint recovery:

- `[grid-resync]` — why a grid-only reattach fired
- `[grid-stall]` — ready pane with no frames / dead WS (log only; no auto-reattach)
- `[soft-resync]` — recovery-connected / events-reopen fan-out
- `[recovery]` — remote recovery kind transitions

---

## 0.40.85 — Remote terminal stuck-ready recovery

### Remote terminals stay usable after grid WebSocket drops

On Connect hosts, a terminal could show **phase ready** but stop painting
and ignore input: `ready` only meant “we once got a snapshot,” while the
grid WebSocket was dead and `sendInput` silently no-oped. Resize sometimes
forced a reattach and “snapped” the pane live.

- Force **grid-only reattach** when the socket is not OPEN (on input, and
  every 2s while visible + ready).
- **Buffer keystrokes** across reattach; flush after the next snapshot.
- Soft-resync (recovery / session-events reopen) still uses the same path.
- **Do not** reattach just because no frame arrived after a key — that
  false-fired reconnect thrash on healthy shells (no-echo, line-edit, idle
  TUI). Console: `[grid-resync] reason=input-dead-ws|ready-ws-not-open`.

Wiki: `.k2/wiki/Bug - Remote Terminal Stuck Ready Dead Grid.md` (local).

---

## 0.40.84 — Terminal drop fix + web download + OOTB Gmail OAuth

### Drag-drop into terminals works again

The multi-window drop fix used `getCurrentWindow().listen`, but Tauri emits
`tauri://drag-*` on the **Webview**. Window-target listeners never fired →
OS file/image drops into terminals did nothing. Now uses
`getCurrentWebview().listen` so only the window under the drop handles
inject/copy (no multi-window fan-out).

### Hosted web: Download files from the Files tree

FileTree **Download** on the hosted SPA streams `fs/read-range` from the
daemon into a browser Blob (or Chromium save picker) — same wire as desktop,
without Tauri `local_download_chunk`. Upload was already web-capable; download
is now OOTB too.

### Pre-packaged Gmail client baked into all release daemons

**Customer proof (DTL):** email-link opened Google with
`client_id=REPLACE_ME.apps.googleusercontent.com` → `invalid_client` 401.
Redirect/PKCE were fine — only the compile-time Gmail client was missing.

Daemon defaults are `option_env!("K2_GMAIL_CLIENT_ID")` /
`K2_GMAIL_CLIENT_SECRET`. Linux GHA never injected those secrets through
0.40.83, so fleet daemons shipped the placeholder. (macOS release already
baked Gmail via `.env`; Microsoft is a separate optional path.)

- **Linux** `daemon-binaries`: inject `K2_GMAIL_CLIENT_ID` + `SECRET` from
  GHA secrets at `cargo build`, then **fail the build** unless the binary
  contains the real client id/secret bytes (not merely “no REPLACE_ME” —
  rustc may leave the dead `None` arm string in the binary).
- **macOS** `release.sh` / `build-app.sh`: same require + bake check;
  `build.rs` reruns when OAuth env changes.
- Repo secrets required: `K2_GMAIL_CLIENT_ID`, `K2_GMAIL_CLIENT_SECRET`.

After this cut, the Gmail auth URL uses the real client id (not `REPLACE_ME`).

---

## 0.40.83 — Connect password reset on Linux / hosted hosts

### Owner-role can reset user passwords over Connect

Settings → Users → reset password used to require the raw **daemon owner
token**. On Linux and K2 Connect hosts the desktop is almost always signed
in as a connect-user **Owner** (seed-users / first owner), so reset always
returned **403** while local macOS (real owner token) worked.

`POST /cli/users/set-password` now accepts the same ownership tier as
`set-role`: on-box owner token **or** Owner-role session. Admins stay barred.

### Forced password-change portal can load policy

While `must_change_password` is set, `GET /cli/users/policy` is allowed so
`https://<sub>.k2.dev/` can show real password requirements during forced
rotation (seed-users on cloud Linux). POST policy remains owner-only.

---

## 0.40.82 — Multi-window, soft-resync, daemon singleton, Linux Gmail OAuth bake-in

### Multi-window: browser, drops, and titles

- **Browser / OAuth webviews** parent to the **invoking window** (not hard-coded
  `main`) so Gmail connect and browser tabs no longer paint on the wrong
  window; create races use reap / wait / adopt.
- **External file drops** are window-scoped — only the window under the cursor
  receives inject/copy (no fan-out across every open K2 window).
- Product chrome uses **K2** (not K2SO) for window titles, app menu, and tray.

### Remote terminals: soft-resync after Connect blips

After a tunnel blip, terminal paint could freeze on a half-open grid for ~30s
while control plane and health poll lagged.

- On **recovery → connected** and **session-events reopen**, force a **grid-only**
  re-attach (PTY stays; fresh snapshot).
- Session-events drops surface reconnecting within ~2s and **immediately**
  re-probe health (no wait for the next 25s tick). Hosted 25s poll budget kept.

### Daemon: one process, no rogue `--version` boot

`k2-daemon --version` / `--help` exit without booting. Exclusive flock on
`~/.k2/daemon.lock` refuses a second concurrent boot (stops dual-resume /
shared-tree chaos).

### Linux fleet: pre-packaged Gmail OAuth at compile time

**Partial / incomplete in this cut.** macOS `release.sh` began requiring
`.env` Gmail keys; the Linux **GHA workflow did not inject secrets** until
0.40.84, so Linux release binaries through 0.40.83 still shipped `REPLACE_ME`.
See **0.40.84**.

---

## 0.40.81 — K2 API Tokens settings + host-session reaper reshape

### Host-session reaper: Working never dies on the wall

Product lock (Scout / Julie / Rosson): persistent-interview cells must outlive
user think-time — a spawn-time spend-cap was the wrong lever.

- **Working** (inject / register / non-final `k2 respond`) → **never** auto-reaped
  for silence and **never** killed when `timeout_secs` elapses from spawn.
- **Grace** after `k2 respond --final` (~10s) → completion reap; new inject
  cancels grace and re-enters Working.
- **Spend control** = integrator **kill** + caps / non-remint (not mid-write wall).
- `timeout_secs` remains a client poll / JWT budget clamp only.

Scout E-1 (`timeout_secs=300`, continuous mid-write past the old hard wall)
survives on this model.

### Settings: K2 API Tokens (global) + workspace API tab

- New top-level **Settings → K2 API Tokens**: list all `k2sk_` keys, mint (one-time
  secret reveal), **disable / enable** (emergency soft kill), and **revoke**
  (permanent).
- Workspace settings (**Workspaces / Agents** → select workspace → **API** tab):
  keys that grant that workspace (or `*`), with the same disable/enable/revoke
  actions and a link to the global page.
- Daemon: `api_keys.disabled_at` + `POST /cli/api-keys/disable|enable`; resolve
  rejects disabled keys. CLI: `k2 api-key disable|enable <id>`.

### Projects dashboard: parallel session attach

Multi-pane Project dashboards batch `lookup-by-agent` with `Promise.all` so
live terminals mount and attach together instead of one-by-one.

---

## 0.40.80 — Host-session tab lands on the right workspace

### API host-session tabs no longer hijack the focused workspace

Scout pilot: `POST /v1/w/sales/host-sessions` could surface an orange audit tab
under whatever workspace the desktop had focused (e.g. **Julie**), even when the
PTY cwd was correctly **sales**. Root cause was renderer
`adoptApiSandboxSession` always appending into the active strip.

- Host/sandbox `SessionAdded` adoption now routes by `event.workspace_path` →
  registered project: active project gets the tab; otherwise park on that
  workspace’s layout/background (no focus steal).
- Ephemeral sandbox cwds (no matching project) still surface on the focused
  strip (audit visibility).
- Daemon: `resolve_workspace_slug` **fails closed** on ambiguous name/basename
  (no silent `LIMIT 1` / first-row guess when duplicate `projects.name` rows).

---

## 0.40.79 — Host-session list + kill API

### List liveness matches real PTY processes

After a daemon restart (or any mid-flight kill), `GET /v1/w/<ws>/host-sessions`
could still report `live: true` for cells with no backing process (scout
0.40.78 finding — S9/list lag). KillMode correctly killed the cgroup; the
list only checked map *presence*, not child liveness.

- `live` requires a map entry **and** a living child (`is_child_alive`, now
  also OS `kill(pid,0)` when ChildExit was missed).
- Reaper + list path **reconcile** dead map entries so phantoms cannot linger.
- Dead-resume `resumed: false` path was already correct; this fixes the list.

### Host-session kill API (integrator spend-cap)

```http
POST /v1/w/<ws>/host-sessions/<sessionId>/kill
Authorization: Bearer k2sk_…
```

Force-stops a live host-session PTY with the same ownership / workspace /
non-oracle rules as message-live. Empty body OK.

- **Live + owned** → `200 {"sessionId","killed":true}` (force map unregister +
  kill; reaper unregisters; quota frees on child-exit — no double-release).
- **Owned but not live** → `200 {"sessionId","killed":false,"reason":"not_live"}`
  (idempotent).
- **Unknown / other principal / ungranted / canonical** → uniform `404`.
- Does **not** delete the cap file or revoke JWTs.

Integrator note: [`docs/host-session-capability-envelope.md`](docs/host-session-capability-envelope.md) §5.2.

---

## 0.40.78 — Soft-reconnect recovery clear + host-session workspace slug

### Remote “Reconnecting…” stuck while terminals still work

After a brief remote blip, the banner could stay on **Reconnecting to …**
and files/chat history could show `host is reconnecting — request skipped`
even though the host was healthy and terminal I/O still worked.

Root cause: soft health-poll recovery set `connectionStatus` back to
connected but never cleared `recovery.kind` to `connected`. The banner and
`cliFetch` gate key on **recovery**, not connectionStatus — so they stayed
blocked until quit+relaunch.

- Soft accept after a debounced drop now sets **`recovery → connected`**.
- Terminal/grid path was never gated; control-plane UI matches it again.

### Host-session `workspace` field is always the slug (F4)

Live-resume responses returned the filesystem path; cold-spawn / dead-resume
returned the URL slug. Normalized to the **slug** on all three paths.

### Host-session response shape (F5) + S5 nits

Session/status fields stay **top-level** on live-resume; `capabilities` is only
mint metadata (`staged` / `env` / `expires_at` / `jtis`) — same home as
cold-spawn. S5 documents: `resource` must be `interview:<id>` (else 400
`capabilities-invalid`); dead-resume keeps a **stable** `sessionId` with
`resumed: false` as the sole re-spawn discriminator.

### Cap 429 after queue wait (F6) + list endpoint (F7)

- After the 30s spawn queue wait, 429 now keeps the **blocking cap code**
  (`workspace-cell-cap` / …) instead of always `spawn-queue-timeout`.
- `GET /v1/w/<ws>/host-sessions` documented in S5; list is **one row per
  sessionId** so historical agent rows don’t all flip `live:true` on respawn.

---

## 0.40.77 — Public `/v1/jwks` (envelope verify fix)

### `/v1/jwks` is unauthenticated

Pilot final-test (0.40.76) found `GET /v1/jwks` returned **401** without an
API key. That broke the ES256 contract: Scout must fetch public keys with
**no** long-lived K2 secret.

- **`GET /v1/jwks` is public** (no Bearer required), same tier as `/boot-status`.
- Served even when the `/v1` spawn surface is dark — keys are public either way.
- Still returns only JWKS public material (no secrets).

Integrator note: [`docs/host-session-capability-envelope.md`](docs/host-session-capability-envelope.md).

---

## 0.40.76 — Host-session capability envelope (testable)

### `/v1` host-sessions — capability envelope + multi-turn reliability

Integrators (e.g. Scout) can request **scoped, short-lived capability JWTs**
on host-session spawn/resume so agents call the app API **without** secrets
in the free-form prompt.

- Optional `capabilities[]` on spawn/resume → ES256 JWTs staged as
  `K2_CAPABILITY_TOKEN` (spawn only) and **`.k2/caps/<sessionId>.json`**
  (multi-turn SSOT; atomic rewrite on remint).
- **`GET /v1/jwks`** for local verify (`iss=k2-host-sessions`, `alg=ES256`,
  `aud` / `resource` / `actions` / `sub=sessionId`).
- Live resume with `capabilities[]` **re-mints** to the cap file (not process
  env). Prior jtis stay valid until `exp` (app-local revoke if needed).
- **`resumed: true|false`** on session-addressed responses (live vs dead re-spawn).
- **Work-completion reaper:** Working agents are not idle-reaped mid-job;
  grace after `k2 respond --final`; hard wall = `timeout_secs`.
- Concurrent defaults: principal **64** / workspace **15** / global **512**,
  with wait-then-structured 429 (`concurrent-cell-cap`, `workspace-cell-cap`,
  `cell-capacity`, `spawn-queue-timeout`).

Integrator guide: [`docs/host-session-capability-envelope.md`](docs/host-session-capability-envelope.md).

Signing key for the pilot is **static** at `~/.k2/capability-signing.pem`
(rotation runbook later). Capability **jti** revoke remains app-local.

---

## 0.40.75 — Hosted web: fewer edge health polls

### Hosted web / K2 Connect remote clients

Open tabs against `*.app.k2.dev` (and other remote hosts) no longer hammer
the edge with a dual `/boot-status` + `/cli/auth/whoami` probe every **4
seconds**.

- Steady-state health poll is **~25s** (was 4s), with **jitter** so many
  tabs don’t lockstep.
- **Hidden tabs pause** health polling; one probe fires when the tab is
  visible again. Session WebSockets still detect real drops while the tab
  is open.
- Mid-session **whoami** every health tick is removed; first-connect still
  validates the session before mount. Token death is handled via real
  `/cli/*` 401/403 and session-events WS recovery.

Cuts request volume on the Cloudflare Worker proxy lane dramatically
(order-of-magnitude for always-open viewer tabs) without slowing first
connect or recovery backoff.

---

## 0.40.74 — Cross-server tray file send (Connect token fix)

### Inter-server agent file packages (`k2 msg --inbox-*`)

Tray file send to another server (`k2 msg agent::host --inbox-silent|wake
<path>`) no longer fails when the host is already in Servers and you’re
signed in — if the desktop has a remembered Connect session for that host.

- **CLI** resolves the destination token from `connect-tokens.json` (flexible
  hostname match) **and** the OS keychain (same store as desktop Remember),
  via the host id in `connect-hosts.json`.
- **Desktop** mirrors remembered session tokens into
  `~/.k2/connect-tokens.json` (0600) on sign-in and on boot hydrate so agents
  and CLI can upload without hand-editing that file.
- **Errors** state the real missing precondition (no session token for that
  host) instead of a misleading “add / sign-in / ServerSwitcher” list when
  those were already true. Live text over federation is unchanged and still
  does **not** carry file bytes.
- **Folders** are not supported — send a file or a `.zip`.

Fixes the class of failure reported in GH #60 (Baden: federated file send
refused despite registered host + account + trust + tunnel).

### Upgrade notes

- **Desktop app** 0.40.74+ (CLI is bundled with the app) for the fix.
- After upgrade, open the app once (or re-sign-in to peer hosts with Remember)
  so tokens mirror for CLI; then retry tray send from an agent.

---

## 0.40.73 — Hosted web file drop

### Hosted web (k2.dev / browser SPA)

- **Drag-and-drop files** from your machine into the hosted client now uploads
  to the connected host (same `fs/upload-binary` / chunked path as the desktop
  remote-drop flow — Drive-style File API, no Tauri).
- Drop onto a **folder** in Files → lands in that folder.
- Drop onto a **terminal** → uploads into the workspace’s **`.k2/downloads/`**
  (created if missing) and **injects the host path** into the session so the
  agent can open it — same product behavior as remote desktop drops.
- Drop elsewhere → “Save to…” picker when the host needs a destination.
- Works over plain **HTTP** LAN/dev origins (non-secure context): toasts,
  transfers, and tab restore no longer crash with
  `crypto.randomUUID is not a function`.

### Desktop app

- Unchanged drop behavior. This release is additive for the hosted web client
  (and the UUID helper is harmless on secure/desktop contexts).

### Upgrade notes

- **Hosted web only** needs this client build for browser drag-upload.
- Daemon already supports the upload routes; no special daemon version gate
  beyond a current host with `fs/upload-binary` (all post-remote-files hosts).

---

## 0.40.72 — Remote Gmail link + browser overlays

### Email Link — Gmail on a remote / headless daemon

- **Connect Gmail** no longer depends on a browser opening on the **server**.
  When you're signed into a remote host, K2 binds loopback on **your Mac**,
  opens Google consent **inside Settings** (embedded browser), and relays the
  auth code to the daemon for the token exchange (PKCE stays on the daemon).
- The consent **URL is always shown** (copy + open outside K2) if you need it.
- Headless Linux hosts no longer fake “a browser opened” when there is no
  `DISPLAY` / Wayland — they teach client-capture instead of hanging.

### Embedded browser

- Browser tabs no longer float **on top of Settings** (or Projects / Feedback /
  Wiki). Opening Settings hides the native webview until you leave.
- Settings embeds (Gmail consent) correctly create the webview once the panel
  has a real size — no more address bar URL with “Enter a URL…” forever.

### Upgrade notes

- **Desktop app + daemon** both need 0.40.72 for remote Gmail link (client
  capture APIs + UI). Older apps against a new daemon get a clear headless
  error; new apps against an old daemon won't complete the relay.

---

## 0.40.71 — Remote TUI scroll attach + tunnel E2E self-heal

### Terminal — smooth remote TUI scroll (attach-size lifecycle)

Fullscreen Claude/Grok (and other mouse-mode TUIs) over **K2 Connect** no
longer go molasses after a cold open or tab return when the session first
attached at the wrong size.

- **Client** measures the pane and spawns the PTY at real cols×rows (no more
  happy-path **120×40** toy spawn that forced a full reflow).
- **Daemon** pre-snaps reuse/attach to the last claimer size before the first
  grid snapshot, and applies attach-critical resizes **synchronously** (no
  120ms debounce gap that left the first frame wrong).
- Short **attach/resize settle fence** stops a reflow from tripping k1 pause →
  fat full-snapshot resync under long-haul RTT.
- **Upgrade both** the desktop app (or hosted web client) **and** the remote
  daemon for the full win. Daemon-only still softens recovery; measure-first
  needs the client.
- Experimental **Remote Pace** remains available as an opt-in Terminal toggle
  (default **OFF**) for future latency experiments — leave it off for the
  smooth path validated in this release.

### Tunnel — silent “daemon active, subdomain dark” after frpc drops

- If the **E2E TLS listener** dies while the daemon process stays up (or frpc
  exits in a network cascade), K2 now **detects the dead loopback port,
  re-binds the listener, and rewrites frpc** to the new live port — without a
  full daemon restart (agent PTYs survive).
- Closes the gap left after 0.40.61’s “reuse frozen localPort” fix: a
  once-recorded dead port no longer blocks self-heal forever.
- **Hosted operators:** upgrade the Linux daemon on any box that has shown
  external `HTTP 000` / connection refused while `systemctl` still said
  `active` (nsi / acv / luzz class). After update, verify with
  `curl https://<sub>.k2.dev/boot-status`, not only process liveness.

---

## 0.40.70 — Browser pane reopen + API host-session defaults

### Embedded browser

- Re-opening an **embedded browser** tab no longer fails with
  `add_child failed: a webview with label browser-… already exists` after a
  desync (reload / missed close). K2 now closes any leftover Tauri webview
  for that tab before creating a new one — no restart required.

### `/v1` host sessions (integrators)

- **`api_skip_permissions` defaults ON** so API-spawned host sessions keep
  auto-approve flags and do not stall on headless HITL permission prompts.
  Opt out per workspace when you want fail-closed stripping:
  `k2 workspace api-skip-permissions set <ws> off`
- New CLI: `k2 workspace api-skip-permissions get|set <ws> on|off`

### Agents / settings

- **Agent display name** is back on the workspace Agent tab (under icon and
  color). Renaming updates the technical name too so federated
  `name::host` stamps stay in sync; names must be unique on the server.
- Top-bar **server switcher** lists saved servers **A–Z** by label (Local
  stays first).

### Upgrade notes

- Existing workspaces with `api_skip_permissions` unset become ON (migration
  0093). Explicit off remains off.
- Client app update is required for the browser-pane fix (daemon-only
  restarts do not clear Tauri webview labels).

---

## 0.40.69 — Composer resize + web paste

### Tabs / terminal

- **Message the agent** box grows and shrinks with your text again, without
  staying too tall or clipping the placeholder when you clear the draft.
- Resting size is back to a single line.

### Hosted web

- **Ctrl+V / Cmd+V** paste works in the terminal (Ctrl+V no longer gets
  swallowed as a control character).

---

## 0.40.68 — Connect reconnection flap fix

### K2 Connect

- After a brief drop while signed into a hosted server, the app no longer
  loops forever on **Reconnecting…** when the server itself is healthy.
  The client now detects a thrashing webview connection path (including
  network / TLS handshake failures, not only HTTP 404s), cold-rebuilds
  once, and if needed asks you to **Restart K2** instead of flapping.
- Session event sockets close cleanly before redialing, which stops
  attach/detach thrash against a healthy tunnel.

---

## 0.40.67 — Connect stability + host-scoped Servers

### K2 Connect

- **Servers** tab is host-scoped: on **This Mac** you manage your saved
  servers; when signed into a remote you see **that host’s federation
  peers** (pair new ones via “Pair from this Mac”). The top-bar switcher
  still lists **your** servers so you can always return to Local.
- Opening **Access** no longer pokes the active remote’s tunnel APIs
  (tunnel config stays on **this Mac**), which was thrashing reconnects
  and emptying the workspace list after Settings.
- Safer remote list handling so a bad or empty daemon response can’t
  black-screen the app with a spread/iterable error.

### Files

- Directory listings tolerate unexpected `fs/read-dir` shapes instead of
  failing the Files panel.

---

## 0.40.66 — Endgame Stage A: agent type dual-read + copy terminal id

### Tabs

- **Copy Terminal ID** on tab right-click works in **GA** for every
  terminal session tab — not only fresh CLI-agent launches. Uses the
  daemon session id when live (what `k2 terminal write` expects), so
  agents can target that PTY directly.

### Endgame Stage A — agent type

- Readers treat **`k2` and legacy `k2so` as the same builtin agent type**
  (daemon, skill/wake paths, and UI). Writers still store `k2so` until a
  later stage migrates values.
- Single helpers (`is_builtin_agent_type` / `isBuiltinAgentType`) own every
  comparison so a future value migration cannot strand old or new rows.

### Upgrade notes

- No data rewrite this release. Fresh and upgraded installs keep existing
  `agent_mode` / frontmatter spellings; both spellings just work.

---

## 0.40.65 — Polish + leave the k2so name behind

### UI polish

- **Finished-agent toast** and Active-bar orange dots attribute the
  workspace that **actually** finished — via the daemon lifecycle
  broadcast (`workspacePath` on `agent_status_changed`), so remote hosts
  stay correct. **View** switches to that workspace.
- Heartbeats drawer always loads the roster from the **daemon** (local
  too), so a stray empty `k2.db` can't report "Project not found" while
  the sidebar still shows the workspace.
- **Tab reorder** drop indicator lines up with the real slot (no left
  offset from pinned Inbox/Chat tabs).
- **View Wiki** is on the workspace right-click menu (and worktree rows).
- **Copy Path** on workspace (and worktree) right-click — clipboard the
  folder path without opening Finder.
- Removed unused **New Section…** worktree-grouping (sidebar + focus
  window). Worktrees list flat again.
- Workspace **Rename** is marked **coming soon** (disabled) — display
  names without folder renames are still to be designed.
- **Projects tab** auto-selects the first project when you open it with
  nothing selected.

### Canonical home paths

- CI’s **k2-home-gate** (was `k2so-gate`) blocks new hardcoded `~/.k2so`
  path literals. Allowlist is only deliberate compat tests and migration.
- Dev helpers (`web-serve`, web client smoke) resolve the daemon port via
  **`~/.k2` only** — the compat symlink still covers upgraded machines.

### Endgame Stage A — database filename

- The daemon **opens `k2.db` when it already exists**, otherwise
  `k2so.db` (unchanged create path for fresh installs this release).
- Prepares the rename lane without rewriting live data yet
  (writer flip is a later endgame stage).

### Repo hygiene

- Developer changelogs live under `docs/changelog/` (not the repo root).

---

## 0.40.64 — Context management stack: always-on AGENTS.md

Always-on workspace context is a **stack of markdown layers** that K2
composes into `.k2/AGENTS.md` — the **context management stack**.

### Stack model

- **System layers** (toggleable): Agent persona, PROJECT.md, Tooling
  (k2-cli pointer).
- **Optional layers**: ordered path references (wiki index, your docs,
  guidance packs). Enable / disable / reorder without editing the
  generated file.
- **SSOT**: daemon SQLite; CLI and Settings stay in sync.

### CLI

```text
k2 agent context list|add|remove|on|off|move|show|regen|catalog
k2 agent context on|off pinned:agent|pinned:project|pinned:tooling
k2 agent hire <dir> --context wiki:index --context docs/notes.md
```

Catalog ids include `wiki:index`, `wiki:home`, **`wiki:hygiene`**,
`subagents:pack`, `manager:pack`, `k2:pack` (lean packs under
`.k2/context/catalog/`), plus live rosters that rewrite with AGENTS.md:
**`connections:roster`**, **`heartbeats:roster`**, **`skills:roster`**.

### Settings

Workspace Settings has a **Context management stack** editor (View/Edit,
system toggles, **Browse catalog**). Day-2 management is
`k2 agent context …`; hire only **seeds** layers with `--context`.

### Soft size warn

Stacks over ~64 KiB show a soft warning so always-on context stays lean.
Load skills for depth; keep stack layers short.

---

## 0.40.63 — `k2 msg` tray packages: silent, wake, and files

### Send durable packages, not only live pings

Live `k2 msg <workspace> "text"` is still for **short** lines injected into
a running session. For briefs, PDFs, CSVs, and multi-file drops, use the
**work tray** modes:

```text
k2 msg <workspace> --inbox-wake   ./brief.md
k2 msg <workspace> --inbox-silent ./report.pdf
k2 msg <workspace> --inbox-wake   a.md b.pdf notes.txt --title "Batch"
```

- **`--inbox-wake`** (preferred) — lands a package under the peer’s
  `.k2/inbox/` **and** knocks with a short live line:
  `[inbox:<id>] <title>` plus `Open: k2 inbox read <id>` (never the full
  file body).
- **`--inbox-silent`** — package only; **does not** notify. Use wake, or a
  separate live `msg`, if you want a knock.
- **Bare `--inbox`** is a hard error — you must pick silent or wake.

Markdown is normalized into a tray item; other files become a **cover note**
plus sidecars under `.k2/inbox/<id>.files/`. Multiple paths → **one** package.

Recipients still manage their tray with **`k2 inbox list|read|move|…`**.
This is **not** `k2 mail` (real email).

### Cross-host files (Connect)

Large or remote drops stage with the same upload path Clone To uses
(single-shot under 50 MB, chunked at/above). Tray send to `agent::host`
needs a **signed-in K2 Connect host** for that server — federation live
text is not enough to upload files. In-cell agent passports can deliver
when the daemon can read the path on the same machine; remote **staging**
needs an owner or Connect-user token (clear error if not).

### Agent docs

Built-in k2-cli skill strings and wake templates teach
`--inbox-wake` / `--inbox-silent` with file paths, not the old bare
`--inbox --title/--body` form.

---

## 0.40.62 — Settings that scale: Connect, LLMs, General

### K2 Connect is easier to navigate

Settings → **K2 Connect** is no longer a permanent side-by-side Host /
Servers wall. You get clear tabs:

- **Tunnel** — expose this machine + host policies  
- **Access** — users and invite (half-width again, so forms aren’t stretched)  
- **Servers** — your address book, updates, and federation pairing  

### Pair two cloud servers while you’re signed into one

The **saved-servers** list (this Mac’s address book) stays visible when
you’re connected to a remote. That unlocks **Pair as federated peer**
between two K2 Cloud boxes without bouncing back to Local first. The
top-bar server switcher still always shows *your* list — not the remote
machine’s.

### Agentic systems are just on

No more **Agentic Systems** beta toggle. Canonical Agent Flow, Heartbeats,
workspace Skills, and agent polling are always available. Older
“turn agentic off” flags no longer hide the product.

### Editors and LLMs are separate

- **Editors** — default editor/terminal and detected apps  
- **LLMs** — default agent, presets, CLI install guide, and a **Credentials**
  column for the big seven (Claude live auto-refresh; Codex, Grok, Gemini,
  Cursor Agent, Hermes, Pi coming soon)  

Nav: **LLMs** sits under Styles. Workspace settings are labeled
**Workspaces / Agents**.

### General has real tabs

Settings → **General**:

| Tab | What’s there |
|---|---|
| **General** | Version, CLI, What’s new, your name, reset |
| **Workspaces** | Active-bar hours, completion sound, **Canonical Agent Flow** help |
| **Server** | K2 Server + keep running when the window closes |
| **Local LLM** `beta` | Workspace assistant / model |

Canonical Agent Flow is no longer its own top-level page — it lives under
General → Workspaces, with room for the diagram and a shorter intro.

### Agent presets & CLI guide

Built-in order puts **Grok before Gemini**. CLI Tools Setup matches that
list and includes **Hermes**. Existing installs get the Grok/Gemini swap
on the next daemon start (or use **Reset Built-ins**).

---

## 0.40.61 — Linux servers actually update (and stay reachable)

### Update from the app installs the new daemon

On a **remote Linux** host (Settings → **Connections** → **Update to …**),
K2 now finishes the full install path: download → verify → **install &
restart**. Before this release, that button could download and stage a
build without ever swapping the binary, so the box looked like it
“updated,” came back on the **same** version, and left you thinking
nothing changed.

After install, the app only treats the update as successful when the
host’s `/boot-status` reports the **expected new version** — not merely
“online again.”

(Settings → General → remote host **Download** / **Install & restart**
was already the complete flow; Connections is now aligned.)

### Tunnel stays in sync after restart

Restarting a Linux daemon (including for an update) could leave the
public `*.k2.dev` tunnel pointing at a **dead local port** while the
daemon listened elsewhere — external **HTTP 000** until an operator
SSHed in and ran `systemctl restart`.

0.40.61 keeps the live HTTPS listener port as the source of truth for
frpc, hardens tunnel stop/reap so orphan frpc is less likely, and
self-heals when frpc’s local port is unreachable but a live listener
exists (rewrite frpc only — agent sessions are not killed for that
heal).

**Ops note for already-provisioned boxes:** new install scripts use
`KillMode=control-group` so orphan tunnel helpers die with the unit.
Existing units may still say `KillMode=process` until you redeploy the
unit or:

```bash
sudo sed -i 's/KillMode=process/KillMode=control-group/' /etc/systemd/system/k2-daemon.service
sudo systemctl daemon-reload && sudo systemctl restart k2-daemon
```

Update both the **thin client** and the **daemon** on the host so the
new Connections flow and the tunnel self-heal land together.

---

## 0.40.60 — Tickets, and chat text you can actually select

### Feedback is now Tickets

The Feedback surface is **Tickets** end to end — page label, agent CLI
(`k2 tickets …`), and wording. Existing ticket data and routes keep
working; this is a product rename, not a wipe.

### Ticket polish

- **Planned** status alongside waiting / answered / closed
- **Assignees** so a ticket can target the right people
- **Drafts** that survive leaving a ticket and coming back
- Reply box **auto-grows**, autofocuses when a thread is ready, and
  resizes with the detail panel
- **Clickable links** in ticket and project chat bodies

### Highlight and copy chat text

You can **drag-select and copy** message bodies in **Tickets** threads
and **Project chat** — not only the composer.

A global “return focus to the terminal” path was treating those
message divs as dead space and clearing the highlight almost instantly.
That reclaim now leaves a live selection alone (and skips full-page
Tickets / Projects / Wiki overlays).

---

## 0.40.59 — Files tree no longer bounces on busy hosts

On remote (and busy local) machines, the Files drawer could flash
**Loading...** over and over, making the tree jump while agents or other
tools wrote under the workspace.

Live refresh still updates when real project files change. High-churn
paths (agent/runtime state, `.git`, build caches, logs) no longer thrash
the list, and a directory you already have open refreshes quietly in the
background instead of re-showing the loading row.

Update both the **thin client** and the **daemon** on the host (e.g. NSI)
so the quieter watcher and the calmer UI land together.

---

## 0.40.58 — Files that keep up, and previews that actually work

The **Files** drawer is a real workspace browser again — not a list that
lags behind agents or only shows plain text when you open something.

### Live refresh when others change the tree

When an agent, another client, or a process on the host adds, renames, or
deletes files, the Files tree updates without a full window reload. That
covers local workspaces and remote hosts the same way.

### Dropping files no longer multiplies copies

Dragging from Finder (or another app) into Files used to sometimes create
several copies of the same drop — especially with multiple panes open.
Drops are routed once and uploads single-flight, so one drag means one
landing.

### Open more than code

Click a file and K2 picks a preview when it can:

- **Images** — PNG, JPEG, GIF, WebP, SVG, ICO (and friends), including
  files on remote hosts
- **CSV / TSV** — table view you can scroll and edit
- **Zip** — list contents, with a clear path to **Extract** on the host
- **Audio / video** — play on **your** machine (the thin client), not the
  remote server’s speakers; bytes stream from the host daemon
- **Diagrams** — Mermaid (`.mmd` / `.mermaid`) rendered in the viewer
- **Everything else binary** — a clear empty state instead of a broken
  text tab

Spreadsheets and slide decks (xlsx / pptx) are still out of scope for this
release.

### Polished tree

Folders sort first, icons follow light/dark (Seti-style), and **Reveal in
Finder** / the OS equivalent is labeled correctly.

---

## 0.40.57 — Live agent terminals stay alive across brief disconnects

The **Active** reaper no longer kills a live agent terminal just because the
app or control plane briefly disconnects. If the PTY is still running, the
session is left alone — only truly idle/orphaned sessions are aged out.

**Explicit dismiss still closes chats** the way you expect. Closing or
dismissing a session yourself is unchanged.

Daemon stderr is quieter too: terminal poll performance histogram lines no
longer spam the log during normal use.

---

## 0.40.56 — Agent passports no longer die overnight

Long-running agents were losing the ability to use the **K2 CLI**
(`k2 msg`, inbox, peers, and other in-cell tools) after about a day, with
errors that looked like an **expired auth token** — even though the agent
session itself was still alive.

### What was wrong

Each agent cell gets a **scoped passport** (`K2_HOOK_TOKEN`) at spawn so it
can call the daemon without holding the full owner secret. That passport
had a hard **24-hour wall-clock expiry**, and there was no way to refresh
it while the process was running. After 24 hours the daemon rejected the
token; messaging and other CLI verbs failed until you restarted the cell.

### What we fixed

Passports now last for the **life of the agent cell**. They still stop
when the session is torn down (or on a global revoke) — that is intentional
security. They no longer suddenly expire just because the clock advanced.

If an agent already hit the old expiry, **restart that session once** after
updating so it mints a fresh passport under the new rules.

---

## 0.40.55 — Clone To: full chat history + your pin comes with you

**Clone To** is a real workspace migration again — not “files only, then
figure out `/resume`.”

### Chats show up after clone (no awkward first resume)

Past conversations appear in Chat History and the agent chat dropdown as
soon as the workspace lands on the destination. You no longer need a
mystery `/resume` just to make the list wake up.

### Pinned Chat stays pinned to the same conversation

If the workspace had a **Pinned Chat** tied to a specific session, that
session id travels with the clone (when the transcript is present). Stars
and custom chat names for those sessions come along too.

### More than Claude — the agents you actually use

Clone To can now carry session history for the major harnesses K2 already
knows how to resume:

- Claude Code (as before, plus the list fix above)
- Cursor Agent chats
- Gemini CLI
- Pi
- Codex
- Grok
- Hermes (workspace rows only — never a whole account database)

Paths and project slugs are rewritten for the destination machine so
resume keeps working when the home path changes (Mac → Linux, etc.).

### What we deliberately do *not* copy

**Provider logins and subscriptions stay put.** The destination server is
expected to use **its own** Claude / Grok / Codex / … accounts. Clone To
moves **workspace + chat history**, not credentials, auth tokens, or
Keychain blobs. Sign in on the other box if you haven’t already.

Also unchanged: tunnel identity, connect-users, and machine-local K2
state stay on each host.

---

## 0.40.54 — Tunnel resilience + hosted web (beta)

Two Connect upgrades: tunnels that actually stop, and a **beta** browser
client for your machine at `https://<you>.app.k2.dev`.

### Hosted web client — beta, available now

Open your K2 server in a normal browser while the desktop app (or headless
daemon) is online with Connect running:

- **URL:** `https://<your-subdomain>.app.k2.dev` (example: `https://z3thon.app.k2.dev`).
- **Same workspace UI** you know from the app — sign in with your K2 Connect
  user, sessions and terminals over the tunnel.
- **Owner wall:** hosted web can be turned off with the daemon
  `web_client_enabled` / owner settings if you do not want browser access.
- **Beta means:** real and usable for day-to-day poking and remote access;
  expect rough edges, rapid fixes, and desktop remaining the primary client.
- **Security note:** the browser path is standard secure-web (TLS at the
  edge, cookie session on your daemon). Desktop Connect to
  `<you>.k2.dev` stays the true end-to-end tunnel path.

Bookmark your `.app.k2.dev` URL once the tunnel is up. If the machine is
asleep or the tunnel is down, the page will say so until it comes back.

### Tunnel resilience (Stop means stopped)

Connect tunnels that used to leave a live `frpc` behind after **Stop**,
daemon restart, or self-update — and that could desync from the live
listener on older builds — now tear down cleanly and keep serving after a
transient frpc drop without a full daemon restart.

#### Stop actually kills the tunnel

- **Stop / disable / SIGTERM** always reaps the supervised `frpc` for this
  daemon's config, including the default single-relay (solo) path.
- No more "UI says stopped, subdomain still registered" from an orphaned
  local frpc after update or restart.
- Tunnel stop status is taken from the real connector state after the kill.

#### Self-heal on frpc drop (no agent kill)

- When frpc exits and the daemon stays up, reconnect reuses the **live**
  E2E listener port — it does not invent a new `localPort` while sockets
  are still bound.
- Full daemon restart still produces consistent ports (the path that always
  worked). Prefer this build on any box that saw silent external outages
  while `systemctl` still showed active (fleet boxes on pre-fix builds
  should upgrade).

#### Identity for multi-device tunnels

- frpc login metadata now carries **`device_id`** when the daemon has one,
  so the relay can tell same-token machines apart (groundwork for
  cross-machine eviction on the control plane).

#### Connect tunnel picker — apex only

- After signing in to your k2.dev account, the subdomain list for **Start
  tunnel** only shows purchased **apex** names (`you.k2.dev`). Nested
  routes (`api.you.k2.dev`, `staging.you.k2.dev`, …) no longer appear as
  tunnel targets — those are routing under an apex tunnel, not separate
  tunnels.

Upgrade any Connect server that flapped after tunnel restart or left
orphan frpc after `systemctl stop` / self-update — and try the beta web
client at `https://<you>.app.k2.dev` once the tunnel is up.

## 0.40.53 — Remote Session (safer-than-SSH help over Connect)

Consent-gated, time-boxed shell drive on a K2 device — for migrations and
remote help **without** handing out root SSH or permanent keys.

### Turn it on (owner)

- **Default OFF.** Nothing remote can open a shell until you enable it.
- **`k2 remote-session enable` / `disable`.** Master wall; disable kills every
  remote shell immediately.
- **`k2 remote-session grant --ttl 45m --label "…"`.** Mints a one-time
  `k2rs_…` drive token (shown once). Revoke with
  `k2 remote-session revoke <id>`.

### Drive (helper / agent)

- **`k2 remote-session shell --token k2rs_…`** opens a daemon-user login
  shell (never root). Works locally or via Connect with `--host`.
- **`write` / `read`** drive that session. Wrong token, expired grant, or
  wall OFF → clear teaching errors (not a silent failure).
- **Audited denials.** Attempts while OFF (or without a grant) show on
  `k2 remote-session status` so you always know someone tried.

### What this is for

- Laptop → server growth / soul-transplant work as the K2 user, without
  root SSH. Full automated runbooks come later; this release is the safe
  hands + grant layer so real migrations can teach the runbook.

See `docs/remote-session.md` for the full cookbook.

## 0.40.52 — Headless Connect CLI + wiki session revive

Provision a Linux box from the shell, and public-wiki chat sessions recover
cleanly after idle reaping.

### Headless K2 Connect onboarding

- **`k2 users add`.** Create the first owner (or more Connect users) on a
  headless daemon: hidden password prompt or `--password-stdin`, then
  `--role owner|admin|member|viewer`. Uses the daemon owner token on the
  box (external setup works without a session passport); in-cell agents
  stay on their scoped passport and cannot elevate.

- **`k2 connect login`.** Pair a purchased k2.dev subdomain from the CLI:
  account email/password (or `--token` access JWT) → pick a subdomain →
  write tunnel config → start tunnel → print the live URL. Session stored
  in `~/.k2/connect-account.json` (0600); password is never written.
  `k2 connect status` / `k2 connect logout` for check and re-auth.
  Manual `K2_TUNNEL_TOKEN` remains the automation fallback.

### Public wiki session lifecycle

- **Close audit tabs when reaped.** Idle host-session PTYs no longer leave
  a dead terminal open in the app.

- **Resume opens a new audit tab.** After reaper kill, waking the same
  session (stored session id) surfaces a fresh tab so the inject is visible
  again.

- **Cold inject path aligned with `k2 msg --wake`.** Host-session first
  message uses settle + readiness + screen quiescence so Grok (and peers)
  don’t lose the paste during first paint.

## 0.40.51 — Public wiki chat + agents control heartbeats

Wiki visitors can talk to a workspace agent in the site itself, and agents
can manage their own heartbeat schedules without fake “invalid token”
errors.

### Public wiki chat

- **Ask this wiki.** Workspaces can turn on public chat for a published
  wiki (default off). Visitors get a third-column chat panel on the live
  site; messages run through the same host-session API as external keys,
  with the chat key held only on the daemon (never in HTML or browser
  responses).

- **Unattended from the first turn.** Enabling public chat opts the
  workspace into skip-permissions for host sessions so the visitor’s
  first message isn’t stuck on a human approval prompt. First-message
  inject also waits for screen quiescence so the paste isn’t wiped by
  the agent’s startup repaint.

- **Guest policy.** Owner-set guest framing still applies on every turn
  (read-only preference + `k2 respond`), same as other API host sessions.

### Agents own heartbeat schedules

- **`k2 heartbeat` works from agent sessions.** Agents can list, add,
  edit, enable/disable, and fire workspace schedules with their session
  passport (UDS when in-cell, TCP dual-auth otherwise). The daemon stamps
  the caller’s workspace so one agent can’t schedule into another’s
  project.

- **Clear owner-only teaching.** OS tick install, fleet-wide lists, and
  similar owner surfaces no longer say “Invalid or missing auth token”
  when an agent passport is presented — they return `owner_only` with a
  hint to ask the human. Agents stop chasing a “broken token” that was
  really a scope boundary.

## 0.40.50 — CLI stays current on Linux servers

Server updates no longer leave the `k2` command stuck on an old version
while the daemon moves ahead.

### Server updates

- **The `k2` CLI can no longer fall behind the daemon.** On Linux
  servers, the daemon refreshes the `k2` command alongside itself on
  every update. If the system-wide install at `/usr/local/bin/k2` isn't
  writable by the daemon's user (a common setup/migration artifact), the
  daemon now stages the current CLI at `~/.local/bin/k2` instead of
  silently leaving the old version in place. Server provisioning also
  installs the CLI daemon-writable from the start.

## 0.40.49 — Pair as federated peer

Cross-server agents needed one more obvious step: **trust**. Turning on
federation and signing into a server was never enough by itself — the two
daemons still had to pin each other as peers. That step is now a button.

### Federated peer pairing

- **Pair as federated peer.** On Settings → Connections, each signed-in
  server tile has a **Pair as federated peer** button. One click establishes
  mutual trust between this Mac and that server (owner on both sides). When
  it works, the tile shows **Peer: trusted**.

- **No more chicken-and-egg.** Workspace Federated Connections used to ask
  you to pick a federated server before any peer existed — and there was no
  UI path to create the first one. Pair from Connections first; then the
  server shows up in Federated Connections so you can link agents.

- **Clear empty states.** Federated servers and Federated Connections empty
  lists now point you at the Pair button instead of a vague "pair first"
  hint.

## 0.40.48 — Resilient reconnect

Server reboots and updates are now a non-event. When a host you're
connected to restarts, K2 notices, waits politely, and reconnects on its
own — no more infinite "Reconnecting…", no more restarting your app to
recover.

### Reconnect that actually recovers

- **Restarts self-heal.** When a remote host reboots or updates, K2
  detects the fresh server instance and quietly reconnects and resyncs.
  The recovery pill (now square, matching the rest of K2) tells you
  what's happening and gets out of the way when it's done.

- **The stuck-connection escape hatch.** Some macOS network sessions can
  keep reusing a dead connection after a server restart (the "works in
  curl, broken in the app" wedge). K2 now detects this with an
  out-of-app probe, clears it automatically where possible, and — in the
  rare case only a restart cures it — says so plainly with a
  **Restart K2** button instead of spinning forever.

- **No more retry storms.** While a host is recovering, K2 stops hammering
  it: requests fail fast, retries are spread out, and reconnection happens
  the moment the host is genuinely back.

### Remote updates you can trust

- **"Updated and reconnected."** After **Update Host**, the status line no
  longer freezes on "Installing & restarting…" — K2 watches the host come
  back and confirms the version it actually returned with. If an update
  rolled back, it tells you that instead of pretending it worked.

- **Servers release their tunnel cleanly.** Daemons now shut down
  gracefully on restarts and self-updates (including supervisor stops),
  releasing their tunnel registration immediately — so the offline window
  during an update is seconds, not minutes.

### Heartbeats

- **Remote heartbeats show up.** The sidebar Heartbeats panel now shows
  the heartbeats of the server you're connected to — not your local
  machine's. Live/resumable/scheduled states come straight from the host.

- **Settings audits the right machine.** The workspace Settings →
  Heartbeats page (roster, fire history, session picker, delivery
  target) now manages the connected host's heartbeats too — everything
  you see and change lands on the server you're looking at.

- **The fleet Heartbeats page follows your connection.** The system-wide
  Heartbeats page (every workspace's heartbeats + the fires audit log +
  wake-scheduler apply) now shows and manages the host you're connected
  to. Requires the host to also run 0.40.48+ for the cross-workspace
  lists; older hosts show an honest error instead of silently showing
  the wrong machine.

- **Errors surface instead of hiding.** A failed heartbeat load shows the
  actual error instead of an eternal "Loading…".

### Layout

- **Column splits stick on remote hosts.** Splitting the tab area into
  columns now saves immediately and survives connection blips — no more
  splits quietly reverting when you're connected to a server.

### Agent messaging

- **Messages to sleeping agents actually arrive.** Waking a dormant
  agent with `k2 msg`/`k2 talk` used to report success while the message
  silently vanished — the injection raced the resumed session's redraw.
  Delivery now waits for the woken terminal to settle before typing, so
  the message lands every time.

- **No more false "Agent launch failed" popups.** The old launch-failure
  guess fired on healthy agent-to-agent messaging (and could quietly
  spawn duplicate sessions via its auto-retry). It's gone; the daemon
  owns spawn health.

### Active area

- **Active means alive.** A workspace now appears in the Active area
  exactly when it has a live terminal session (or is pinned). An agent
  woken by a message pops back in the moment its session exists — on
  every connected client.

- **Dismiss works again.** Right-click → Dismiss removes the workspace
  from the Active area immediately; its session is put to sleep a few
  seconds later.

## 0.40.47 — Workspace wiki brain map

Your notes become a living map. Open **View Wiki** on a workspace to explore
`[[wikilinks]]` as a force graph, read articles side-by-side, and zoom out to
every brain on the machine.

### Workspace knowledge base

- **View Wiki.** From the workspace panel: a full-page map of
  `.k2/wiki/` Markdown notes. Click a node to read it; wikilinks in the
  article jump to other notes. **Hide article** collapses the reader so
  the graph uses the full width.

- **Search + Articles count.** Filter notes by title, tags, or aliases.
  **Articles** shows how many real notes match the current scope and
  search (not phantom missing links).

- **Global / Local.** Global is the whole workspace brain. Local zooms to
  the neighborhood of the selected note (depth 1–2). Home stays lightly
  blue when not selected so you can always find it.

- **Seed & Serve.** One click creates Home + Index under `.k2/wiki/`.
  Serve a read-only localhost site when you want to share or browse in a
  browser. CLI: `k2 wiki status|index|note|seed|serve`.

### K2 fleet map

- **K2 tab.** See every workspace brain registered on this host
  (`~/.k2/wiki`). Workspace hubs connect into each brain without polluting
  per-workspace notes.

- **Projects | Groups.** Two fleet lenses in their own tab strip:
  - **Projects** — project squares link to member workspace hubs. Filter
    with the same workspace/project dropdown used on Feedback.
  - **Groups** — focus-group squares link to hubs when focus groups are
    on. Filter with a focus-group menu (All / Ungrouped / each group).

- **Membership ≠ wikilinks.** Dashed edges are organizational (project or
  focus group). Solid edges inside a brain are real `[[wikilinks]]`.

### Cleanup

- **State is gone product-wide.** Workspace States settings and related
  surface area are removed so the model stays simpler: workspaces,
  projects, and agents.

## 0.40.46 — Cross-server agents + WebGL terminal you can tune

Two big tracks since 0.40.45: **federated agent messaging that actually
pairs and talks**, and **Kessel WebGL** spacing, weight, scroll, and
recovery after workspace switches.

### Cross-server agents

- **Connect a remote agent in one gesture.** On a workspace’s **Federated
  Connections**, pick a paired server and an agent it exposes. K2
  auto-pairs both daemons (mutual trust, no codes) and records the link
  **both ways**, so either side can message the other. **X** removes that
  agent link (and the reverse when it can). Peer pickers only — no
  free-typed hostnames.

- **`agent::host`, not mail.** Federated addresses use double-colon
  (`cortana::rosson.k2.dev`). Inbound chat shows `[from agent::host]`, so
  agents use **`k2 msg`** instead of **`k2 mail`**. Legacy `agent@host`
  still works on the way in.

- **Install on both machines — the daemon restarts with the app.** A
  same-version AirDrop used to leave the *old* launchd daemon running
  (version matched, binary on disk was new). The app now detects a
  replaced `k2-daemon` and kickstarts it. Open the app once after install
  so federation fixes load.

- **Clearer federation CLI.** `k2 fed peers` lists pinned servers and
  trust. Failed `k2 msg agent::host` names known peers and hints when a
  reply path isn’t paired yet. Error copy says **k2**, not the old k2so
  name.

- **Roster only shows contactable agents.** Remote agent lists respect
  Remote Access / contact permission so you don’t pick agents that won’t
  accept federated messages.

- **Passport dual-auth on send.** Agents can send across servers under
  their scoped credential when the connection and trust gates pass —
  without elevating to disk-owner for every hop.

### Terminal (WebGL painter)

- **Per-style text weight.** Dark styles preset heavier; light styles
  thinner. Override under **Settings → Styles → Terminal text weight
  (WebGL)** — saved **per style and scheme**, live on open tabs. Switching
  styles restores that style’s weight.

- **Line height & character spacing.** Global knobs under
  **Settings → Terminal** (WebGL only): line height (default 1.2× font
  size) and character spacing/tracking. Same values across themes; open
  tabs update live. DOM painter is unchanged.

- **Smoother scroll under pressure.** Prewarm backs off before it can
  force an atlas clear; wheel paint and scrollbar drag use live geometry;
  resync after a big backlog no longer yanks the view when you’re
  scrolled up (content seam-match re-anchoring).

- **WebGL recovers after workspace switches.** Hiding a tab or opening
  Settings used to lose the GL context for good. The painter remounts when
  the surface is shown again instead of permanently falling back to DOM.

- **Richer glyph edges.** Default coverage gamma moves so edges keep a
  little more ink after smoothing.

## 0.40.45 — Safer agent mail, cleaner terminals, smoother painting

Agents get clearer boundaries on mail and messaging — and the terminal
looks and scrolls the way you expect, whether you stick with the classic
painter or try the new WebGL one.

- **Mail that knows who you are.** Agent mail keeps riding the same
  grants and levels you already set, but catalog and send paths are
  tighter: the inboxes list is dual-auth with a real session passport,
  linked/BYO send follows the same agent-send gate as hosted mail, and
  listing every hosted address stays owner-only. Agents can also
  **schedule** outbound mail with `--at` / `--in` and track it in the
  outbox. Owner-only hostmail and access verbs now answer with a clear
  **owner-only** teaching error (exit 3) instead of a misleading
  “invalid token.”

- **Connections before you talk.** Cross-workspace `msg`, `read`, and
  inbox compose from an agent require a local connection first — no more
  silent surprise paths. Creating those connections stays **off by
  default** until you enable “Allow agents to create connections” in
  Settings (or per workspace). Compose and inbox targets also stay put
  under stamp (no more writing the wrong inbox after identity stamp).

- **Copy that actually pastes.** New agent sessions inherit a proper
  UTF-8 locale, so box-drawing and typography copied from TUI tools
  (Claude Code, etc.) land on the pasteboard cleanly instead of as
  mojibake.

- **Terminal painter upgrades (opt-in WebGL still in Settings).** Better
  synthetic box/block glyphs, steadier scroll (no hop or jump-back while
  scrolled up), fuller TUI wheel forwarding, weight/smoothing fixes for
  “chonky” text, and emoji that keep their width and a bit more presence.
  Flip **Settings → Terminal → Terminal Painter** when you want WebGL;
  DOM remains the default.

- **Phone push that actually fires.** Feedback and project-chat already
  knew how to notify; a gateway URL normalization fix means those
  notifications reach registered devices instead of dying on a bad path.

## 0.40.44 — DNS your agents can manage

Point a domain at K2 and let an agent run its DNS — safely, because every
agent now carries its own identity.

- **Manage DNS from the CLI.** If a domain's nameservers point at K2, your
  agents can now view and change its records with `k2 dns` — `k2 dns
  access` to see what they're allowed to touch, `list` / `records` to
  read, and `record add|remove` for A / AAAA / CNAME / TXT / MX / SRV /
  CAA. It's **off by default**: nothing happens until you grant it, per
  server or per workspace, in Settings → K2 Connect. Creating or deleting
  whole zones stays a human-only action.

- **Every agent gets its own secure identity.** Under the hood, each agent
  session now carries an unforgeable credential that K2 issues the moment
  the session starts — so a capability you grant one agent can't be
  borrowed by another, even on the same machine. This is what makes
  handing an agent real power like DNS safe, and it's the foundation the
  rest of the permission system now builds on. Nothing to configure; it
  just works.

- **A cleaner CLI contract for agents.** `k2 publish` now speaks `--json`
  like `k2 dns`, `k2 mail`, and `k2 tunnel` do, and help text, error
  format, and exit codes are consistent across all of them — so agents
  scripting against K2 get predictable, machine-readable output
  everywhere.

## 0.40.43 — Resilient Edge

Your tunnel now survives infrastructure failures — and you get real control
over its lifecycle.

- **Automatic tunnel failover.** K2's secure tunnel now knows about a
  *list* of relay servers instead of one. If your relay becomes
  unreachable — mid-session or at connect time — the tunnel automatically
  re-homes to a backup relay within seconds and fails back once the
  primary has proven stable. New K2 Cloud servers get the two-relay
  configuration out of the box; existing tunnels keep working exactly as
  before and gain failover as their configuration updates. Nothing to set
  up, nothing changes about your `you.k2.dev` address.

- **Disable vs. Release — two clear tunnel controls.** *Disable* pauses
  the tunnel and stays paused across daemon restarts, reboots, and even a
  forgotten background daemon — no more zombie processes reclaiming your
  subdomain. *Release* goes further: it permanently retires this device's
  claim on the subdomain (with a confirmation step), so a stale backup or
  an old machine can never contest the name again. `k2 tunnel
  disable|enable|release` from the CLI, or Settings → K2 Connect in the
  app. `k2 migrate` now releases the old machine's claim automatically as
  its final step.

- **Public API toggle in Settings.** The `/v1` HTTP API can now be
  switched on per server by the owner in Settings (or `k2 api on|off`) —
  it takes effect immediately, no restart, and the `K2_API=1` environment
  variable still works as a force-on for headless deployments.

- **Fixed: remote updates on Linux now actually install.** On some Linux
  deployments, "Download & install" would verify and stage the new
  version, restart — and come back running the *old* one, with no error.
  (The helper process that swapped the binary was being killed by systemd
  before it could do its job.) The daemon now installs the update itself
  before restarting, so it works on any Linux box regardless of how its
  service was set up — and if an update ever fails to boot, it rolls back
  to the previous version automatically.

## 0.40.42 — K2 gets email

The big one: agents can now run and use **real email**. Three pieces, tied together by one CLI.

- **A mail CLI built for agents (`k2 mail`).** The layer that brings it
  all together: a workspace's agent can mint addresses, list and read
  incoming mail, block on `k2 mail wait` for a verification code, and
  send / reply / draft — all under your governance (`off` by default,
  `approval`, or `on`, with an outbox and audit trail). Every message body
  arrives wrapped in `BEGIN/END EXTERNAL EMAIL` markers so the agent treats
  it as data, never instructions. Whether an address is hosted by K2 or a
  linked account you own, it's the same verbs.

- **Host your own mail server — Email Hosting (Linux).** On a Linux
  deployment, Settings → Email Hosting stands up a real mail server: add a
  domain and K2 shows the exact **MX / SPF / DKIM / DMARC / rDNS** records
  to set, verifies them, then you mint **unlimited addresses** on that
  domain (cPanel-style, with catch-all and plus-addressing). A built-in
  **deliverability doctor** probes port 25, reverse DNS, blocklists, TLS,
  and open-relay safety and grades your send readiness; send **direct from
  the box** or via a **relay** (SMTP / SES / Mailgun / Resend). (On Mac the
  page shows with a "Linux deployments only" banner.)

- **Or link an inbox you already have — Email Linking.** Connect your own
  account as a read + draft (and, when you allow it, **send**) assistant
  inbox, bound to one workspace. Two ways in: an app-password over
  **IMAP** (Gmail, Fastmail, company IMAP), or **Gmail over OAuth** — sign
  in through Google in your browser, no app-password to generate. Reply
  drafts land in the account's real Drafts folder for you to review.
  **Microsoft (Outlook / 365) is coming soon.**

- **Attachments + bring-your-own OAuth.** `k2 mail send`/`reply` take
  `--attach <path>` (repeatable), and `k2 mail outbox` lists what you
  attached. And Settings → Email Link → **OAuth apps (advanced)** points
  K2 at your *own* registered Google/Microsoft OAuth client — your quota,
  your consent screen — instead of the built-in default; the client secret
  is write-only, vaulted and never shown back.

- **Organize + polish.** Move, flag, archive, delete-to-Trash, and manage
  folders on any inbox — plus a batch of help-text, status, routing, and
  wording fixes from real agent testing.

## 0.40.41 — The heartbeat CLI you were promised

- **Point a heartbeat at a trained session.** Every heartbeat tile (in
  Workspace Settings and the Heartbeat Settings page) now has a delivery
  drop-down: **Pinned chat**, **Own session** (fresh on next fire), or
  any saved session in the workspace — Claude, Codex, Gemini, whichever.
  Train a session once, then let the heartbeat wake *that exact session*
  on schedule; a one-line wakeup is enough because the session already
  knows the flow. The open button beside it jumps straight to wherever
  the heartbeat delivers. Agents get the same lever via
  `k2 heartbeat session <name> [--pinned|--auto|--set <id> --provider <p>]`.

- **`k2 connect` is now `k2 publish` (breaking).** Putting a subdomain
  on the internet is *publishing*, so the CLI verb finally says so:
  `k2 publish status` and `k2 publish subdomain
  create/list/point/rm/claim/unclaim`. There is no alias — `k2 connect`
  now fails with a one-line pointer to the new verb. (Only the CLI verb
  changed; the K2 Connect product and Settings page keep their name.)

- **`k2 heartbeat --help` is finally just help.** Asking any heartbeat
  command for help used to get parsed as a schedule and *written* —
  routine discovery corrupted your schedule state. Help now prints usage
  and exits, everywhere, guaranteed. (GH#22, #23, #24)
- **The documented commands exist now.** `k2 heartbeat schedule
  add/list/remove/edit/rename/enable/disable` and `k2 heartbeat signal
  fire/wakeup/wake` — the surface the docs and skills always described —
  are wired to the real named-heartbeat system. Bare `k2 heartbeat`
  lists your schedules instead of erroring. Unknown subcommands and
  misspelled flags are loud usage errors instead of silent writes or
  silently-ignored options. (GH#10, #24)
- **Give a heartbeat its job at birth.** `k2 heartbeat schedule add
  --instructions "..."` (or `--instructions-file <path>`) writes the
  WAKEUP.md at create time — no more schedules that fire with no defined
  work, and no $EDITOR required for headless agents. (GH#23, #24)
- **No more heartbeats that can never fire.** Creating a heartbeat now
  warns loudly if the schedule transport (launchd/cron) isn't installed
  or has stopped ticking. The daemon also rejects junk schedule writes
  from older CLIs and cleans up any junk they already left behind.
  (GH#22, #23)

## 0.40.40 — Project chat, your styles, and sessions that stay awake

- **Project chat is for the whole team.** Owners, Admins, and Members can
  all message a project's Point of Contact from Project Chat. Viewers stay
  read-only. Posts also show who actually sent them — not always as the
  host owner.
- **Your look stays yours when you switch servers.** Styles (Square /
  Glass / Bezel, palette, light/dark/auto, density) are personal to your
  app, not shared via the server. Connecting to a remote host no longer
  swaps your theme for someone else's day-shift skin.
- **Cmd+N creates one note.** A double-bound menu shortcut was opening
  several untitled documents at once — the same class of bug we fixed for
  Cmd+Shift+T. One press, one note.
- **Wake session in Projects stays alive.** Waking a team-member agent
  from a project dashboard now marks that workspace Active, so the
  session-cleanup reaper no longer kills it after ~15 seconds while you're
  still watching.

## 0.40.39 — The agent status you can trust

- **Tab spinners tell the truth now.** The little braille spinner on a
  tab used to die a second after you switched away — the app lost sight
  of hidden panes. Activity detection now lives in the daemon, which
  watches every session whether or not anyone's looking, so spinners
  stay accurate across tab switches, across windows, and across remote
  connections.
- **The completion chime rings when the agent actually finishes.** Same
  root cause: switching away used to fire the chime ~5 seconds later
  regardless of whether the agent was done. Now it rings at the real
  moment of completion — and only for work you weren't watching.
- **An amber square shows you WHICH tab finished.** When an agent
  completes while you're elsewhere, its tab now shows an amber square in
  the spinner slot (matching the Active bar's amber dot) until you visit
  it. Lots of tabs, one chime — now you know where to look. Hover still
  gives you the ✕ if you just want to close it.
- **Your agent sessions are now archived before providers delete them.**
  Some agent CLIs quietly remove session transcripts after ~30 days. K2
  now sweeps daily and copies aging sessions into
  `.k2/session-archive/` inside each project (originals untouched —
  resume keeps working). Default is 14 days; configurable via the
  `session_archive_days` setting, `0` disables.
- **Remote sessions feel local.** Tab renames, project settings, and
  chat history now update instantly for everyone connected to a server —
  no more reloading to see a teammate's changes.
- **Tab icons survive logout.** Agent launcher icons (and launch
  commands shown in tab tooltips) no longer vanish when you log out of a
  server and back in.

## 0.40.38 — Make K2 yours: Styles

- **Settings → Styles.** Pick K2's entire look: **Square** (the classic),
  **Liquid Glass** (frosted translucent chrome over an ambient canvas), or
  **Bezel** (layered-ring cards with a gold accent) — each with its own
  palettes. Hover any option for a live preview; switching is instant, no
  restart.
- **Light mode.** Square ships **Paper**, a warm paper-and-ink light
  palette — and a Light / Dark / **Auto** switch that follows your OS
  appearance through the day.
- **Density.** Square can breathe: Compact (the classic flush layout),
  Regular, or Spacious — panes float apart on the canvas and the seams
  stay draggable.
- **Terminals are part of the theme.** Every palette carries a full
  terminal color set (ANSI-16, cursor, selection) that live terminals pick
  up the moment you switch.
- **Glass has a Frost dial**, and reduced-transparency preferences are
  respected everywhere.
- **Make your own.** Styles are schema-validated data packages — see
  CONTRIBUTING-STYLES.md to submit a palette or a whole new look by PR.

## 0.40.37 — Spring cleaning, part one

- **The legacy `.k2so` era is ending — safely.** Every internal path now
  uses `~/.k2` directly, and on first launch K2 quietly rewrites any
  agent CLI configs (Claude/Cursor/Gemini) that still pointed at the old
  location. Nothing changes for you; the compatibility link stays in
  place as a safety net while the transition completes over the next
  releases. A new build-time guard makes sure the old paths can never
  sneak back in.
- **Settings tell the truth about file locations.** A few Settings labels
  showed the old `~/.k2so/...` paths for models and logs; they now show
  where the files actually live.

## 0.40.36 — Copy that

- **Copy in a remote terminal, paste on your machine — for real this
  time.** Selecting text in a TUI on a remote server now lands on YOUR
  clipboard. The 0.40.34 plumbing was right, but the final OS-clipboard
  write was silently rejected by the webview; it now goes through a
  native path that can't be. Copies go only to the person who made the
  selection.
- **Open any page in a browser tab.** Cmd+K, type or paste a URL (bare
  domains like `example.com` work), hit Enter — a browser tab opens.
  Previously only intercepted links could create one.
- **Remote workspace images pick from the server.** Setting a workspace
  or project icon while connected to a remote host now browses the
  HOST's files in K2's own picker instead of your local Finder.
- **Cmd+Shift+T spawns exactly one terminal.** A triple-stacked event
  bug could spawn 2–4 agents per press. One press, one terminal.
- **Server connections are more reliable for everyone, today.** A relay
  fix (no app update needed) cures a class of "Server unreachable" /
  silent connect failures where the app's requests could bypass tunnel
  routing entirely. If relaunching K2 used to fix your connection —
  this was why.

## 0.40.35 — Fresh-install pairing fix

- **New installs pair with the daemon again.** A recent change stopped
  creating a compatibility link the app relies on to find its daemon, so
  brand-new installs on 0.40.33/0.40.34 couldn't connect on first launch.
  Fixed — and if you already installed one of those versions, K2 repairs
  the link automatically the next time it starts on this version.

## 0.40.34 — The web comes to K2

- **A real browser tab.** K2 can now open web pages in a native browser
  pane — a new tab type alongside terminals and files, with an address
  bar. It's the foundation for agents that browse.
- **Terminal links open in K2 — even from remote servers.** When anything
  in a session opens a URL (`xdg-open`, `$BROWSER`, or clicking a link in
  the terminal), it now opens as a K2 browser tab on your screen — even
  when the session runs on a headless server an ocean away, where
  "opening a browser" used to mean nothing happening on a machine with no
  display.
- **Remote servers feel live now.** Project members, project chat, and
  feedback used to update in real time only on your local machine — on a
  hosted server you had to leave the page and come back. All of it now
  streams live over the same channel that follows your connection.
- **Copying inside TUIs works across the tunnel.** When a terminal app
  copies your selection (OSC 52), the text now lands on the clipboard of
  the person who selected it — and only theirs — whether the session is
  local or on a remote host.
- **API keys can finally reach workspaces.** `k2 api-key create` gained
  `--workspaces` ('*' or a list) — keys minted without a grant authorize
  nothing on the /v1 API by design, and the CLI previously had no way to
  say otherwise (it now warns loudly when you mint an ungranted key).

## 0.40.33 — Your files, where you left them

- **Dragging files into K2 always copies now — never moves.** Dropping a
  file from Finder into the file tree used to silently relocate the
  original, which could look like losing it entirely if you thought it
  was headed to a remote server. External drops are copies, full stop;
  reorganizing files within the tree still moves them like before.
- **Clones carry your whole workspace.** "Clone to server" quietly left
  behind anything your `.gitignore` listed — which usually meant your
  agent's entire `.k2/` folder (persona, skills, heartbeats) and your
  `.env` files, even with "Include secrets" checked. Agent state now
  always travels, and the Include-secrets toggle genuinely decides
  whether your `.env`/`.auth` files come along.
- **New machines stop growing a mystery `.k2so` folder.** The
  compatibility symlink now only appears on machines that actually
  migrated from the pre-0.40 layout.

## 0.40.32 — Multiplayer manners

- **Coming back to K2 no longer steals the session.** In shared sessions,
  resurfacing the app used to silently take control back from whoever was
  driving. Now simply looking is just looking — control changes hands only
  when you act: click the terminal, type into it, or switch your window to
  claimer mode. A network blip still restores control you already held.
- **Backgrounded windows keep up with takeovers.** If K2 was hidden or
  covered when someone took over a session, your terminal could miss the
  rescale and stay wrong until you poked it. Frames (and their
  acknowledgements) now apply within about a second even while the window
  is occluded, and the instant you bring K2 back it snaps current.
- **K2 Connect settings page stops asking for your password.** 0.40.31's
  fresh-install fix accidentally made the Settings → K2 Connect page
  demand your login-keychain password on every visit. The page now reads
  your session the same trusted way the daemon does — no prompt, ever.
- **Linux servers see updates again.** The update manifest only listed the
  macOS daemon, so Linux servers cheerfully reported "up to date" no
  matter how far behind they were (now fixed for this and all future
  releases — hosted servers will offer 0.40.31+ correctly).

## 0.40.31 — K2 comes to Linux

- **K2 Desktop on Linux (beta).** The full K2 app — terminals, projects,
  presence, the works — now builds and runs on Linux. This release ships
  `.deb`, `.rpm`, and AppImage packages for x86_64, and a `k2-daemon`
  `.deb` with a systemd user service for headless installs. The macOS app
  is unchanged and remains the flagship; Linux desktop is young — tell us
  what breaks.
- **Black screen after updates — fixed at the root.** If an update ever
  relaunched K2 into a black window, that was our watchdog trying to
  revive the interface with a tool that couldn't work when the interface
  was truly stuck. Recovery is now native (and has a second, harder
  fallback), the app re-reads daemon credentials after an upgrade instead
  of knocking with yesterday's key, and if the interface fails to load
  you get a real error message with a Reload button that actually appears.
- **No more password prompt after updating.** Signing in to K2 Connect now
  stores your session with the daemon pre-authorized to read it, so macOS
  stops asking for your login password after app updates — permanently,
  across future re-signings.
- **Hire agents straight into projects.** `k2 agent hire <dir> --project
  <name>` places a new agent into a project at hire time (repeatable for
  several), `k2 agent set --add-project/--remove-project` manages
  membership afterward, and `k2 agent get <ws> projects` shows where an
  agent belongs. The first member of an empty project becomes its
  point-of-contact, same as in the app.
- **One switcher to rule them all.** ⌘J now lists every active session —
  including pinned agent tabs — with tab names. The old Review Queue and
  Agent Ops pages are retired (View → Running Agents replaces the menu
  entry); reviews live on in each agent's Review tab.
- **Polish + plumbing.** Fixed five long-standing paper cuts the type
  system flagged (broken toasts in the wake scheduler, opening `.sol`
  files, enabling the editor minimap, a terminal setting stuck on, and
  warning toasts missing their color). Dragging the window from the top
  bar works on the Projects and Feedback pages. Under the hood: ~3,700
  lines of dead code deleted and the entire codebase now builds
  warning-free on both macOS and Linux, enforced by new CI gates.

## 0.40.30 — Talk to your server, not just at it

- **Spawn agent sessions over the API.** K2 Cloud servers (and any daemon
  you run with the API enabled) gain a host-sessions API: `POST` a prompt
  to `/v1/w/<workspace>/host-sessions` and a real agent session boots in
  that workspace, works, and **answers back over the same API** — agents
  reply with `k2 respond`, and you read the conversation with a plain GET.
  List a workspace's API sessions, resume one that's still live, or send
  a follow-up message to it. Requires a Pro subdomain.
- **Safe by default.** API-spawned sessions run with your agent preset's
  auto-approve flags **stripped** unless you explicitly opt the workspace
  in with the new `api_skip_permissions` setting — an unattended server
  never bypasses permission prompts just because a request came in over
  the wire. API sessions are also invisible to other workspaces and can't
  touch your human tabs.
- **Watch API sessions type from any client.** Session stream links minted
  by the API now connect as first-class viewers with the right identity —
  a phone or browser following an API session sees it live without
  claiming your seat.
- **K2 Cloud servers get the read-back plumbing out of the box.** New
  provisions ship with scoped hook tokens on, so `k2 respond` from an
  API-spawned agent lands back in the API conversation with zero setup.
- **Any agent over the API — for real.** API-spawned sessions now brief
  every agent on how to answer back (the contract preamble travels with
  the prompt), listing and resuming works for every agent that keeps a
  session store (Claude, Grok, Codex, Gemini, Pi, Cursor, Hermes — not
  just Claude), prompt delivery respects each agent's real startup
  behavior so slow-booting TUIs stop eating your first message, and an
  API caller's OpenAI/Gemini/xAI key is staged under the right env var
  for the workspace's agent, not force-fed as an Anthropic credential.
- **Custom agents are first-class citizens.** Agent presets now carry
  metadata — auto-approve flags to strip on API spawns (fail-closed: a
  custom agent's own bypass flags get declared and stripped, never
  silently trusted), environment variables (point any OpenAI-compatible
  harness at your local Ollama/LM Studio server), and startup-readiness
  hints. Manage it all headlessly with the new `k2 preset` CLI, and
  `k2 agent hire --agent <preset>` now actually sets the workspace's
  agent — hiring a Codex agent launches Codex, not the default.
- **The K2 agent contract, written down.** `docs/agent-contract.md`
  documents exactly what a custom or local-LLM agent must do to be
  first-class in K2 — and the sandbox now launches your workspace's
  real agent inside the cell instead of assuming everyone runs Claude.

## 0.40.29 — Companion groundwork

- **Push notifications for the K2 Companion app.** Your daemon can now
  register a phone for notifications and hand off alerts (a new feedback
  request, a project message from someone else) to deliver to it — all
  opt-in and content-free. Dormant until you turn push on; nothing leaves
  your machine otherwise.
- **Claim a terminal's size from your phone.** When you open a shared
  session in the Companion app, your phone can pin the terminal to fit its
  screen without other viewers fighting you for the dimensions. The pin is
  ephemeral — it clears the moment you disconnect.
- **Linux daemon downloads now work.** Headless and cloud installs can
  fetch and self-update to signed Linux daemon builds — the release
  pipeline now signs those artifacts correctly for the first time.

## 0.40.28 — Projects

- **Projects: group your agents into one effort.** A new top-level page
  (the top bar is now three tabs — **Agents | Projects | Feedback**) where
  you gather any set of workspaces into a named project. A workspace can
  belong to as many projects as you like. Give each project an icon and a
  color; pin your favorites; the left rail collapses to icon-only just
  like the Agents sidebar.
- **Every project has a Point of Contact.** One member agent is the PoC
  (the first one you add, reassignable anytime). Anything anyone else
  posts in the project chat is delivered straight into the PoC's live
  session — so the project always has an agent listening. K2 refuses to
  retire or remove a workspace that's still a PoC until you name a
  successor.
- **Project chat.** A resizable right-hand panel with one shared stream
  per project. Your messages show a "delivered to <PoC>" receipt; unread
  dots follow you to the nav, rail, and panel toggle. Agents post from
  their terminals with `k2 project msg` and catch up with
  `k2 project read`.
- **Dashboards you arrange like windows.** Each project gets dashboards
  (as many as you like — they're the tabs) where every pane is a live
  agent terminal or a pinned HTML page. Drag a member in and drop on any
  pane's left/right/top/bottom to split it, drag pane headers to move or
  swap, resize any divider in real time, or pick a preset arrangement
  from the layout menu. Layouts are shared — everyone sees the same
  dashboard — and never rearrange under someone who has it open.
- **Switch panes from the keyboard.** Panes show ⌘1–⌘9 badges; hit the
  combo to jump focus. Esc drops you back into the pane you last used.
- **Project settings live in Settings.** A new Settings → Projects section
  (same master-detail as Workspaces): members, PoC, dashboards
  (add/rename/reorder/delete), icon + color, pinned-HTML browser, and a
  danger zone that deletes the project without ever touching the
  workspaces. Right-click any project for a shortcut straight there.
- **Feedback, project-scoped.** Each project has a Feedback tab showing
  just its members' asks, and the main Feedback page gains a Projects
  filter.
- **The full `k2 project` CLI.** create / list / show / add / remove /
  poc / msg / read — with auto-detection of which project you're in,
  stdin piping, and paging. Run `k2 project --help` from any terminal.
- **Your agents know about all of this.** Agent-facing docs (the k2-cli
  skill, agent templates, hire charters, and `k2 glossary`) now teach
  both the Feedback channel and Projects — including exactly how to reply
  when a `[project:…]` message lands in their session. Existing agents
  pick this up automatically on their next wake.

## 0.40.27 — See who's here

- **Presence: know who's on your server.** The top bar shows everyone
  connected to the daemon — role-colored avatars (owner amber, admin purple,
  member blue, viewer gray) with a `+N` overflow. Click it for the full
  picture: every user, their role, how many windows they have open,
  which workspaces they're in, and how long they've been connected.
- **Moderate from one place.** Owners and admins can **kick** a user from
  the presence panel — their sessions are revoked and their connections
  close immediately — and grant a **temporary edit pass** to view-only
  users. Grants are a simple toggle and automatically reset to view-only
  when that user disconnects.
- **A new "Viewer" role.** Users you want watching, not driving: viewers
  can see everything but can't type, resize, or take over a terminal
  unless you flip their edit toggle. Set it per-user in Settings →
  K2 Connect.
- **Viewer/claimer mode per window.** A new top-bar toggle puts any window
  in view-only mode — great for demos and second screens. The server
  enforces it: viewer windows can't type into or resize shared terminals,
  so nobody's stray keystroke lands in your session.
- **Pin a terminal to a size.** From any terminal tab: pin to 80×24,
  100×30, 120×36, 160×48, or "match my window now." Everyone then sees the
  exact same grid — smaller windows scale it down to fit instead of
  fighting over the size. Made for teaching, demos, and pair sessions;
  unpin to go back to normal. The pin survives daemon restarts.
- **See where people are working.** Workspace rows in the sidebar now show
  mini presence avatars (who's in that workspace right now) in place of
  the old +/- git counters — the full change list still lives in the
  Changes panel.
- **The timer is now a stopwatch.** One click to start counting up,
  pause/resume and stop as before — the countdown presets and expiry
  pop-ups are gone. Your saved time entries and history are untouched.

## 0.40.26 — Agents can ask you things

- **`k2 feedback ask` — a durable question channel from agents to you.** An
  agent that needs a human decision no longer has to hope you're watching its
  terminal: it files an ask (question, approval, or FYI — with optional
  one-tap choices and a priority), keeps working or waits, and the ask sits
  on the new Feedback page until you handle it. `--wait` blocks the agent
  until your reply and prints it back, safe for scripting.
- **The Feedback page.** A "Feedback" button in the top bar (with a
  waiting-count badge) opens a two-column board: feedback cards on the left —
  workspace icon + name up top, live status controls on the card — and the
  conversation on the right. Filter by status chips, a searchable workspace
  picker, or free-text search. Each item has two tabs: **Thread**, and
  **Agent** — the asking agent's actual terminal, embedded, with one-click
  wake if it's dormant.
- **Replies land in the agent's session.** Every comment you write is
  delivered straight into the asking agent's terminal (waking it if needed),
  and the agent's replies appear in your thread instantly. It's a real
  conversation with a paper trail — your first reply to a waiting ask is
  recorded as the answer.
- **Know when an agent finishes — without staring.** Agents that finish
  working while you're looking elsewhere now show an amber dot in the Active
  bar until you check on them, with an optional soft chime (Settings →
  General) when it happens.
- **Desktop notifications** for new asks when K2 isn't focused, and agent
  persona templates now teach hires that the feedback channel exists.

## 0.40.25 — Any agent, first-class

- **Pick your default agent — for real.** The Editors & Agents default now
  genuinely works for any agent, and every workspace gets its own Default
  Agent dropdown. Cmd+Shift+T, heartbeats, wakes, and restart recovery all
  spawn your choice — Claude, Codex, Gemini, Grok, Cursor Agent, Pi, or
  Hermes.
- **Any agent's chat can be the workspace chat.** The pinned tab's session
  picker now lists every agent's sessions (with their icons) — pin a Grok
  or Hermes conversation as the canonical chat and K2 resumes it with the
  right binary and the right flags, including waking it by message.
- **Grok and Hermes sessions in your drawers.** Both agents' local session
  stores are discovered live, and the activity spinner / permission
  indicator now understand each agent's own signals — including Grok's
  "Action Required" state.
- **Terminal rendering, iTerm-crisp.** Box-drawing and block characters are
  now painted geometry instead of font glyphs (seamless TUI borders and
  logos), exotic art is pinned per-cell so animated symbols can't warp
  rows, the grid centers its leftover pixels evenly, and panes gained the
  width that used to be reserved padding.

## 0.40.24 — Hire agents from the command line

- **`k2 agent hire` — provision an agent in one command.** What used to take
  seven manual steps (folder, persona file, registration, mode, connections,
  launch, first message) is now one idempotent call: `k2 agent hire ~/agents/scout
  --template worker --connect "Appa" --onboard "Welcome aboard."` Bring your own
  persona with `--persona <file.md>`, or scaffold from a built-in archetype
  (worker, manager, qa, researcher). Re-running is always safe — it completes
  whatever's missing and reports what changed.
- **A full agent-management surface:** `k2 agent conf` (all settings), `get`/`set`
  (change mode, name, persona — with automatic persona backups), `list` (your
  fleet at a glance), and `retire` (safe decommission: refuses if it finds
  uncommitted git work or likely secrets, and archives the folder — never
  deletes). Everything supports `--json` and `--dry-run`, designed for both you
  and your agents to drive.
- **Agent names are now human.** Display names can have spaces and capitals
  ("QA Bot"), and renaming an agent updates how you address it everywhere.
- **Paused agents stay paused.** Disabling an agent now survives a server
  restart — it won't quietly come back to life.
- **CLI footguns fixed:** `--help` on workspace commands no longer acts on the
  literal text "--help" (it once created a workspace named that), `workspace
  remove` accepts agent names (not just paths), and setting agent modes no
  longer scribbles into your persona file.

## 0.40.23 — Heartbeats you can trust

- **Missed heartbeats now catch up.** If your machine was asleep or off when a
  heartbeat was scheduled, it fires once when K2 comes back — no matter how
  long it's been — instead of silently skipping. Restarting the daemon also
  fires anything that came due while it was down.
- **Firing windows.** Each heartbeat can now be limited to a time-of-day range
  (say, hourly but only 9 AM–5 PM). Fires that come due outside the window wait
  for it to open.
- **Manual runs no longer eat the schedule.** Launching a heartbeat's agent by
  hand doesn't consume that day's scheduled fire anymore.
- **Failures are visible and bounded.** A heartbeat that keeps failing backs
  off, then disables itself after five straight failures with a clear badge —
  and re-enabling it resets the slate.
- **The scheduler can't die silently.** If the background scheduler ever goes
  missing (it could, invisibly, for weeks), K2 now shows a "transport down"
  notice and reinstalls it automatically.
- **Snappier workspace switching.** Workspaces with long histories no longer
  stall on entry: saved layouts self-clean leftover empty terminal tabs from
  older versions, and hidden terminals wait to start their shells until you
  actually view them.

## 0.40.22 — Kessel: a first-class terminal

Meet **Kessel** — K2's terminal, rebuilt to feel first-class. It's the same
engine that already let one session be watched from many screens at once; this
release makes it *smooth*.

- **Buttery scrolling.** Terminal scrolling is now pixel-smooth with a real
  overlay scrollbar, and it keeps pace with your display instead of stuttering.
  Fullscreen agent UIs (like Claude's) scroll at full frame rate too.
- **Resizing no longer flashes black.** Resizing a terminal — or a whole window
  — now dissolves from the old size to the new one instead of blanking to black
  mid-reflow.
- **Click, select, and copy inside fullscreen agents.** In a fullscreen agent
  UI you can now click to move the cursor, drag to select, and copy — and the
  copy lands on your clipboard as the app intended it, clean. Hold Shift or
  Option while dragging for K2's own text selection instead.
- **Copy that respects real line breaks.** Copying wrapped lines rejoins them
  into one line, wide characters (CJK/emoji) line up correctly, and there's no
  more stray trailing space.
- **Instant workspace switching.** The pinned chat for each of your active
  workspaces stays warm in the background, so hopping between workspaces — even
  on a remote server — is instant instead of reloading each time.
- **An experimental GPU renderer.** Settings → Terminal → Terminal Painter →
  WebGL turns on a GPU-accelerated painter for the terminal. Off by default.

## 0.40.22 also includes

- **Send and receive files of any size.** Sending files to a server is no
  longer capped at 100 MB — large files stream with a progress bar. Right-click
  a folder on a remote server to **compress** it, and right-click a file to
  **download** it back to your computer, at any size.
- **Clone a workspace back to your computer.** When you're working on a remote
  server, "Clone to This Computer" pulls a whole workspace — files, chats, and
  history — down to your local machine, even if that server has never heard of
  your computer.
- **Reconnecting to updated servers, finished.** Updating or restarting a
  server now recovers cleanly even when your saved login expired during the
  update: K2 shows "restarting…", re-authenticates on its own, and only sends
  you to the sign-in screen if your credentials genuinely no longer work — no
  more relaunching the app to reconnect.
- **The busy spinner clears when an agent is done.** Fixed a case where a
  workspace's activity spinner could spin forever after you switched away from
  it mid-task.
- **Federation settings in one place.** "Enable federation" and "Let remote
  users message agents" now sit together under Remote access in K2 Connect,
  and both are restricted to Owner and Admin — a Member can no longer change
  them. Messages queued for an offline server now actually deliver when it comes
  back, instead of silently waiting forever.
- **Stability: terminal sessions no longer leak.** Fixed a long-standing issue
  where closed terminals and split-column layouts could pile up unused sessions
  on a server — and, with two apps or windows on one server, exhaust it. Closing
  a tab now reliably ends its session.

## 0.40.21 — Agents by API, smoother terminals, federation fixes

- **Run coding agents on your K2 server by API.** Authenticated API calls can
  start a Claude Code session inside any workspace — in a hardened sandbox with
  a timeout you control — or message the workspace's own agent directly, the
  same one you chat with in the app. API-launched sessions appear live as
  orange tabs in the workspace.
- **Sandboxed chats in the Chats panel.** A new collapsible Sandboxed section
  lists every API-launched session; click one to relaunch it right back into
  its sandbox.
- **Smoother terminals.** Output rendering is frame-paced, wheel scrolling is
  snappier, and copying text out of a terminal grabs clean lines without stray
  padding.
- **Cross-server messaging repaired** (broke in 0.40.20): pairing no longer
  records unroutable peers, and a declined message now tells the sender it
  didn't land instead of silently claiming success.
- Fixed a first-provision crash in secure-tunnel certificate setup and a CLI
  crash in `k2 terminal write`.

## 0.40.20 — Smoother server updates + tighter inbox trust

- **Updating or restarting a connected server no longer needs an app relaunch.**
  When you restart or update a server from its tile, K2 now watches for that
  server to actually come back online (its real readiness signal, not a guessed
  timer), shows a "reconnecting…" state, and reconnects on its own — instead of
  getting stuck on "Load failed" until you quit and reopen.
- **A workspace with Remote Access off now fully declines cross-server
  messages.** Previously such a message quietly landed in the agent's inbox; now
  it's declined outright and the sender is told it didn't land. A workspace
  you've opted out of remote instruction never receives un-requested work in its
  trusted inbox. (Both servers need 0.40.20.)

## 0.40.19 — Cross-server replies just work

- **Replying across servers no longer trips on capitalization.** Connection
  addresses (`agent@server.k2.dev`) are now matched **case-insensitively**, so a
  reply from a connected agent goes through even when the connection was
  auto-created from a folder name with different casing (e.g. `Cortana` vs
  `cortana`). No more manually re-adding the lowercase form.
- **Agents are addressed by their real name.** A cross-server message now
  carries the sender's **display/persona name** (not the workspace folder name),
  so the `[from …]` attribution and the reply target line up. (Both servers need
  0.40.19.)

## 0.40.18 — Agents actually talk across servers

- **Cross-server agent messaging now works end-to-end.** `k2 msg
  <workspace>@<server>.k2.dev` reaches a connected agent on another of your
  servers exactly like messaging a local one — and the message now **lands in
  the recipient agent's chat** (or its inbox), so it's actually seen and can be
  replied to. Before, a cross-server message was accepted but silently dropped
  on the receiving side; that's fixed.
- **Same commands, local or remote.** The CLI tools behave identically whether
  the agent is on this machine or a connected server — so you and your agents
  can rely on `k2 msg` working the same everywhere. A message only goes through
  if the connection exists (otherwise you get a clear *"not a connection — add
  it with `k2 connections add …`"*). Replying is just `k2 msg` back.
- **You control which workspaces a remote agent can reach.** A remote message
  only drives a workspace's agent if that workspace has **Remote Access** turned
  on; otherwise it lands in that agent's **inbox** to pick up — never lost,
  never forced. Cross-server messaging still requires an owner-confirmed
  pairing + an explicit connection. (Both servers need 0.40.18.)

## 0.40.17 — Cross-server connections that actually connect

- **Connect a workspace on another of your servers — both directions.** Type a
  remote agent's full address (`workspace@server.k2.dev`) into a workspace's
  **Connected Workspaces**, and K2 pairs the two servers and records the link on
  *both* sides, so your agents can message each other across machines either
  way. Works as long as you're an **owner or admin** on the other server. (Both
  servers must be on 0.40.17.) This is the fix that makes the 0.40.16
  cross-server feature actually work over K2 Connect.
- **Connected servers survive a restart or update — no relaunch.** After a
  remote server restarts or updates, the app now reconnects on its own instead
  of getting stuck behind a dead connection. You no longer have to quit and
  reopen K2 to reach a server you just restarted.
- **Friendlier connection management.** When a saved server's sign-in expires,
  its tile offers **Sign in** again instead of silently failing; cross-server
  errors now say what's actually wrong (not signed in / must be owner-or-admin /
  federation off); the add box guides you to type a full
  `agent@server.k2.dev` address; and per-server update status sits tidily
  beneath each server's buttons.

## 0.40.16 — Connect your servers, manage them in place

- **Connect a remote workspace to a workspace in one step.** As an owner, type
  the remote workspace's address — `workspace@yourserver.k2.dev` (the part
  before the `@` is the workspace name on that server) — into a workspace's
  Connections, and K2 pairs the two servers for you (mutual trust, no codes to
  compare) and records the connection, so your agents can message each other
  across machines. (Both servers must be on 0.40.16.)
- **Operate your connected servers from their tiles.** Each saved server in
  **Settings → Connections** now has real buttons: **Sign in** (manage it in
  place without switching to it), **Restart**, **Check for updates / Update**,
  and a **Federation on/off** badge.
- **The Connections page reflects the server you're viewing.** When you're on a
  connected server, the panel shows what *that* server is federated with and its
  cross-agent connections — not your own device's address book.
- **Turn federation on or off on a connected server, remotely.** Flip it from
  **Settings → K2 Connect** while viewing that server (or headless with
  `k2 fed enable/disable/status`).
- **Your draft messages stick around.** The compose bar keeps what you've typed
  when you switch workspaces — and even if the app restarts — per terminal, on
  your device. Works on remote servers too.
- **Fixes & polish.** Switching servers on the Connections page no longer
  crashes; the saved-server list is sorted alphabetically; the
  Remember-password checkbox shows a proper checkmark.

## 0.40.15 — Cross-server connections + talk reliability

- **`k2 talk` reliably delivers again.** A shell-parsing bug could abort a
  `talk` send right before delivery on some systems — fixed, so agent-to-agent
  messages land every time.
- **The message composer no longer disappears on remote machines.** When you're
  driving a connected K2 server, the compose bar stays available; authorization
  is enforced by the daemon either way.
- **Federation, made operable (experimental, opt-in).** You can now turn
  cross-server agent messaging on **per server** from **Settings → K2 Connect →
  Enable federation** (no config files), and connect a *specific* remote
  workspace to a workspace with `k2 connections add workspace@yourserver.k2.dev`.
  Agents can only
  message a remote agent that's an explicit **connection** — same-server agents
  still reach each other freely; cross-server requires the connection. Still
  early and off by default.

## 0.40.14 — Cross-server agent messaging (experimental, opt-in)

- **Your agents on different K2 machines can message each other.** Pair two of
  your own K2 servers and an agent on one can send a message into an agent's
  **inbox** on the other — over the same end-to-end-encrypted path as K2 Connect
  (the relay only ever sees ciphertext; each message is signed and verified
  against the peer's pinned key). As an owner/admin you can see a paired server's
  agents in **Workspace Connections** and address them directly.
- **Off by default, for self-hosted fleets.** This is early and **opt-in** —
  enable it per-daemon (the `K2_FEDERATION` flag) on the machines you want
  federated. Pairing is owner-confirmed. Cross-server messages are delivered to
  the inbox and **never auto-run** — a remote message can't drive or spawn a
  session on its own.

## 0.40.13 — Per-workspace remote access + agent reliability + the federation groundwork

- **Choose which workspaces accept remote messages.** A new **Workspace
  Settings → Remote Access** toggle lets you opt *specific* workspaces into
  being messaged by your K2 Connect users — instead of one all-or-nothing
  account setting. Default **off**: a workspace never accepts a remote
  instruction until you turn it on (the owner can always message their own
  workspaces). Your existing account-wide setting still works as a master
  switch.
- **Agents in workspaces without a persona file now behave correctly.** Some
  workspaces (especially newly-created ones) didn't have an `AGENT.md` persona
  file, which silently stopped their heartbeats, inbox wake-ups, and canonical
  chat session from working. K2 now resolves a workspace's agent identity from
  its actual configuration, so those workspaces run like any other.
- **Security hardening (opt-in): scoped per-session agent-hook tokens.**
  Groundwork so an agent's lifecycle hooks can use a per-session, capability-
  scoped credential over a private per-cell socket instead of the broad owner
  token — behind an off-by-default switch, no change unless you enable it.
- **Branded installer.** The `.dmg` install window now carries the K2 lockup
  and a drag-to-Applications guide.

## 0.40.12 — Message your agents from the app + a live Agent Ops view

- **Message an agent right from its terminal.** Every terminal now has a compose
  bar at the bottom — type a note and press **Enter** to send it straight to the
  agent running there (Shift+Enter for a newline). It's delivered as a proper
  message, not raw keystrokes, so it lands cleanly even mid-task. Your messages
  are attributed to your name.
- **Set your display name.** Settings → General → **Your name** controls how your
  messages to agents are signed (defaults to "owner").
- **Agent Ops — a live view of your fleet (Cmd+Shift+O).** A new pane, opened from
  the top bar, shows what your agents are doing across every workspace in one
  place: active sessions and a live activity feed. The start of K2's
  management console.
- **Optional on-device detection for "agent is waiting on you."** A new **AI
  Workspace Assistant → "Use local LLM to detect HITL"** toggle. It's **off by
  default** — detection stays fast and deterministic — but once you've loaded a
  local model you can turn it on to let the on-device 1.5B model help spot when an
  agent is paused on a question. Fully local; nothing leaves your machine.
- **`k2 talk` (beta) — agents collaborating over the CLI.** A new command that
  lets one agent read a peer's terminal and respond in a single step, for
  orchestration setups. (Power-user / agent feature; everyday messaging is the
  compose bar above.)

## 0.40.11 — Unsaved edits protected + scroll fixed in fullscreen TUIs + canonical fix

- **Mouse-wheel scrolling works again in fullscreen terminal apps.** When a
  terminal program took over the screen with mouse support on (e.g. Claude Code
  in `/tui fullscreen` mode), the scroll wheel did nothing — K2 now forwards
  wheel ticks to the app so it scrolls its own view, the way native terminals do.
  Plain shells and inline TUIs scroll exactly as before.
- **Leaving a workspace with unsaved file edits now asks before you lose them.**
  If you edit a markdown (or other) file and switch to another workspace before
  saving, K2 prompts **Save / Discard / Cancel** instead of silently reverting to
  the original — no more spending 45 minutes on a file and coming back to find it
  reset. Save writes all your edits, Discard drops them, Cancel keeps you put.
  (Cmd+S still works for explicit saves; switching tabs within a workspace was
  always safe.)
- **The "K2 Canonical Agent" checkbox now sticks on remote servers.** When you
  were connected to a remote K2 Connect host, enabling canonical agents would
  appear to revert to unchecked when you left and returned to Settings — the
  setting was being read from your *local* machine instead of the host it was
  saved on. The canonical-agent state is now read from the correct server.

## 0.40.10 — No more "enter your password" prompt on every update

- **Updating K2 no longer pops the "K2 needs to install the command-line tool —
  enter your password" dialog every time.** That admin prompt was firing on each
  launch/update because the background check that keeps the `k2` command healthy
  mis-detected a perfectly good install as needing repair (a macOS path quirk)
  and then asked for your password to "fix" it. Now the check is exact, and the
  background heal never asks for a password — it only does the work it can do
  silently. The one place that can ask for your password is the deliberate
  **Settings → Install CLI** button, exactly once.

## 0.40.9 — Remote updates no longer log you out + login reliability

- **Updating a remote server no longer signs everyone out.** Your K2 Connect
  login now survives a server restart (including a remote self-update), so a
  routine update doesn't kick you — or any connected teammate — back to the
  sign-in screen. Sessions are stored securely on the server (only a hash of
  your token is ever written to disk, never the token itself).
- **Revoking access is still instant — even across restarts.** Disabling,
  removing, or changing the role/password of a user cuts off all of their live
  connections within a few seconds and stays revoked through any restart. A
  user disabled while the server was briefly down is rejected the moment it
  comes back.
- **Log out everywhere.** A new sign-out path cleanly ends a session on the
  server, and brute-force lockout counters now survive a restart too.
- **More reliable sign-in.** A stale connection to the relay no longer makes the
  first login attempt fail with a confusing error — K2 retries transparently.
- **Hardened the encrypted tunnel.** Added connection timeouts and a concurrency
  cap so a stalled or flooded connection can't tie up the server, plus a
  constant-time check on the owner credential. (Security hardening; no action
  needed.)

## 0.40.8 — No more black screen + reliable reconnect after a remote update

- **Leaving Settings can no longer black-screen the app.** If a view ever hits
  an unexpected error, K2 now shows a small recoverable panel with a **Reload**
  button instead of going to a blank screen that needed a right-click → Reload
  or a relaunch.
- **Updating a remote server no longer strands you on "Connecting…".** After you
  remotely update a server you had saved, the app reliably drops to the
  **sign-in screen** so you can re-authenticate, instead of getting stuck
  forever trying to connect with a session the server's restart had cleared.

## 0.40.7 — Clearer remote-host version + update info

- **When you're connected to a remote host, the General settings now always
  show what version that host is running** — previously the host's version
  only appeared when an update happened to be available, so an up-to-date
  host showed nothing.
- **The "App Version" line is now labeled "This Mac"** while you're connected
  to a remote, so it's not mistaken for the host's version.
- **Update checks compare versions correctly**, including pre-release builds
  (e.g. an `-rc` build now sorts before its final release instead of being
  treated as equal).
- **If a newer version exists but there's no build for the host's platform,**
  K2 now says exactly that instead of misreporting the host as up to date.

## 0.40.6 — End-to-end encryption on by default

- **Your K2 Connect tunnel is now end-to-end encrypted, on by default.**
  Traffic between your machine and whoever connects is encrypted the whole
  way — the relay that forwards it only ever sees scrambled ciphertext, never
  your terminals, files, or keystrokes. It just works with no setup; the
  daemon provisions its own certificate automatically. (Advanced users can
  opt out by setting `K2_E2E=0`.)

- **Turning on the Canonical Agent never overwrites your files.** When you
  enable harness fan-out, any AI-tool files you already have (`CLAUDE.md`,
  `AGENTS.md`, `.cursor/rules`, …) are **moved** into a `.k2/migration/`
  folder inside your workspace before K2 replaces them with links to its
  generated context — nothing is deleted and you can restore anything from
  that folder. If a file can't be moved for any reason, K2 leaves it exactly
  where it is rather than touch it. No recycle bin, no countdown clock.

- **Simpler Canonical Agent Flow settings.** Removed a duplicate "enable
  fan-out" toggle and a workspace-state list that couldn't be acted on from
  there — the checkbox on each workspace is now the one place you turn this
  on. The confirmation pop-up is wider and clearer.

- **K2 Connect stops re-asking for keychain access.** Click **Always Allow**
  once on the keychain prompt and it stays allowed — K2 now records itself as
  a trusted app for its own K2 Connect and companion keychain items, so the
  prompt no longer reappears every time you open the K2 Connect settings.

- **Hermes is a built-in agent preset.** It now appears in the agent list
  (after Pi), with the Nous Research mark sized to match the other icons.

- **Pick your subdomain — no more phantom default.** When you signed in to
  K2 Connect, the subdomain menu *looked* like it had already picked one of
  your purchased subdomains, but nothing was actually selected — so starting
  the tunnel failed with a confusing "tunnel not configured" error. The menu
  now stays empty ("Select a subdomain…") until you choose, choosing is what
  arms the tunnel, and Start tells you plainly if you haven't picked yet.

- **"Clone to" handles large workspaces.** Cloning a workspace to another of
  your machines over K2 Connect used to fail once the bundle passed ~75 MB —
  the whole thing was sent in a single chunk and rejected. Large bundles now
  stream to the destination in small pieces, so multi-gigabyte workspaces
  copy across without hitting a size ceiling or spiking memory. Smaller
  workspaces are unchanged and still work with older machines on the other
  end.

## 0.40.5 — Menu bar icon refresh

- The macOS menu bar icon now shows **K2** — it previously still read **K2SO**.

## 0.40.4 — The `.k2/` workspace rename + access fixes

- **Your workspace's `.k2so/` folder is now `.k2/`.** The K2 rebrand
  finally reaches the per-workspace directory. The first time the updated
  daemon starts, K2 renames each registered workspace's `.k2so/` to
  `.k2/` automatically, re-points the harness symlinks (`CLAUDE.md`,
  `GEMINI.md`, and the `.claude/`/`.opencode/`/`.pi/` files), and updates
  `.gitignore` so the previously-ignored `inbox`/`sessions`/`logs` stay
  ignored. Nothing is lost — git sees it as a clean set of renames, so
  your history follows; just commit the rename in each repo when it's
  convenient. Workspaces still on `.k2so/` keep working — K2 reads either
  name. This is the last structural rename; everything from here is
  additive.

- **Disabling or removing a K2 Connect user now ends their live session
  immediately.** Previously a revoked user's already-open terminal stream
  kept flowing until they happened to disconnect. Now disabling,
  removing, or changing a user's role drops their token and tears down
  any live terminal/events socket within seconds.

- **CLI version no longer shows "v?" after the rename.** If you updated
  in place, the `k2` command-line tool could still point at the old
  `K2SO.app` path and report its version as `v?`. K2 now self-heals the
  `/usr/local/bin/k2` link on startup — whether your app is named
  `K2.app` or the older `K2SO.app`.

- **The "Canonical Agent" harness fan-out toggle stays checked.** Turning
  it on no longer silently reverts — a leftover "skip harness management"
  flag was overriding it, and that's now cleared when you enable fan-out.

## 0.40.3 — Honest message-delivery status

- **`k2 msg` stops crying wolf.** Live messages between agents sometimes
  reported as failed (`no_submit`, exit code 2) even though they
  delivered fine — a false alarm from the after-send screen check racing
  the recipient's redraw, which could push agents into re-sending and
  stacking duplicate text. Delivery itself was already solid (the paste
  framing and insurance Enter from the comms-reliability release keep it
  that way); now the *status* reflects reality: a message handed to a
  live agent session with its submit keystroke is reported as sent.
  Genuine failures — a crashed recipient session — still surface loudly.

- **Updated from K2SO and want the app file itself named "K2"?** An
  in-place update keeps the old `K2SO.app` filename on disk (its name and
  icon already display as **K2**, so this is purely cosmetic — updates
  keep flowing to it either way). For a clean `K2.app`, download the
  latest from **[k2.dev](https://k2.dev)** and drag it into your
  Applications folder, replacing the old one. Optional, not required.

- **Remote access is "K2 Connect" in Settings again.** The remote-access
  tab briefly carried an internal codename; it's back to the clear,
  familiar **K2 Connect** label.

## 0.40.2 — Clearer permission prompts after the rename

The K2SO → K2 rename means macOS sees a brand-new app, so a few system
permission prompts can appear once after updating. This release makes
them clearer and reduces how often you'll see them:

- **The CLI-install password prompt now explains itself.** Instead of the
  generic "osascript wants to make changes," it reads "K2 needs to
  install the k2 command-line tool…" so you know exactly what you're
  approving.
- **Your K2 Connect sign-in carries over without opening Settings.** If
  you had a tunnel running, K2 now moves your saved session to its new
  home at startup — your tunnel keeps working without you having to visit
  the K2 Connect page first.
- **The keychain prompt is friendlier.** When macOS asks to unlock your
  K2 Connect credentials, it now names them "K2 Connect sign-in" instead
  of a cryptic identifier. (macOS controls the rest of that dialog's
  wording — the lock icon and title are the system's, not ours.)

## 0.40.1 — CLI upgrade heals itself

- **If you had the CLI installed, K2 now finishes the upgrade for you.**
  On first launch, K2 detects a pre-0.40 CLI install and offers to set up
  the new `k2` command (one admin prompt) — keeping `k2so` working as an
  alias. No trip to Settings needed.
- **The `k2so` compatibility alias works again from `/usr/local/bin`.**
  On machines that had the CLI installed, the 0.40.0 alias resolved its
  new `k2` sibling next to the symlink instead of next to the real
  script, and failed. It now finds the bundled CLI no matter how it's
  invoked.
- **`k2 daemon status` stops saying "Running: no" while the daemon is
  running** — a long-standing parse bug on macOS, now fixed (it shows
  the PID and port again).
- Last few "K2SO" mentions in CLI messages updated to K2.

## 0.40.0 — Welcome to K2 by Alakazam Labs

K2SO is now **K2**. New name, new icon, new home — same product, and
everything you had is exactly where you left it.

**What this update did on your machine (automatically):**

- Your app is now **K2.app** with the new K2 icon.
- All workspaces, sessions, agents, schedules, settings, and your
  K2 Connect sign-in carried over. The daemon, heartbeat, and credential
  refresher re-registered themselves under their new names and
  reconnected on their own — if you can read this, it worked.
- The CLI is now **`k2`** (try `k2 activity`). Your existing `k2so`
  commands and scripts still work — `k2so` stays as an alias through
  the 0.x series, and prints a gentle reminder to switch.
- Your data folder moved from `~/.k2so` to `~/.k2`, with a
  compatibility link left behind so older scripts keep working.
- Updates now come from K2's new home:
  [github.com/Alakazam-211/K2](https://github.com/Alakazam-211/K2).

**Licensing, as announced in 0.39.48:** K2 is now **Fair Source**
(FSL-1.1-Apache-2.0). Free to use for individuals and businesses, source
visible, each version converts to Apache 2.0 after two years. Details:
LICENSE.md, COMMERCIAL_HOSTING_GRANT.md, and TRADEMARKS.md in the new
repository.

Thanks for riding along since the K2SO days. Onward.

## 0.39.48 — K2SO is becoming K2

This is the last release under the K2SO name. The next update is **K2 by
Alakazam Labs, version 0.40.0** — same product, same workspaces, same
daemon, new name and new home.

**What happens when you update to 0.40.0:**

- The app becomes **K2** (you'll see the new icon and name). Your
  workspaces, sessions, agents, settings, and K2 Connect sign-in all carry
  over automatically — the update migrates everything on first launch and
  reconnects on its own. Nothing for you to do.
- The CLI becomes **`k2`**. Your existing `k2so` commands and scripts keep
  working — `k2so` remains as an alias through the 0.x series.
- Development moves to a new repository:
  **github.com/Alakazam-211/K2**. Updates continue to arrive in-app
  exactly as before.

**A note on licensing — please read:**

K2 is released under the **Functional Source License (FSL-1.1-Apache-2.0)**
— a Fair Source license. For you, nothing changes: K2 stays free to use,
for individuals and businesses, source visible as always, and each version
automatically converts to Apache 2.0 after two years. The license restricts
one thing: selling K2 itself (or a hosted version of it) as a competing
product. Hosting K2 for clients stays permitted under a standing grant when
remote access runs through the official K2 Connect tunnel. Full details ship
in LICENSE, COMMERCIAL_HOSTING_GRANT.md, and TRADEMARKS.md.

If you have questions about the change, open an issue — we read them all.

## 0.39.47 — Active-bar switches land on the right pinned chat

- **Switching workspaces from the Active section no longer flashes the wrong
  conversation.** Clicking a workspace in the Active bar (and jumping via the
  command palette, keyboard shortcuts, or the review queue) could briefly run a
  hidden *double* workspace switch — the focus-group change auto-activated the
  group's first workspace before your actual target — and the two racing
  switches could stamp another workspace's pinned Chat session into the one you
  opened. Every entry point now performs exactly one switch, same as clicking
  in the main workspace list.

## 0.39.46 — Remote terminals stop losing lines

- **The "missing line" remote-terminal bug is fixed at the root.** When viewing
  a terminal from a connected client, typing past a line wrap (or Claude's input
  box growing) could leave the view missing the row that just moved — and it only
  healed when you switched tabs and back. The cause: with more than one viewer
  attached to a session (your remote client plus the host app counts as two),
  each screen update was delivered to whichever viewer asked first and *silently
  skipped* for the rest. Every session now has a single broadcaster that encodes
  each update once and delivers it to **every** viewer — nobody gets starved, no
  matter how many windows, machines, or tabs are watching.
- Side benefit: with several viewers on one session, the server now does the
  rendering work once instead of once per viewer.
- Locked in by integration tests that attach two live viewers and assert both
  see every keystroke — including a late-joining viewer arriving mid-session.

## 0.39.45 — Messages that actually arrive: the comms-reliability release

- **`k2so msg` now confirms the Enter actually landed.** Under heavy host load
  (big agent fleets), a delivered message could sit un-submitted in the
  recipient's input box until a human pressed Enter — while the sender saw
  `success: true`. Delivery now wraps the message in explicit paste framing and
  *verifies* submission against the recipient's screen, re-sending Enter until
  it lands. If it truly can't confirm, you get an honest `no_submit` error
  (with guidance) instead of a silent stall.
- **Long messages stop getting clipped.** Inbox memos silently truncated at
  ~2.7 KB (~54 lines) and long live messages lost their tails — the payload
  rode the URL, which had a hard cap. Message text and inbox bodies now travel
  in a proper request body with no size limit, and anything that *would*
  overflow the old path errors loudly instead of corrupting the record.
- **Workspace names are forgiving now.** `k2so msg appa` finds the workspace
  named `Appa`; misses come back with a "did you mean …?" suggestion instead of
  a bare not-found.
- **`k2so connections` stops lying.** `add` failures actually print an error
  (they used to exit silently as if they'd worked), and `list` shows *whose*
  connections you're looking at and flags peers that are no longer reachable.
- **New workspaces show up immediately.** A workspace added from another
  window, the CLI, or a connected client now appears in the sidebar and
  Settings on every client — no more manual window reload. CLI-created
  workspaces no longer vanish into an unreachable no-focus-group limbo.
- **Create workspace folders from a connected client.** The remote folder
  picker has a "+ New Folder" button, so you can create a fresh workspace
  folder on the host instead of only adopting existing ones.
- **`k2so settings --mode` and `k2so workspace list` work.** The mode setter
  was a silent no-op and the list always failed with no output; both do their
  jobs now.
- **Pinned tabs can't borrow a sibling's identity anymore.** The server now
  heals any pinned Chat/Inbox tab stamped with a neighboring workspace's
  identity during a switch race — on save *and* when serving older corrupted
  layouts.

## 0.39.44 — Clone-to actually brings your Claude chat history

- **Cloned workspaces now arrive with their conversations.** A folder-naming
  mismatch meant K2SO and Claude Code looked in *different* directories for a
  workspace's sessions whenever the path had an underscore (or a dotted name, or
  lived under a symlinked location) — so a "Clone to" could report success yet
  bring **zero** chat history, and `/resume` came up empty. K2SO now names those
  folders exactly the way Claude Code does, so sessions — including worktree
  history — bundle, transfer, and resume correctly. A one-time self-heal repairs
  earlier clones (and local workspaces) that had landed in the wrong folder, and
  the clone now logs how many sessions it moved.

## 0.39.43 — Right-sized terminals + reliable pinned-chat switching

- **The terminal sizes for whoever's actually looking.** With multiple clients on
  one server, the PTY now follows the active viewer — the focused client's (or
  whoever just typed) screen dimensions are used — so a remote viewer no longer
  gets a terminal clipped to someone else's window. Whoever last interacts owns
  the size, and it hands back cleanly when the other side takes over.
- **Switching the pinned chat's session works again.** Picking a different
  conversation from the pinned-tab dropdown now reliably loads it: your explicit
  choice is honored instead of being quietly reverted to the most-recent session.

## 0.39.42 — Tabs behave when two clients share a server

- **No more tab/terminal flicker with multiple clients.** When two windows — or
  two people — were connected to the same server, tab-order sync could spiral
  into a fast loop that constantly rebuilt the tabs and reloaded the terminals
  (and the pinned chat). Tab order now syncs cleanly and quietly.
- **Reordering a tab just moves the tab.** Adopting another client's reorder no
  longer rebuilds the panes — your terminals and pinned chat stay put instead of
  reloading.
- **Your selected tab is yours.** Which tab you're looking at is now per-client:
  a teammate reordering tabs or switching their view no longer drags yours along.
  Each person explores the workspace freely.

## 0.39.41 — Pinned chats remember exactly where they were

- **Your pinned chat resumes the same conversation, every time.** The workspace's
  pinned chat now has one canonical, server-owned identity — so reopening it,
  switching devices, or restarting the host all return you to the *same* Claude
  session instead of occasionally starting a fresh one. This is the root fix
  behind the remote re-mint loop the last release patched.

## 0.39.40 — Clone-to chat history lands, and remote pinned chats settle down

- **Cloned workspaces keep their chat history on the new machine.** After a
  "Clone to", Claude's `/resume` came up empty on the destination because each
  session still pointed at the *source* machine's folder. Clones now rewrite
  those paths on arrival, and a one-time self-heal repairs workspaces you
  already cloned — your conversations show up where they belong.
- **Pinned chat no longer churns when viewed remotely.** Opening a workspace's
  pinned chat from a connected/companion client could spin in a loop, minting a
  brand-new session on every reconnect instead of resuming the real one. It now
  resumes the workspace's actual session and stays put.
- **Settings top-bar alignment.** "K2 ‹Server Name›" now sits flush in the
  Settings top-bar instead of dropping below the window's traffic lights.

## 0.39.39 — The server runs the show (steadier chats, less chatter)

- **Pinned chats are server-owned and steadier.** The daemon now owns the pinned
  chat's session end-to-end — opening, switching sessions, reloading, and
  reviving the right session after a restart are all handled server-side. No more
  open-flicker, and the tab keeps its icon.
- **Live updates instead of polling.** The app used to poll the server on timers
  for model status, agent activity, the review queue, and tunnel state. It now
  receives those as live pushes — fresher, lighter, correct across multiple
  devices, and it keeps working on a headless server.
- **Shared truth across everyone on a server.** Tab renames, tab order, the
  Active bar, and heartbeat "live" state now sync to every connected device — one
  consistent picture, not a per-window guess.
- **K2 Connect is now K2 Toge.** The remote-access feature was renamed (the old
  name belonged to another product). Settings and the website reflect it.
- **Settings shows the connected host up top.** The Settings page now carries the
  same "K2 ‹Server Name›" top-bar as the main view, with the host switcher there.
- **Small fix:** the Active-window up/down arrows in General settings are now
  visible instead of black-on-dark.

## 0.39.38 — Remote sessions stay alive, and Clone-to brings everything

- **Remote chat sessions no longer die after ~15 seconds.** When you opened a
  dormant workspace's chat from a connected client, the host could mistake it
  for a closed tab and reap the session out from under you. "Active" workspaces
  and the cleanup that acts on them now live on the server itself, so opening a
  workspace from *any* device keeps it alive — and the host (or a headless
  server) does the cleanup correctly on its own.
- **Everyone on a server sees the same Active workspaces.** When two people use
  one server, each sees the other's open workspaces appear in the Active bar —
  one shared, live picture of what's in use, mirrored to every connected device.
- **"Clone to" now migrates your *entire* chat history.** It used to bundle only
  the single newest session per workspace; it now brings every session by
  default. A new **"Include all chat history"** toggle (on by default) lets you
  opt back to live-only if you want a slimmer bundle.
- **Rename tabs.** Double-click a tab — or right-click → **Rename Tab** — to give
  it your own name.
- **Pinned-chat session picker, fixed and sturdier.** The dropdown reliably
  switches the pinned chat to a past session, the reload button reloads the one
  it names, and your chosen session is remembered across restarts (and reinstalls)
  so it comes back without re-picking — it's stored on the server now.
- **Brand-new workspaces open cleanly.** The pinned Chat + Inbox show up
  immediately (no more "leave and come back"), and a workspace's first chat starts
  fresh instead of failing on a not-yet-existent session.
- **Remote access stays connected.** A connected machine's tunnel keeps itself
  alive even if the Settings panel isn't open — the host renews its own lease, so
  remote access no longer drops out from under you.
- **Behind the scenes:** restored chat history always binds to the workspace that
  owns it (no wrong-history on lookalike workspaces); a safety rail prevents a
  misbehaving chat from respawning in a loop; plus test-suite reliability fixes.

## 0.39.37 — Settings layout polish for connected hosts

- **"Update Host" button.** Updating a connected machine is one click that
  downloads, installs, and relaunches it — so the button now says **"Update
  Host"** (it was "Download," which implied a separate install step that
  doesn't apply to app hosts).
- **General settings reads cleaner when connected to a host.** It now splits
  into two equal halves with a full-height divider — your general settings on
  the left, the connected host's Restart + Update controls on the right. On
  your own Mac it's a single half-width column with no divider.

## 0.39.36 — Reconnecting after a host restart just works

- **No more "invalid token" dead-ends after updating or restarting a host.**
  When a machine you're connected to restarts (e.g. right after a remote
  update), its sign-in can expire. K2 now checks your session the moment it
  reconnects and, if it's expired, **prompts you to sign back in** — instead of
  silently opening a broken workspace where the file tree, chat history, and
  terminals all fail with "invalid or missing auth token." One re-auth instead
  of having to remove and re-add the connection. (A momentary network blip
  never logs you out — only a genuinely expired session does.)

## 0.39.35 — Remote updates that actually work (both kinds of host)

- **Updating a machine you're connected to now works end-to-end.** A signing-
  manifest bug was silently breaking every remote daemon self-update; that's
  fixed. **Headless/server hosts** update via a verified binary swap;
  **desktop-app hosts** update by triggering that machine's own app updater —
  K2 now auto-detects which kind of host it is and picks the right path for you.
- **Update failures tell you why.** Instead of a generic "Update failed," the
  remote-update panel now shows the actual reason (download, signature, or
  version detail) so a stuck update is diagnosable at a glance.
- **Cleaner Settings when connected to a host.** The remote **Restart** and
  **Update** controls now sit together in their own right-hand column with a
  divider; when you're on your own Mac, the page is a single column as before.
- Under the hood: signed-download hardening (redirect handling + real logging),
  host-type reporting on the connection handshake, and a clear "open the app on
  that machine" message if a desktop host's app isn't running.

## 0.39.34 — Active bar that tells the truth (and uses less RAM)

- **"Active" now means *alive or recently worked*, not *what you're looking
  at*.** Workspaces you haven't touched in a while age out of Active on their
  own, and their background sessions get cleaned up — so K2 stops quietly
  holding hundreds of MB for workspaces you walked away from days ago.
- **Tune how long Active sticks around.** General settings has a new
  **"Keep workspaces Active for [N] hours"** — lower it for more aggressive
  cleanup, raise it to keep sessions warm longer.
- **At-a-glance status on every Active item:** a small **green square** when
  the workspace has a live session (grey when none), the **braille spinner**
  when it's working, and an **EKG icon** when it has an enabled heartbeat (i.e.
  it can run on its own). **Pinned** workspaces float to the top, separated
  from the rest.
- **The pinned Chat tab shows when it's working** — its icon turns into a
  spinner while the agent is busy, then back when it's done.
- **Heartbeat indicators are honest now.** A workspace only shows the heartbeat
  icon when it actually has an enabled heartbeat — fixed a case where a
  workspace with every heartbeat turned off still looked self-driving (and held
  its session open forever).
- **Squared-off status dots** in the server switcher, matching the rest of the
  UI. Plus K2 Connect settings polish and a reordered Settings list.

## 0.39.33 — Remote reboot + remote updates (beta)

- **Restart a machine you're connected to — from the app or the terminal.** A
  new **Restart host** control (Settings) appears only when you're on a remote
  host and is clearly labelled for *that* machine, not your Mac. From the CLI,
  `k2so daemon restart --host <url> --wait` does the same and waits for it to
  come back up. Owner/Admin only.
- **Update a remote machine over K2 Connect (beta).** On a remote host:
  check → download → verify → install & restart, with live progress and an
  automatic **rollback** if the new build doesn't come back. The download is
  **minisign-verified** before anything is swapped. The flow names the remote
  machine at every step so it can never be mistaken for your local one.
- **Install on a headless server from the CLI (beta).** `k2so daemon install`
  (and a `curl … | sh` one-liner) fetches, **verifies the signature**, and
  installs the standalone daemon, registering a systemd/launchd service so it
  stays up across restarts.

> Remote update and headless install are **beta**: the macOS path is wired end
> to end, while the Linux server binaries (built in CI) and the live
> download → swap → relaunch want a real-world shakeout. Signature verification
> is mandatory; all of it is Owner/Admin gated.

## 0.39.32 — Leaner memory, smoother relaunch

- **Closed a memory leak that piled up background agent processes.** Terminal
  and agent sessions are now force-reaped when their tab or workspace goes
  away (or when a remote host-switch tears them down) instead of orphaning a
  long-running `claude`/agent process (~150 MB each). If your machine felt
  heavier the longer K2 ran, this was why.
- **Dismissing a workspace from the Active bar now frees its sessions.** After
  a short grace period the dismissed workspace's pinned Chat (and any extra
  terminals) are reaped to reclaim memory; reopening the workspace relaunches
  the saved session right where you left off.
- **The workspace you land on at launch starts its Chat on its own.** Fixed a
  cold-start race where the first workspace's pinned Chat tab wouldn't spawn
  until you clicked refresh.
- **Connected Workspaces works on a remote machine** — the related-workspaces
  list now reads from the host you're connected to.
- **"Connection dropped" stays out of your way.** A brief tunnel blip now
  shows a small, non-blocking indicator instead of a full overlay — the top
  bar stays usable and the screen keeps updating; it only flags a real drop
  after repeated failures.
- **Clone-to cleans up after itself** — temporary transfer bundles are removed
  once a clone finishes, and stale ones are pruned, on both source and
  destination machines.

## 0.39.31 — K2 Connect: the whole remote surface is host-aware

- **What you do on a remote machine now actually happens on *that* machine.**
  A batch of actions were quietly running against your *local* machine even
  while you were connected to a remote — now they target the host you're
  connected to: approving / rejecting / requesting-changes on agent reviews,
  creating & deleting agents, editing heartbeats (add / edit / archive /
  enable / rename), the agent presence locks, scheduler ticks, managing
  skills, saving an agent's `AGENT.md`, regenerating the workspace skill,
  workspace connections, and more.
- **Format-on-save no longer misfires on a remote** — it skips rather than
  running a local formatter against a file that lives on the host.

## 0.39.30 — Fix: pinned-chat dropdown works on a remote machine

- **The pinned chat tab's chat-picker now switches chats on the machine
  you're connected to.** Selecting a different chat from the dropdown was
  updating only your *local* machine, so on a remote the chosen chat never
  loaded — it now writes to the active host, so it works the same remote as
  it does locally. (Working directly on the machine was already fine.)

## 0.39.29 — Clone to: the cloned workspace shows up + "Open on host"

- **The cloned workspace now appears on the host immediately** — no more
  manual window reload to see it. After a clone finishes, the destination's
  workspace list refreshes on its own.
- **"Open on \<host\>" button on the done screen** — jump straight into the
  freshly-cloned workspace on the remote machine, instead of hunting for it
  in the sidebar.

## 0.39.28 — Clone to: fix crash on workspaces with symlinked folders

- **Clone to** no longer fails with *"Is a directory"* on a workspace that
  contains a **symlink pointing at a folder** (for example, linked
  agent-skills under `.k2so/`). Those links are now skipped while bundling;
  symlinks to individual files are still copied. (0.39.27 introduced Clone
  to — this makes it work for those workspaces.)

## 0.39.27 — Clone a workspace to another machine + rock-solid remote tunnels

- **"Clone to" — move a whole workspace to a remote machine.** Right-click a
  workspace and pick **Clone to → <host>** to copy it onto a machine you're
  connected to over K2 Connect. It bundles the workspace — its files, the
  agent's memory, and session history — pushes it over your existing
  encrypted connection, unpacks it on the host, and registers it there with
  its K2 settings, ready to resume. A quick pre-flight lets you **decide
  whether to bring secrets** (`.env`, `.auth/`, in-workspace tokens): on by
  default since it travels over your encrypted link, or off if you'd rather
  re-add them on the host. (Your Claude login is never copied — the host
  signs in as itself.)
- **Remote tunnels now survive updates and restarts.** Fixed a bug where a
  K2 Connect host could go unreachable at `<you>.k2.dev` after a software
  update or daemon restart: the tunnel could pin a stale internal port, and
  leftover tunnel processes could pile up and fight over your subdomain. The
  host now always tracks its live port and clears out old tunnel processes
  on start, so remote access self-heals on the next launch.
- **CLI polish.** `k2so tunnel` and `k2so daemon companion` no longer print
  an error on their status output under newer Python versions.

## 0.39.26 — K2 Connect: drag files straight onto the remote machine

When you're connected to another machine, dragging a file in from your
computer now actually **transfers it to that machine**, decided by where
you drop it:

- **Onto a terminal** → the file uploads to the workspace's
  `.k2so/downloads/` and the path is dropped into the prompt, so the agent
  can use a file that really exists on the host.
- **Onto a folder in the file tree** → the file uploads into that folder.
- **Anywhere else** → you're asked where on the host to save it.

Local drag-and-drop is unchanged. (Both machines need 0.39.26 for the
host to accept the upload.)

## 0.39.25 — Remote folder picker everywhere + agent slash-commands

- **Open a remote folder from anywhere.** The 0.39.24 remote folder
  browser now backs **every** "add workspace" entry point — the main
  navbar **+**, the sidebar, the File menu, and ⌘O — not just Settings. So
  while you're connected to another machine, adding a workspace always
  browses **that** machine, never your local disk.
- **Agents can trigger slash-commands over messages.** `k2so msg` gains a
  `--command` flag that prepends a slash-command (like `/loop` or `/goal`)
  to the front of a delivered message — so one agent can kick off a
  command in another. Omitted, messages deliver exactly as before.

## 0.39.24 — K2 Connect: open a workspace on the remote machine

- **Open folders that live on the host.** When you're connected to another
  machine, "New Workspace" now lets you browse and pick a folder on **that
  machine** — an in-app folder browser that walks the remote's filesystem —
  instead of your local file picker (which could only see this computer).
- **Friendlier with out-of-date machines.** The app stays compatible with
  hosts running an older K2SO, so you can always connect and sign in to
  update one. And when a host is too old for a newer feature, the app now
  tells you which version it needs instead of silently doing nothing.

## 0.39.23 — K2 Connect: roles + cleaner remote settings

- **User roles for shared servers.** Connect users now have a role:
  **Owner**, **Admin**, or **Member**. The owner can promote trusted people
  to help run the server (including handing off ownership); admins can add
  users and enable/disable them; members just connect and use it. Removing
  users and changing roles stay owner-only.
- **Cleaner settings when viewing another machine.** The K2 Connect
  *tunneling* controls — k2.dev sign-in, subdomain, start/stop — now hide
  while you're connected to a remote host, since those belong to the machine
  that owns the daemon. Managing **that** server's users still works from
  right there.
- **`k2so` works from any folder.** Fixed a bug where running the `k2so`
  command (for example, an agent-to-agent message) from a directory that
  isn't a git repository would exit silently with no output.

## 0.39.22 — Onboarding fixes + remote settings clarity

- **Agents spawn out of the box.** The background daemon can now find
  `claude`/`cursor`/`gemini` even when they're installed in `~/.local/bin`
  (the native Claude installer), Homebrew, nvm, etc. — previously it only saw
  a bare system PATH and failed with "command not found".
- **No more stuck-on-Connecting after an update.** If K2SO was ever launched
  straight from the mounted disk image, the daemon could get pinned to that
  stale copy and never pair after upgrading. It now self-heals its path on the
  next launch, and warns if you run it from the DMG instead of /Applications.
- **Settings shows which server you're on.** While connected to another
  machine, the top of Settings now displays (and lets you switch) the active
  server.

## 0.39.21 — K2 Connect: the client fully mirrors the host

When you connect to another machine, the **whole** app now reflects that
host — workspaces, the active bar, pinned/active lists, whether focus
groups are on, panels, custom themes, timer entries, and settings — instead
of bleeding through your local machine's state. Your own client preferences
(terminal look, file-tree options, window layout) correctly stay yours.

## 0.39.20 — K2 Connect: remote clients can read the host's data

Fixes the bug where connecting to another machine showed *your* workspaces
instead of the host's. The host daemon was refusing a connected user's
session on every data read (workspaces, files, git), so the client silently
fell back to showing local data. Now a connected client sees the host's
workspaces, files, and git as intended. Update the **host** machine to 0.39.20.

## 0.39.19 — K2 Connect: driving a remote machine, done right

Connecting to another machine now makes the **whole** app follow it —
workspaces, files, git, agents, settings — reliably. This reworks the
0.39.18 approach from the inside: the app talks to the connected daemon
directly instead of proxying through this machine. That removes the
freeze-on-connect and fixes the bug where a failed connection could blank
your local workspace list until a reload.

## 0.39.18 — K2 Connect: actually drive the remote machine

When you connect to another machine, K2 now shows **that machine's**
workspaces, files, git, and agents — not your local ones. Previously the
connection succeeded but most panels kept showing this computer; now the
whole app follows the daemon you're connected to.

Also fixes the "Invalid or missing auth token" error on connect (the app
now waits for your sign-in before loading), and **bundles the tunnel client
(`frpc`) inside K2** — a fresh host machine can start a secure tunnel with
no manual install.

## 0.39.17 — K2 Connect sign-in fix

Fixes a "Load Failed" error when signing in to your k2.dev account (and
when connecting to a remote machine) in the packaged app. The production
build was blocking the secure connections K2 Connect needs; signing in,
loading your subdomains, and connecting to another machine over
`https://<you>.k2.dev` all work now.

## 0.39.16 — K2 Connect: reach your workspace from anywhere

K2SO can now expose your daemon at your own **`https://<you>.k2.dev`**
address, so you can reach this machine from another computer.

Sign in to your k2.dev account right in **Settings → K2 Connect**, pick a
subdomain you own, and hit **Start** — your machine goes live over a secure
tunnel. It can re-launch the tunnel automatically when the daemon restarts,
and if the same subdomain is already running on another device it's greyed
out so the two don't clash (swapping asks first).

Decide **who** can connect in: under **Users / Access** add people with a
username + an initial password (you set it once and can reset it, but never
see it again), choose your password rules (length, special characters), and
they manage their own password in a browser at your `k2.dev` address. To
connect *to* another machine, add it under **Connections** with its URL,
username, and password.

Settings → K2 Connect and Connections are now a single page, and the
K2 Companion page points you to the mobile app.

## 0.39.15 — No more phantom "audit bucket" projects in the sidebar

New users no longer see two confusing entries — **"Orphan audit bucket"**
and **"Broadcast audit bucket"** — in the workspace sidebar. Those are
internal bookkeeping items (they route the activity feed behind the
scenes); they were never meant to look like workspaces you created. They
now stay hidden from the project list while still doing their job
internally.

## 0.39.14 — Pinned Chat/Inbox tabs always point at the right workspace

Fixes a bug where a workspace's **pinned Chat and Inbox tabs** could stay
stuck pointing at a *different* workspace — wrong agent, wrong folder,
and (for Chat) the wrong conversation. New terminal tabs always opened
in the right place, but the pinned tabs kept routing to the other
workspace, and there was no way to fix it from the app.

It happened mainly to workspaces **created from inside another
workspace** (e.g. spinning up a new workspace from within an existing
one's chat) — the new workspace's pinned tabs picked up the parent's
context and held onto it.

Now K2SO **re-checks and corrects** a pinned tab's workspace every time
you switch into it, so any affected workspace **heals itself the next
time you open it** — no reinstall, no settings to touch. (When the
workspace is corrected, the pinned Chat tab also starts a fresh
conversation, since the old one belonged to the other workspace.)

A follow-on to 0.39.12's terminal-stall fix that removes the root cause
rather than just the symptom.

K2SO keeps every workspace's terminal session running in the background
(that work never pauses) — but until now the app also kept a **live
data stream open for every one of them**, even sessions you weren't
looking at. With many workspaces and a long-running app, that piled up
into a lot of redundant streaming, which was the underlying driver of
the terminal stalls.

Now the app **streams only the terminal pane that's actually on
screen.** When you switch tabs or workspaces, it stops streaming the
one you left and starts streaming the one you land on — instantly, with
no loss, because the session itself keeps running in the daemon the
whole time. Background sessions stay fully alive and keep working; the
app just doesn't waste resources rendering them when you can't see them.

The result: dramatically less background load, and terminals stay
responsive no matter how many workspaces you have open or how long
the app has been running.

Two fixes from user reports.

**Terminals no longer stall and "catch up."** If you run with more than
one terminal open — or just keep K2SO running for a while with several
workspaces — terminals could freeze for a few seconds and then suddenly
jump back to life, getting worse the longer the app was open. The cause
was an internal "who's the live view of this terminal?" signal that
every open pane was claiming at once, including hidden background tabs,
whenever the window had focus. With many sessions that turned into a
constant tug-of-war that flooded the terminal's live-update channel —
the flood is what you saw as the freeze, and the recovery is what you
saw as the sudden catch-up. Now only the **one terminal you're actually
looking at and typing in** claims that role, so the tug-of-war can't
happen no matter how many sessions are open or reconnecting.

**Chat history shows the workspace you're in.** Opening the chat-history
panel inside one workspace could show *another* workspace's chats — the
one that happened to be globally active (usually whichever has agents
running). The panel now binds to the workspace it's opened from, so you
always see that workspace's own history.

## 0.39.11 — Self-healing window: no more black screen after sleep or update

If K2SO ever opened to a **black, unresponsive window** — after an
update, or after your laptop slept and woke — this release makes it
recover on its own.

The root cause was the app's renderer occasionally not coming back to
life: the window's web layer would load but never start running, most
often right after an auto-update or when the Mac's app-rendering
process gets killed during sleep/wake. Until now the only fix was the
hidden right-click → Reload, which ordinary users would never think to
do — so the app just looked broken.

K2SO now watches its own window with a lightweight heartbeat. If the
interface stops responding, the app **automatically reloads it from
the native side** (the same thing the manual reload did) and brings
it back within a few seconds — no clicking required. It covers both
the after-update case and the after-sleep case, and it won't touch a
window that's working fine.

Also: the update button in Settings → General now reads **"Download"**
instead of "Download & Install" (the install happens when you click
the separate "Install & Relaunch" button).

## 0.39.10 — Read another agent's terminal + agent-setup fix

Three improvements for working with agents.

**`k2so read <workspace>` — look over another agent's shoulder.** The
read complement to the messaging verbs: `msg` talks live, `inbox` is
mail, and now `read` shows you the last N lines of another workspace's
live terminal. Great for human-in-the-loop — peek at what an agent is
doing or waiting on *before* you send it a message, or diagnose one
that's gone quiet:

```
k2so read <workspace>                 # last 50 lines of its session
k2so read <workspace> --lines 120     # more history
k2so read <workspace> --agent <name>  # a specific agent's session
```

**`msg` length limit is now documented.** Live `msg` is for short,
single-line messages — it's injected into the recipient's input line,
so long or multi-line text gets truncated. For anything substantial
(task briefs, file contents, multi-line notes) use the inbox, which has
no length limit: `k2so msg <workspace> --inbox --title "..." --body "..."`.
That length limit is the whole reason the inbox exists.

**Fixed: new agents are set up in the right place.** When you turned a
workspace into a Custom or K2SO agent, its persona file could get
scaffolded into a legacy `.k2so/agents/` folder instead of the canonical
`.k2so/agent/AGENT.md` — so an agent's documentation could land
somewhere the rest of K2SO wasn't looking. New agents now go to the
correct location, and any workspace already affected gets its agent
files moved back automatically on the next launch (your content is
preserved).

## 0.39.9 — Hotfix: exited terminals stay exited

Hotfix for a regression introduced in 0.39.8's reconnect logic. If a
terminal's child process exited (you closed a shell with `exit`, an
agent ran to completion, etc.) at exactly the moment the daemon's
WebSocket connection dropped, the new reconnect path would
incorrectly resurrect the exited terminal as a brand-new shell
session — visually confusing and wrong.

0.39.9 fixes that: an exited terminal stays exited, even if the
WebSocket teardown races the child-exit event. Normal mid-flight
WebSocket reconnects (the actual fix from 0.39.8) work exactly as
before.

If you didn't see any weird "my terminal that ran to completion came
back as a fresh shell" behavior on 0.39.8 — congrats, you weren't
hitting the race; just update and move on.

## 0.39.8 — Terminal panes recover from network blips + no more "frame stalls"

Two distinct long-running-session bugs fixed, both reported with
deep diagnostic profiles by external users. Combined with 0.39.7's
fd-exhaustion fix, multi-hour K2SO sessions should now stay smooth.

### Terminal panes survive network blips (was: silently frozen until quit)

Before: if a WebSocket between K2SO and its background daemon dropped
mid-flight — TCP reset, macOS App Nap, network blip — the terminal
pane went silently dead. Last frame stayed on screen, keystrokes
went nowhere, and the only fix was to quit and relaunch the app.
Cause: the WS `close` handler was a no-op, with no reconnect path.

Now: each terminal pane automatically reconnects within ~500 ms of
a drop (with a brief backoff for sustained outages). The PTY
session survives intact — your shell history, scrollback, and
running program continue. You'll see the pane go to "Connecting…"
briefly and then come back to life.

The session-events subscription (which keeps the workspace sidebar
in sync) got the same treatment: any error or close now triggers
an idempotent reconnect. Closes a gap where WebKit Networking
hiccups could leave that channel silently dead.

### Terminal output no longer "freezes then snaps"

Before: in long sessions, every terminal could intermittently freeze
for a beat then "catch up" all at once. The renderer was hot-looping
focus claims/releases to the daemon, eventually overrunning the
daemon's broadcast buffer by **thousands** of frames; recovery
required the daemon to flush a fresh full-grid snapshot. During the
overrun window all subscribers stopped seeing live updates.

Now: focus claims are deduplicated at the WebSocket-send level, so
the daemon only hears about real focus transitions (not React
re-render noise). In the common single-viewer case, the channel goes
completely silent except for legitimate user-driven focus changes
— and the broadcast buffer never overruns.

Thanks to the users who profiled both of these in production
(Issues #3 and #5) and submitted complete fix recommendations along
with their diagnoses.

## 0.39.7 — No more "K2SO slows down over the hour" lockups

Bug-fix release. If you ever ran K2SO for ~45 min to an hour and watched
it progressively slow down — file tree's `loading…` indicator lengthening,
terminals stalling for stretches, then everything "coming back to life
out of nowhere" — this is the release that ends it.

**What was happening:** every fetch the app made to its own background
daemon was a brand-new TCP connection, because the daemon forced
`Connection: close` on every response. macOS's web-renderer process
has a default cap of 256 sockets, and it cleans them up slowly. Over
~50 minutes of normal use the leftover sockets piled up against that
ceiling, and new requests had to wait for the kernel to time out old
sockets before they could go through. That's the "loading…" lengthening
and the freezes-then-recovery you saw.

**What's fixed:** the daemon now reuses one TCP connection for many
requests (standard HTTP keep-alive). Sockets recycle properly; the
~50-min wall is gone. A user reported the bug with a full live-CPU +
`lsof` + `sample` profile that nailed the root cause — credit to them
for the diagnosis.

Nothing for you to do — just smoother long sessions from here on.

## 0.39.6 — Terminal-stall storm fixed

Bug-fix release. If you ever saw every terminal session **lag or stall
for about 15 seconds at once** — usually with a lot of agent terminals
open — then "come back to life" on its own, this is the release that
ends it.

**What was happening:** the renderer's "Active agents" sidebar polled
every running terminal individually every 2.5 seconds, firing one
small HTTP request per terminal. On a box with many agent terminals
that meant a periodic flood of requests through the WebView's
networking stack — enough to spike renderer CPU to 80–128% and stall
every terminal at once until the storm cleared. The daemon was idle
throughout (a victim, not the cause).

**What's fixed:** the sidebar now makes **one request per poll**
instead of one per terminal, and stops re-rendering the active-agents
list when nothing has actually changed. Behaviour is identical — same
agents detected, same idle/active dots — just without the
request-storm side-effect.

Thanks to the user who profiled this in production and submitted the
fix.

## 0.39.5 — No more blank window after an update

Bug-fix release. If you ever updated K2SO and landed on a blank/black
window that only a right-click → Reload could fix, this is the release
that ends it — especially when updating from an older version that has
a lot of one-time setup to do on first launch.

**What was happening:** during an update, the app could briefly talk to
the *old* daemon that was on its way out, mount against it, and then get
stranded when the new daemon was still busy applying updates. No crash —
just a window that never finished loading.

**What's fixed:** the app now refuses to start against anything but the
daemon that ships with this exact build, and the daemon reports its
progress while it works. So instead of a blank window you'll see a
brief **"Setting up K2SO — applying updates…"** while first-boot
migrations run, then the app opens normally. On a big upgrade that
setup can take a few seconds; you'll see it happening rather than
staring at black.

Nothing for you to do — just smoother updates from here on, no matter
how old the version you're coming from.

## 0.39.4 — What's New popup: walk back to 0.39.0

Tiny UX fix to the "What's new" popup itself. Before: if you landed
mid-track on a 0.39.x patch (e.g. you updated 0.39.2 → 0.39.3 via
auto-update), the popup only showed entries newer than the version
you'd last dismissed — so you missed the foundational **0.39.0**
entry that explains why your workspaces sidebar got rearranged.

Now: while you're anywhere on the 0.39.x minor track, the popup
always carries **every 0.39.x entry** up through the version you
just installed.

**👈 Hit the ← arrow at the bottom-left of this popup to walk
back through 0.39.3, 0.39.2, 0.39.1, and 0.39.0.** The 0.39.0
entry is the one to read if you're wondering where your "Agents"
section went or why some workspaces are now pinned — it's the
release where that all changed, and it's only one ← away.

The same behaviour will hold for every future minor: land
anywhere on the X.Y.* track, walk back to read the whole story.

## 0.39.3 — ConnectionGate fix: no more black screen after update

Patch release. 0.39.2's ConnectionGate gated the *render* of the
app but still loaded the entire app's modules at startup. Several
stores fire daemon fetches the moment they're imported — if the
daemon was still kickstarting (the auto-update scenario), those
fetches failed and the stores got stuck in a broken state, leaving
the app rendering as a black window even after the gate dismissed.

0.39.3 defers loading the app's modules until the daemon is verified
healthy. App imports happen for the first time AFTER the gate sees a
green daemon — so every store's initial fetch hits a daemon that's
ready to respond. The black-screen-then-reload workaround is gone.

Bonus polish: the Reload button on the Connecting screen now appears
after 10 seconds (was 30), with friendlier copy explaining that the
daemon may still be loading and offering both "quit + relaunch" and
"reload" as recovery options.

## 0.39.2 — ConnectionGate: render after daemon healthy

Patch release. Fixes the "blank screen after update" race that some
users saw when 0.39.1 landed via auto-updater. A new ConnectionGate
component shows a small "Connecting…" overlay while it waits for the
daemon to be reachable, then mounts the app once it responds. No
more "right-click → Reload to make it work" on first launch after
update.

Bonus: this is the same primitive K2 Connect will use when
connecting to remote daemons (where transient unreachability is
normal). So the architecture lands now and pays dividends later.

## 0.39.1 — Manager-pin fix

Patch release. 0.39.0's one-shot migration over-pinned workspaces in
**manager mode** (manager / coordinator / pod) — the pre-0.39.0
sidebar only auto-promoted **K2SO Agent** and **Custom Agent**
workspaces, so manager-mode shouldn't have been pinned by the
migration. 0.39.1 ships a corrective one-shot migration that unpins
those manager-family workspaces on first launch.

**This only happens once.** After the corrective migration runs,
your pin choices are yours to keep — re-pin any manager workspaces
you want at the top (right-click → Pin) and they stay pinned across
all future versions.

## 0.39.0 — Clean foundation: new CLI, unified sidebar, chat/inbox everywhere

The first public release after a major behind-the-scenes refactor. K2SO
got a lot tidier — same product, cleaner bones. Things you'll notice:

- **Workspaces sidebar simplified.** The "Agents & Pinned" auto-promote
  behavior is gone — agent-mode workspaces no longer get a dedicated
  section forced above your manually pinned workspaces. **A one-time
  migration on first launch pins every workspace that was in agent
  mode** so nothing moves on you visibly: the workspaces that lived in
  the auto-promoted Agents section will still appear at the top of
  your Pinned list. If you don't want them pinned, right-click → Unpin
  any of them — they'll flow into the normal ungrouped / focus-group
  sections. Future workspaces you switch into agent mode won't
  auto-pin; you decide where they go. Same for the Workspaces Settings
  page where you organize what shows up in your nav.

- **Chat + Inbox tabs visible for every workspace** — even ones with
  agent mode set to "off". Every workspace is reachable via cross-
  workspace messaging (`k2so msg <workspace>`), so the inbox surface
  is always available now. Previously these tabs hid when agent mode
  was off, which made the receive side invisible.

- **New CLI** with 24 cleaner verbs across daily / power / internal
  tiers. Old verbs like `k2so delegate`, `k2so work create`, `k2so
  who`, `k2so roster` now print a helpful error pointing at their
  replacement (`k2so inbox compose`, `k2so connections list`, etc.).
  See `docs/changelog/release-notes-0.39.0.md` for the full deprecation map.

- **Storage shapes consolidated**: `.k2so/work/` → `.k2so/inbox/` and
  `.k2so/agents/<name>/` → `.k2so/skills/<harness>/`. The daemon
  migrates existing workspaces on first launch; no manual steps
  required. Your inbox items and skills survive — they just live in
  cleaner paths now.

- **Daemon-first foundation.** Most logic moved from the desktop shell
  into the daemon so the same code can power K2 Connect / K2 Companion
  (coming in 0.40.0). Mobile companion's pending-reviews badge, the
  desktop Review Queue UI, the heartbeat triage, and `cli` commands
  all share one source of truth now. **Bug fixes shipping with this**:
  `/cli/agentic` settings no longer 400s, review queue no longer
  silently shows 0, regenerated SKILL.md / CLAUDE.md / AGENT.md use
  the new CLI verbs, chat-history dedup for Pi / Codex / Cursor-IDE
  parsers, trash test infra hardened against macOS Touch ID flakes.

Plus a long list of internal cleanup — see `docs/changelog/release-notes-0.39.0.md`
for the developer-facing catalog.

## 0.38.13 — Faster launch + smarter memory threshold

Cleanup pass on 0.38.12's two big additions:

- **Launch speed.** The What's New popup's "is daemon ready?" retry
  loop used to block a Tauri worker thread for up to 5 seconds at
  app startup, contending with all the other launch-time work. Now
  the retry happens renderer-side via plain `setTimeout` (yields
  between attempts) and the popup's first check is deferred until
  2 seconds after the rest of the UI has painted.
- **Smarter memory warning.** The 800 MB threshold was firing
  immediately on app launch because the local LLM loads ~1+ GB of
  weights into the process address space. The watcher now captures
  a settled baseline at the second sample and warns only on
  **growth** above that (+800 MB) or a hard ceiling (3 GB). Either
  signals a real leak; LLM steady-state is silent.

## 0.38.12 — Memory watcher + quieter heartbeat audit log

Two improvements driven by an overnight crash report:

- **Renderer memory watcher.** K2SO now logs its own memory usage
  every 5 minutes (visible in the Web Inspector console as
  `[k2so/memory] rss=...MB`). If the app ever crosses 800 MB you'll
  see a toast suggesting a restart. Gives us telemetry to catch
  Tauri-side memory leaks before macOS reaps the app under pressure.
- **Heartbeats auto-disable when WAKEUP.md is missing.** Before:
  a deleted or unreadable WAKEUP.md caused the heartbeat to retry
  every tick, spamming the audit log with `failed to compose wake
  prompt`. Now: the heartbeat flips to disabled on the first miss,
  records a single `auto_disabled` audit entry, and stays quiet
  until you fix the file and re-enable it from Settings →
  Heartbeats.

## 0.38.11 — Split popup into auto-fire vs button-trigger

Small architecture fix to the "What's new" popup. The popup is now
explicitly two-purpose:

- **Auto popup on the main screen** — fires once on first launch
  after a K2SO update. Same as before.
- **Button popup in Settings** — only opens when you click
  **Read what's new** in Settings → General. Never auto-fires when
  you happen to open Settings between updates.

Same modal UI in both places; just cleaner separation under the hood.

## 0.38.10 — Heartbeats on freshly-flipped agent workspaces

Hotfix: if you flipped a workspace to Custom / Workspace Manager /
K2SO Agent and immediately tried to add a heartbeat, you'd hit
"No scheduleable agent found in this workspace." Cause: the
validation was looking at `.k2so/agent/AGENT.md` on disk to confirm
"this is an agent workspace," but a mode-flip writes the DB
declaration immediately while AGENT.md may not be written yet.

Now: heartbeat add/remove/rename trust `projects.agent_mode` — the
column that's the source of truth for "this workspace is configured
as an agent." If the mode is set, you can schedule heartbeats
without waiting for any specific file to appear on disk.

## 0.38.9 — "Read what's new" works while Settings is open

Tiny hotfix to 0.38.8's new Settings button. Before: clicking
**Read what's new** in Settings → General appeared to do nothing —
the popup only appeared after you closed Settings. Cause: the popup
component wasn't mounted in the Settings-open layout, so the
button's open-popup event had nowhere to land.

Now: the popup opens immediately on top of Settings, no need to
close anything first.

## 0.38.8 — Cmd+T tabs remember their conversations + popup fixes

Two follow-ups to 0.38.5 and 0.38.7:

- **Cmd+T `claude` tabs now resume their conversations** across daemon
  restarts (app updates, kickstart, crash). Before: tabs came back as
  fresh claude sessions. Now: they pick up exactly where you left off,
  same as pinned chat does.
- **"What's new" popup wasn't appearing** for some users after the
  0.38.7 update — the renderer was checking the daemon before
  credentials were written, missing the popup entirely. Fixed with a
  short retry window so it survives launch races.
- **New "Read what's new" button** in Settings → General (under the
  CLI version row) — reopen the popup anytime to re-read what changed
  in the current release.

## 0.38.7 — Update notes when K2SO updates

You're seeing this because K2SO now shows a small "what's new" popup
the first time you open the app after an update. It rolls up everything
you missed if you skipped a version or two — no more wondering what
changed.

- Friendly per-update highlights
- Catches you up across multiple versions if you skipped a few
- `k2so whatsnew` reprints them anytime from the terminal
- `k2so whatsnew --reset` makes the popup show again next launch
  (good for sharing with a teammate)

## 0.38.6 — Inter-agent messages just work

`k2so msg <workspace> "text"` now delivers reliably on the first try.
The "send it twice and pray" workaround that agents were using is no
longer needed.

- One canonical JSON response shape every call — no more guessing
  whether `injected_to_pty: true` actually meant delivered.
- When delivery fails, you get a specific `reason` and an actionable
  `hint` instead of a silent inbox fallback.
- Recipients see `[from <sender>]` prefixed on every message, so they
  always know who's talking.
- `--wake` is no longer needed — `msg` is always live. Use
  `k2so work send` when you actually want to queue something for later.
- `k2so msg --help` finally works.

## 0.38.5 — Cmd+T tabs survive app updates

Your terminal tabs (including pinned chat) keep their `claude` sessions
through app updates and daemon restarts.

Before: a tab opened with `claude` would become a plain shell after the
next K2SO update. Now it comes back as `claude` — same command, same
working directory, same args. Subsequent updates won't reset your tabs
back to a shell.

## 0.38.4 — Heartbeats panel polish

The Heartbeats settings panel now matches the rest of the app's theme.
Heartbeat list is sorted alphabetically (case-insensitive — workspaces
named `alakazam-labs-website` and `BIG-CRM` no longer cluster apart).
Cosmetic only; no behavior change.

## 0.38.3 — System-wide Heartbeats settings page

Added a right-hand panel to the Heartbeats settings showing every
heartbeat across every workspace with toggles for enable/disable,
pinned-chat opt-in, and edit-wakeup. Plus a third column for a running
audit log of every fire system-wide — so you can finally see at a
glance which heartbeats are firing and which are dark.

## 0.38.2 — Heartbeats finally fire reliably

If you had heartbeats configured but they hadn't been firing for a
while (sometimes weeks), 0.38.2 fixes it. We replaced our hand-rolled
scheduler with the well-tested `croner` crate. Heartbeats now recover
cleanly from any pause and fire on schedule.

## 0.38.0 — Daemon-authoritative tabs + multi-window sync

Terminal tabs, including the pinned chat, now persist correctly when
the Tauri app closes and reopens — the daemon owns the sessions, and
the renderer attaches to whatever's already running. Cross-window
state (heartbeats minimized, pinned chat refresh, etc.) syncs
automatically.
