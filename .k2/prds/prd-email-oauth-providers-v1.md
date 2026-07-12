# PRD Addendum — External Mail OAuth Providers (Gmail via IMAP + Microsoft 365 / Exchange via Graph)

**Status:** Planned for **0.40.42** (Rosson 2026-07-11). Addendum to [`prd-email-server-v1.md`](./prd-email-server-v1.md). Build after the base mail system (hosted + Email Link + unified access + `k2 hostmail`/`k2 mail`) is confirmed.

**One-liner:** Let a user connect their **Gmail** (via IMAP-XOAUTH2) or **Microsoft 365 / Exchange Online** (via the **Microsoft Graph API**) account to Email Link with OAuth — no passwords, no embedded browser — so a workspace's agents READ it and save reply DRAFTS, exactly like today's IMAP linked inboxes. **The agent-facing `k2 mail` CLI never changes**; both providers plug in behind the existing `ReadBackend`/`backend_for_address` seam.

**Audience:** a junior developer. Read the base PRD §17.5 (provider seam) and this whole document before writing code.

---

## 0. The provider/transport decision (READ FIRST — this is the load-bearing choice)

We build **each provider on the transport its own vendor supports long-term**, not a single lowest-common path. Rationale: minimize maintenance-under-fire over years (Rosson's explicit priority).

| Provider | Transport | Auth flow | Why |
|---|---|---|---|
| **Gmail / Google Workspace** | **IMAP-XOAUTH2** (reuses the existing IMAP backend) | **Authorization-code + system-browser + loopback** (RFC 8252) | Gmail's IMAP is first-class & stable — Google isn't deprecating it. Reuses the IMAP backend we keep anyway for generic accounts. **Gmail's IMAP scope is NOT device-flow-eligible** (§11.1), so device flow is impossible here. |
| **Microsoft 365 / Exchange Online** | **Microsoft Graph REST API** (new backend) | **Device-code flow** (RFC 8628) | Microsoft treats IMAP as **legacy** (basic-auth IMAP already killed; OAuth-IMAP at risk). Graph is the API Microsoft invests in. Graph mail scopes ARE device-flow-eligible → uniform local/remote, no loopback. Clean REST is easier to maintain than IMAP's quirks. |
| **Generic / self-hosted IMAP** | IMAP (existing) | app-password (today) or XOAUTH2 if the host supports it | unchanged; the fallback for everything else. |

**We deliberately do NOT build the Gmail REST API** (Gmail's IMAP is fine — no vendor pressure to leave it) and **do NOT depend on Microsoft's legacy IMAP.** Net: two provider paths (IMAP-XOAUTH2 for Gmail, Graph for Microsoft) + the untouched generic-IMAP path — **all three behind the same `k2 mail` CLI.**

---

## 1. Why this is contained (the seam already exists)

- **`mail_external_inboxes.kind`** — `CHECK (kind IN ('imap','jmap','gmail-api'))`; **add `'graph'`** in this work. Nothing may assume `kind == 'imap'`.
- **`crates/k2-daemon/src/mail/messages.rs`** — `enum MailBackend { LocalStalwart, ExternalImap, … }`, the `ReadBackend` trait (`list_inbox`, read, `wait`, draft, and the wave-3 manage ops), and `backend_for_address(address) -> MailBackend`. **Microsoft Graph is a new `MailBackend::Graph` variant + trait impls; the routes/CLI/UI never learn it exists.**
- **`crates/k2-daemon/src/mail/external_imap.rs`** — the IMAP backend. Auth isolated in `login(inbox, password)`. Gmail-XOAUTH2 is a new auth branch here — everything after auth (folders/list/fetch/APPEND/move/flag) is the EXISTING code.
- **`secrets.rs`** — vault (`ext-inbox-<row-id>`); tokens live here as a suffixed key, never a DB column or log line.
- **CLI**: `k2 mail link add|remove` + `k2 mail messages|read|wait|attachments|draft|move|flag|archive|delete|folder`. Source-agnostic; Graph must satisfy the same read/draft/manage trait surface so these keep working.

The work: (a) an OAuth engine supporting **both** device-code and auth-code+loopback, (b) token storage + refresh, (c) Gmail XOAUTH2 auth branch in the IMAP backend, (d) a **Graph `ReadBackend`/manage backend** for Microsoft, (e) provider presets, (f) a "Connect" UI showing a code/link. Reading/drafting/managing logic is otherwise reused.

---

## 2. Goal & non-goals

**Goal:** `k2 mail link add me@gmail.com --provider gmail` and `k2 mail link add me@company.com --provider microsoft` → OAuth → a connected inbox that behaves identically to any linked inbox for read/draft/manage.

**Non-goals (BINDING):**
- **No OAuth send scope in Phase 1.** OAuth inboxes stay read + draft-to-Drafts + manage. (The base system's SMTP linked-send — app-password, opt-in `send` level, ungated — is separate and unaffected; `external_smtp.rs`, base PRD §17.5 amendment.) OAuth send (Gmail `gmail.send` / Graph `Mail.Send` / SMTP-XOAUTH2) is a later decision.
- **No embedded webview for auth.** Google returns `disallowed_useragent` in webviews; passkeys/session break. K2's browser pane (WKWebView) must NOT do the OAuth handshake. Use the **system browser** (`k2-open` / Tauri shell-open).
- **No client secret embedded in the app.** Public clients: Gmail's installed-app (loopback) client and Microsoft's device-flow public client both work **without** a secret.
- **No Gmail REST API** (§0 — Gmail IMAP is sufficient) and **no reliance on Microsoft IMAP** (§0 — legacy).
- No changes to hosted (Stalwart) mail, DNS, doctor, or the `k2 mail` agent surface.

---

## 3. The two OAuth flows (the engine supports both; provider selects)

`crates/k2-daemon/src/mail/oauth/mod.rs` (new) exposes provider-agnostic `obtain_tokens(provider)` and `refresh(provider, refresh_token)`, with two flow implementations:

### 3a. Device-code flow (RFC 8628) — **Microsoft**
```
1. POST device-authorization endpoint (client_id, scope) → { device_code, user_code, verification_uri, expires_in, interval }
2. Show the user: "Go to <verification_uri> and enter <user_code>"  (CLI prints; UI renders a copy-button card)
3. Poll the token endpoint (grant_type=…device_code, device_code, client_id) every `interval`s:
     authorization_pending → keep polling ; slow_down → +5s ; access_denied/expired_token → fail clearly ; success → tokens
4. Vault tokens, verify a Graph call succeeds, mark connected.
```
Works identically for **local and remote daemons** — no redirect.

### 3b. Authorization-code + loopback (RFC 8252) — **Gmail**
```
1. Daemon binds a throwaway http://127.0.0.1:<random-port>/cb listener + builds the consent URL
   (client_id, scope=https://mail.google.com/, redirect_uri=that loopback, access_type=offline, prompt=consent, state).
2. Daemon opens the user's SYSTEM browser to the consent URL (Tauri shell-open / k2-open).
3. User approves in their real browser → Google redirects to the loopback with ?code=… → the listener captures it.
4. Daemon exchanges code→tokens at the token endpoint, vaults them, verifies an IMAP XOAUTH2 login, marks connected.
```
**V1 scope: LOCAL daemon only** (daemon == the machine running the browser, so it can bind the loopback). **Remote-daemon Gmail** (the K2 app captures the loopback and relays the code to the daemon over the existing connection) is an explicit **follow-up**, not V1 — surface a clear "Gmail linking must be done on the machine running this K2 daemon" message when remote. Microsoft (device flow) has no such limit.

> Why not device flow for Gmail: Google's device authorization grant only permits a limited scope set; `https://mail.google.com/` is **not** among them (§11.1). Every OAuth IMAP client for Gmail uses this loopback flow.

---

## 4. Data model

Migration (next free after the base tip — verify `crates/k2-core/src/db/mod.rs`; renumber-past-main discipline applies):
```sql
-- prd-email-oauth-providers: OAuth-linked inboxes (Gmail IMAP-XOAUTH2 + Microsoft Graph).
ALTER TABLE mail_external_inboxes ADD COLUMN auth_kind TEXT NOT NULL DEFAULT 'password'
    CHECK (auth_kind IN ('password','oauth'));
ALTER TABLE mail_external_inboxes ADD COLUMN provider TEXT;            -- 'gmail' | 'microsoft' | NULL(generic imap)
ALTER TABLE mail_external_inboxes ADD COLUMN token_expires_at INTEGER; -- unix secs; NULL for password auth
-- also: add 'graph' to the `kind` CHECK (table-rebuild migration, or a documented widen) — Microsoft rows are kind='graph'.
```
- **Tokens** vaulted, not columns: key `ext-inbox-<row-id>-oauth`, JSON `{access_token, refresh_token, scope, token_type}`. `token_expires_at` is the only non-secret bit (so refresh is decidable without unvaulting).
- **Gmail** rows: `kind='imap'`, `auth_kind='oauth'`, `provider='gmail'`, host `imap.gmail.com:993`. **Microsoft** rows: `kind='graph'`, `auth_kind='oauth'`, `provider='microsoft'`, host/tls unused (Graph is HTTPS to `graph.microsoft.com`).
- Generic IMAP: `auth_kind='password'`, `provider=NULL`, unchanged.

---

## 5. The two backends

### 5a. Gmail — XOAUTH2 branch in the existing IMAP backend (`external_imap.rs`)
Add to `login(inbox, …)`: if `auth_kind=='oauth'`, get a fresh access token (§6) and authenticate via **SASL XOAUTH2** instead of LOGIN:
```
base64( "user=" + <email> + ^A + "auth=Bearer " + <access_token> + ^A + ^A )   # ^A = 0x01
```
Everything after auth (survey_folders, list, fetch, APPEND-to-Drafts, MOVE/STORE for wave-3 manage) is the EXISTING code. Password path unchanged. **⚠ LIVE-BOX:** the XOAUTH2 error surface (Google returns a base64-JSON error on a `+` continuation) — one flagged function.

### 5b. Microsoft — a new Graph `ReadBackend` (+ manage) (`crates/k2-daemon/src/mail/graph.rs`, new)
`MailBackend::Graph`; implement the SAME trait surface the seam requires so `messages/read/wait/attachments/draft/move/flag/archive/delete/folder` all work:
- **List** newest-first: `GET /me/mailFolders/{folder}/messages?$orderby=receivedDateTime desc&$top=N&$skip=M&$select=…` (map `--folder`/`--junk`/`--since`/`--before`/`--offset` to `$filter`/`$search`/`$top`/`$skip`). Default folder = Inbox; folders resolve by well-known name (`inbox`,`junkemail`,`archive`,`deleteditems`,`drafts`) or displayName.
- **Read** one: `GET /me/messages/{id}` (body, headers) → the same shaped output; opaque id `m_<b64url(address\ngraph:<id>)>` (parallel to the IMAP/JMAP id schemes; the seam already uses opaque ids).
- **Draft**: `POST /me/messages/{id}/createReply` then `PATCH` the body (or `POST /me/messages` with `isDraft`) → lands in the account's Drafts, same "user reviews + sends" model.
- **Manage (wave-3)**: move = `POST /me/messages/{id}/move {destinationId}`; flag/read = `PATCH {isRead, flag}`; folder create/rename = `POST/PATCH /me/mailFolders`; **delete = move to `deleteditems` (Trash), NEVER `DELETE`** (mirror the wave-3 no-permanent-delete rule).
- **wait**: reuse the poll-loop; Graph list is the poll.
- Pagination via `$skip`/`@odata.nextLink`. **⚠ LIVE-BOX:** Graph message-id opacity, `$search` quirks, throttling (429 + Retry-After) — flagged.
Auth = Bearer access token (§6) on every request. NO IMAP for Microsoft.

---

## 6. Token lifecycle (both providers)

`oauth/mod.rs::access_token_for(inbox) -> String`: if `now >= token_expires_at - 60s`, POST the token endpoint `grant_type=refresh_token` + vaulted `refresh_token` + `client_id`; store the new access token + `token_expires_at`. **Google keeps the same refresh_token; Microsoft ROTATES it — persist the new one when present.** Called before every XOAUTH2 login (Gmail) / before Graph calls (Microsoft). **Revoke on unlink** (best-effort to the provider revoke endpoint) then wipe the vault entry + row. **Never log** tokens/SASL/bodies; redact everywhere.

---

## 7. Routes + CLI

**CLI:** `k2 mail link add <address> --provider gmail|microsoft` (OAuth) alongside the existing generic-IMAP `--host …`. For an OAuth provider, `link add`:
1. POST `/cli/mail/link/oauth/start {address, provider, workspace}` → daemon begins the provider's flow; returns `{ flow: 'device'|'loopback', userCode?, verificationUrl?, linkId, expiresIn }` (device → code+url; loopback → the daemon has already opened the browser, returns "waiting").
2. CLI prints the instruction (MS: "enter <userCode> at <url>"; Gmail: "a browser window opened — approve there") and **long-polls** `/cli/mail/link/oauth/status?linkId=…` (the `wait` idiom) until `connected|denied|expired`.
3. On success prints the connected inbox. Exit 0/1/2/3.

**Routes** (`mail/routes_link_oauth.rs`, owner/Primary-gated; the `link/*` aliases from the base fix apply):
- `POST /cli/mail/link/oauth/start` → begins the flow; for Gmail binds the loopback + opens the browser + records pending state; for Microsoft starts device auth + records the poll task. Returns the user-facing bits. **Server-side** poll/exchange under the dispatcher `spawn_blocking` arm.
- `GET /cli/mail/link/oauth/status` → `{state: pending|connected|denied|expired, …}`.
Tokens/codes never reach the CLI/UI. `messages/read/wait/attachments/draft/manage` are UNCHANGED (they resolve through `backend_for_address`).

---

## 8. UI (Settings → Email Link)

**"Connect Gmail"** / **"Connect Microsoft"** buttons. On click → POST `link/oauth/start`:
- **Microsoft**: render the `user_code` (big, copy) + an "Open <url>" button (system browser) + a "waiting for approval" spinner; poll status → connected.
- **Gmail**: the daemon already opened the system browser; show "Approve in the browser window that opened…" + spinner; poll status. If the daemon is **remote**, show the "do this on the daemon's machine" message instead (V1 limitation, §3b).
Never render tokens. The generic-IMAP add form stays for advanced/self-hosted. Cross-platform (Email Link is not Linux-gated). Reuse the mail hook events to flip to connected live.

---

## 9. Provider specifics

### Google (Gmail / Workspace) — IMAP-XOAUTH2, auth-code+loopback
- **Endpoints:** auth `https://accounts.google.com/o/oauth2/v2/auth`; token `https://oauth2.googleapis.com/token`; revoke `https://oauth2.googleapis.com/revoke`.
- **Scope:** `https://mail.google.com/` (full IMAP; no read-only IMAP scope exists). `access_type=offline` + `prompt=consent` to get a refresh_token. Server `imap.gmail.com:993` implicit TLS; Drafts `[Gmail]/Drafts` (autodetect handles it).
- **Client:** Google Cloud project + OAuth consent screen, **"Desktop app" (installed-app / loopback) client type** (NOT "TV/Limited-Input" device client — that can't grant this scope). `https://mail.google.com/` is a **restricted scope** → **app verification + CASA security assessment** required for public use (weeks; start early). Works for the project's test users meanwhile.
- App passwords remain a generic-IMAP fallback.

### Microsoft 365 / Exchange Online — Graph, device-code flow
- **Endpoints:** device auth `https://login.microsoftonline.com/common/oauth2/v2.0/devicecode`; token `…/common/oauth2/v2.0/token`. Graph base `https://graph.microsoft.com/v1.0`.
- **Scope:** `https://graph.microsoft.com/Mail.ReadWrite offline_access` (ReadWrite = read + create drafts + move/flag; `offline_access` REQUIRED for a refresh_token). (`Mail.Read` alone is read-only — insufficient for draft/manage.)
- **Client:** Azure AD app registration, **public client** ("allow public client flows" = yes), device-code grant enabled. Admin consent may be required in some tenants. Multi-tenant → `common`.
- **Refresh tokens ROTATE** — persist the latest every refresh.
- On-prem legacy Exchange with basic-auth IMAP → the generic-IMAP path covers it; don't special-case.

---

## 10. Security (BINDING)
- Public clients, **no embedded secret** (Gmail loopback client + MS device client both work without one).
- **Narrowest workable scopes** (Gmail IMAP is coarse by necessity; Graph `Mail.ReadWrite` is already fairly narrow). **No send scope** in Phase 1. **Never** log tokens/SASL/bodies.
- Tokens vaulted (`ext-inbox-<id>-oauth`), 0600, revoked + wiped on unlink.
- Loopback (Gmail): bind `127.0.0.1` on an **ephemeral** port, validate `state`, single-use, short timeout; the listener returns a tiny "you can close this tab" page and nothing else.
- Masked `not_found` ownership (base system) unchanged.
- Refresh off `token_expires_at` − 60s; on a 401 (IMAP or Graph) force one refresh + retry, then fail.

---

## 11. Pre-mortem — what bites a junior dev

### 11.1 THE big one: Gmail can't use device flow
Google's device authorization grant only permits a restricted scope set; **`https://mail.google.com/` is not device-flow-eligible.** Gmail MUST use auth-code + loopback (§3b). Do not copy the Microsoft device-flow path for Gmail — it will fail at Google with a scope/invalid-request error. (The O0 spike VERIFIES this empirically per provider before building the rest.)

### 11.2 The rest
1. **Embedded webview** — Google rejects it (`disallowed_useragent`). System browser only.
2. **Client secret in the bundle** — public clients need none; never commit one.
3. **Google verification timeline** — restricted scope = weeks of review + CASA. Start day 1; dev on test users. Don't discover at launch.
4. **Microsoft basic-auth IMAP assumption** — dead; and we're on Graph anyway, not IMAP, for MS.
5. **Dropping the rotated MS refresh token** — persist the latest or you're logged out in 24h–90d.
6. **`offline_access` / `access_type=offline` omitted** → no refresh_token → dies at first expiry (~1h). Include it (both providers).
7. **Device-code expiry + `slow_down`** (MS) — surface "code expired, retry"; honor `interval`/back-off or get rate-limited.
8. **Loopback pitfalls** (Gmail) — ephemeral port, validate `state`, single-use, timeout; and **remote daemons can't loopback** → show the "link on the daemon's machine" message, don't hang.
9. **Poll server-side** — the daemon polls/exchanges; CLI/UI only long-poll the daemon `status`. Keeps tokens off the client, works for remote (MS).
10. **XOAUTH2 error shape** (Gmail) — base64-JSON on an IMAP continuation, not a normal `NO`. Parse it or mis-report "connected." (⚠ LIVE-BOX §5a.)
11. **Graph throttling** — 429 + `Retry-After`; honor it or get blocked. (⚠ LIVE-BOX §5b.)
12. **Graph delete = move to Deleted Items, NEVER `DELETE`** — same no-permanent-delete guarantee as wave-3 IMAP.
13. **`kind` widen for `'graph'`** — SQLite CHECK change needs a table rebuild (or a documented approach); Microsoft rows won't be `'imap'`.
14. **Assuming `kind=='imap'`** anywhere — the seam forbids it; Graph is a peer backend.
15. **Tokens in logs** — a token in one debug line = full account compromise. Redact hard.
16. **Real provider calls in CI** — don't. Mock the token endpoint, a loopback IMAP mock (Gmail), and a Graph HTTP mock (Microsoft). Only a manual gated live pass touches a real account. No real sends ever.

---

## 12. Phase 2 (OPTIONAL, later)
- **Gmail REST API** — only if Google ever pressures IMAP (they don't today). Deferred.
- **OAuth send** — Gmail `gmail.send` / Graph `Mail.Send` / SMTP-XOAUTH2, gated by the future unified email-review layer + send gates (base PRD roadmap item 2).

---

## 13. Slice plan (0.40.42)
- **O0 — Provider/flow spike (DO FIRST; needs Rosson to register clients):** register a Google **Desktop** client + an Azure device-flow client; EMPIRICALLY confirm: Gmail device flow REJECTS `https://mail.google.com/` (→ loopback), Gmail loopback GRANTS it; MS device flow GRANTS `Mail.ReadWrite`. Lock the flow-per-provider table. Cheap insurance against building the wrong flow. **The O1–O5 CODE can be built in parallel against MOCKS; O0 gates only the live acceptance.**
- **O1 — OAuth engine + tokens:** migration (auth_kind/provider/token_expires_at + `graph` kind), vault token JSON, `oauth/mod.rs` with BOTH flows (device + loopback) + refresh/revoke, mocked token endpoint + full unit tests. No provider I/O yet.
- **O2 — Gmail path:** XOAUTH2 branch in `external_imap.rs` + loopback route; end-to-end read/draft/manage on a Gmail test account. ⚠ LIVE-BOX isolated.
- **O3 — Microsoft Graph backend:** `graph.rs` `MailBackend::Graph` (list/read/draft/wait/attachments + manage/move/flag/delete-to-DeletedItems/folder) + device-flow route; end-to-end on an M365 test account.
- **O4 — Routes + CLI:** `link/oauth/start`+`status`, `k2 mail link add --provider …`, exit codes, `--schema`/help.
- **O5 — UI:** Connect-Gmail/Microsoft buttons, code/link card (MS) + "approve in browser" (Gmail) + remote-daemon-Gmail message, poll-to-connected.
- **O6 — Docs:** `k2 mail link` help, agent-skills note (provider linking is human setup; agents still just `k2 mail`), glossary `mail-link-oauth`.
- **Live acceptance:** one manual pass each — Gmail (loopback) + M365 (device+Graph): connect, read, draft-into-Drafts, move/flag/delete-to-Trash, refresh across an expiry, unlink+revoke. Never send.

---

## 14. Acceptance criteria (0.40.42)
- [ ] `k2 mail link add me@gmail.com --provider gmail` opens the system browser, and after approval the inbox connects (`source=linked`, `provider=gmail`, `kind=imap`).
- [ ] `k2 mail link add me@company.com --provider microsoft` prints a device code + URL, and after approval connects (`provider=microsoft`, `kind=graph`).
- [ ] `k2 mail messages/read/wait/attachments/draft/move/flag/archive/delete/folder` work on BOTH with zero code outside `login()`/`oauth`/`graph.rs`.
- [ ] Tokens auto-refresh across an expiry; MS rotation persisted.
- [ ] Gmail delete + Graph delete are BOTH move-to-Trash — no permanent-delete path.
- [ ] No send scope requested; the app-password SMTP linked-send stays separate/unaffected.
- [ ] No embedded webview; system browser only; no client secret in repo/bundle.
- [ ] Remote-daemon Gmail shows a clear "link on the daemon's machine" message (no hang); MS works local + remote.
- [ ] Tokens never logged; vaulted; revoked + wiped on unlink.
- [ ] CI passes with mocked token endpoint + loopback IMAP mock + Graph HTTP mock; no real provider calls.

## 15. Open questions for Rosson (decide before O1)
1. **Whose OAuth apps?** One K2-owned Google Desktop client + Azure app (users consent to "K2") vs. BYO-`client_id` for enterprises. (Recommend: K2-owned + optional BYO.) **Blocks live testing — someone must register these.**
2. **Ship 0.40.42 with OAuth code but Gmail live-gated on Google verification** (weeks)? MS could go live as soon as the Azure app is registered; Gmail waits on Google's review. (Recommend: ship the capability; Gmail activates when verified.)
3. **Remote-daemon Gmail** (app-relays-the-loopback) — V1 local-only acceptable, or is remote-Gmail needed at launch? (Recommend: local-only V1; MS covers remote via device flow.)
