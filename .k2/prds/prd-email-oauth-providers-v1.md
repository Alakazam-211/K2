# PRD Addendum — External Mail OAuth Providers (Gmail + Microsoft 365 / Exchange)

**Status:** Planned. Addendum to [`prd-email-server-v1.md`](./prd-email-server-v1.md). Build this AFTER the base mail system (hosted + Email Link + the unified access layer / `k2 hostmail` + `k2 mail` split) has passed its checks and Rosson has confirmed the CLI shape feels right.

**One-liner:** Let a user connect their **Gmail** or **Microsoft 365 / Exchange Online** account to Email Link using **OAuth 2.0 device-code flow** (no passwords, no embedded browser), so a workspace's agents can READ it and save reply DRAFTS — exactly like today's IMAP linked inboxes. **Nothing about the agent-facing CLI changes**; this is a new *auth + provider* path behind the seam the base system already built.

**Audience:** a junior developer. Every file/function this touches is named. Read the base PRD §17.5 (external-inbox provider seam) and this whole document before writing code.

---

## 1. Why this is small (the seam already exists)

The base system was built so a new provider is "a row value away." Concretely, all of this already exists on `feat/email-server`:

- **`mail_external_inboxes.kind`** — `CHECK (kind IN ('imap','jmap','gmail-api'))`, default `'imap'`. The OAuth kinds are already in the CHECK. The schema comment explicitly says *"nothing may assume `kind == 'imap'`."*
- **`crates/k2-daemon/src/mail/messages.rs`** — `enum MailBackend { LocalStalwart, ExternalImap, … }`, the `ReadBackend` trait (`list_inbox`, read, `wait`, draft…), and `backend_for_address(address) -> MailBackend`. A new backend is a new enum variant + trait impl; the routes/CLI/UI never learn it exists.
- **`crates/k2-daemon/src/mail/external_imap.rs`** — the IMAP backend. **Auth is isolated in `login(inbox, password)`** (line ~80). Every read/draft op calls `login(...)` then does IMAP work. This is the single place OAuth changes anything for the IMAP path.
- **`crates/k2-daemon/src/mail/secrets.rs`** — the vault. `SecretStore::store_exact(key, secret)`, deterministic key `ext-inbox-<row-id>`, `resolve_secret_ref` for `env:` / abs-path / `mailsec_*` forms. Tokens live here; **no credential ever becomes a DB column or a log line.**
- **CLI**: `k2 mail link add|remove` (provisioning), `k2 mail messages|read|wait|attachments|draft` (use). All source-agnostic already.

**Therefore the work is: (a) the device-code OAuth engine, (b) token storage + refresh, (c) an XOAUTH2 auth path in the existing IMAP backend, (d) provider presets, (e) a "Connect" UI that shows a code + link.** The mail-reading/drafting logic is untouched.

---

## 2. Goal & non-goals

**Goal:** `k2 mail link add rosson@gmail.com --provider gmail` (and `--provider microsoft`) → device-code flow → connected inbox that behaves identically to an IMAP-linked inbox for read + draft.

**Non-goals (BINDING):**
- **No sending over OAuth scopes.** ~~No sending from external accounts. Ever.~~ **AMENDED 2026-07-11:** the base system now DOES support sending from a LINKED inbox — over **SMTP submission** with the account's own app-password, opt-in at the `send` access level, ungated for now (`external_smtp.rs`, `prd-email-server-v1.md` §17.5 amendment). That path is unaffected here. THIS non-goal narrows to: the **OAuth** providers still do NOT request a send scope (`gmail.send` / `Mail.Send`) in Phase 1 — an XOAUTH2-authenticated SMTP send is a later decision. OAuth inboxes stay read + draft-to-Drafts.
- **No embedded webview for auth.** Google returns `disallowed_useragent` inside webviews; Microsoft discourages it; webviews break passkeys and the user's existing session. K2's browser pane is a WKWebView — do NOT use it for the OAuth handshake. (It stays fine for DNS-help / docs.)
- **No client secret embedded in the app.** Device flow uses a **public client**; there is no secret to leak.
- **Graph / Gmail REST API backend is Phase 2 (§12), optional.** Phase 1 is IMAP-XOAUTH2 for both providers — it reuses the whole existing IMAP backend.
- No changes to hosted (Stalwart) mail, DNS, doctor, or the `k2 mail` agent surface.

---

## 3. Decision: OAuth 2.0 **device-code flow** (RFC 8628)

Chosen over the system-browser+loopback-redirect flow (RFC 8252) as the **default** because:

1. **It works identically for local AND remote/headless daemons.** K2 daemons commonly run on a remote Linux box (the whole hosting story). A loopback `127.0.0.1` redirect lands on the *daemon's* machine, not the user's browser machine — broken for remote. Device flow has no redirect: the daemon polls, the user types a short code into any browser, anywhere.
2. **No embedded browser, no local port juggling, no redirect-URI registration headaches.**
3. Both Google and Microsoft support it for installed/public clients.

The flow (RFC 8628):
```
1. Daemon → POST device-authorization endpoint (client_id, scope)
   ← { device_code, user_code, verification_url, expires_in, interval }
2. Daemon shows the user: "Go to <verification_url> and enter <user_code>"
   (CLI prints it; the Settings page renders it with a copy button.)
3. Daemon polls token endpoint (grant_type=urn:ietf:params:oauth:grant-type:device_code,
   device_code, client_id) every `interval` seconds:
     - authorization_pending → keep polling
     - slow_down → increase interval by 5s
     - access_denied / expired_token → fail with a clear message
     - success → { access_token, refresh_token, expires_in, token_type }
4. Daemon stores tokens (vault), verifies an IMAP XOAUTH2 login succeeds,
   marks the inbox connected.
```

> **System-browser + loopback (RFC 8252) is an OPTIONAL later fast-path** for known-local daemons only. Not in Phase 1.

---

## 4. Data model

Add to `mail_external_inboxes` via a new migration (next free number **after** the base system's tip; verify with `crates/k2-core/src/db/mod.rs` — do NOT hardcode; renumber-past-main discipline from the base PRD applies):

```sql
-- prd-email-oauth-providers: OAuth device-flow linked inboxes.
ALTER TABLE mail_external_inboxes ADD COLUMN auth_kind TEXT NOT NULL DEFAULT 'password'
    CHECK (auth_kind IN ('password','oauth'));   -- 'password' = today's app-password path
ALTER TABLE mail_external_inboxes ADD COLUMN provider TEXT;                -- 'gmail' | 'microsoft' | NULL(generic imap)
ALTER TABLE mail_external_inboxes ADD COLUMN token_expires_at INTEGER;     -- unix secs; NULL for password auth
-- host/port/tls/username/kind/drafts_folder stay as-is.
```

- **Tokens** (access + refresh) are vaulted, NOT columns. Reuse the deterministic key with a suffix so they don't collide with a password secret: `ext-inbox-<row-id>-oauth` holding a JSON `{ "access_token":…, "refresh_token":…, "scope":…, "token_type":"Bearer" }`. `token_expires_at` in the row is the only non-secret bit (so refresh can be decided without unvaulting).
- `kind` stays `'imap'` for the XOAUTH2 path (it's still IMAP transport). `provider` records which OAuth vendor so refresh/endpoints/preset are chosen. (`gmail-api` / `jmap` kinds are for the Phase 2 REST backend.)
- A generic (non-OAuth) IMAP inbox keeps `auth_kind='password'`, `provider=NULL` — the existing path, untouched.

---

## 5. Auth: XOAUTH2 in the existing IMAP backend

**This is the entire "provider backend" for Phase 1** — no new backend, just a new auth path in `external_imap.rs`.

- Add an auth branch to `login(inbox, …)`: if `inbox.auth_kind == 'oauth'`, obtain a **fresh access token** (§6) and authenticate the IMAP session with **SASL XOAUTH2** instead of `LOGIN`. The `imap` crate exposes `Session::authenticate("XOAUTH2", &authenticator)` where the authenticator yields the SASL string:
  ```
  base64( "user=" + <email> + ^A + "auth=Bearer " + <access_token> + ^A + ^A )      # ^A = 0x01
  ```
  Everything after auth (`survey_folders`, list, fetch, APPEND-to-Drafts) is the EXISTING code, unchanged.
- Keep the password path exactly as-is for `auth_kind='password'`.
- **⚠ LIVE-BOX** (per base-PRD convention): the exact XOAUTH2 error surface (Google returns a base64 JSON error blob on a `+` continuation; the `imap` crate's XOAUTH2 handling) is the one genuinely-uncertain bit — isolate it in one function with a `⚠ LIVE-BOX` doc comment and list it in the module header, resolved against a real account.

---

## 6. Token lifecycle

- **Refresh** (`crates/k2-daemon/src/mail/oauth/mod.rs`, new): `access_token_for(inbox) -> String`. If `now >= token_expires_at - 60s`, POST the token endpoint with `grant_type=refresh_token` + the vaulted `refresh_token` + `client_id`, store the new access token + `token_expires_at` (Google keeps the same refresh_token; **Microsoft ROTATES the refresh_token** — store the new one when present). Called by `login()` before XOAUTH2.
- **Revoke on unlink**: `k2 mail link remove` for an oauth inbox best-effort-revokes the token at the provider's revocation endpoint, then deletes the vault entry + row (base system already cascades grants).
- **Never log** access/refresh tokens or the SASL string. Redact in every error path.

---

## 7. Routes + CLI

**CLI (`cli/k2`, `mail link` family):**
```
k2 mail link add <address> --provider gmail|microsoft         # OAuth device flow
k2 mail link add <address> --host … --tls … [--pass-stdin]    # existing generic IMAP (unchanged)
```
For an OAuth provider, `link add`:
1. POSTs `/cli/mail/link/oauth/start {address, provider, workspace}` → daemon runs step 1 of §3, returns `{ userCode, verificationUrl, expiresIn, linkId }`.
2. CLI prints: **"Open `<verificationUrl>` and enter code `<userCode>` (expires in N min). Waiting…"** and then **long-polls** `/cli/mail/link/oauth/status?linkId=…` (reuse the `wait` long-poll idiom: bounded, injectable clock in tests) until `connected` / `denied` / `expired`. Same block-until-done UX as `k2 mail wait`.
3. On success prints the connected inbox summary. Exit codes 0/1/2/3 as elsewhere.

**Routes** (new, in a `mail/routes_link_oauth.rs`; owner/Primary-gated like `link add`):
- `POST /cli/mail/link/oauth/start` → begins device flow, persists a pending link + `device_code` (vaulted), spawns/records the poll state, returns the user-facing code/url.
- `GET /cli/mail/link/oauth/status` → `{state: pending|connected|denied|expired, …}`. The daemon does the actual token POLLING server-side on a bounded task (device_code has `expires_in`, usually ~15 min); status just reports. Runs under the dispatcher's `spawn_blocking` arm (blocking reqwest), like the other mail routes.
- The token exchange + IMAP verify happen daemon-side; the CLI/UI only ever see the code + a state.

No change to `messages/read/wait/attachments/draft` — they already resolve through `backend_for_address`.

---

## 8. UI (Settings → Email Link)

Add per-provider **"Connect Gmail"** / **"Connect Microsoft"** buttons to the Add-inbox area. On click:
1. POST `link/oauth/start` → render a card: **the `user_code` (big, copy-button) + a "Open `<verification_url>`" button that opens the SYSTEM browser** (the existing `k2-open` / Tauri shell-open path — NOT the browser pane) + a live "Waiting for you to approve…" spinner.
2. Poll `link/oauth/status` (or subscribe to the mail hook events) until connected; then it appears in the inbox list with `source=linked`, ready for the shared `InboxAccessPanel` (Primary + grants).
3. Never render tokens. The generic-IMAP add form stays for advanced/self-hosted IMAP.
Cross-platform (no Linux gate — Email Link is universal).

---

## 9. Provider specifics

### Google (Gmail / Google Workspace)
- **Endpoints:** device auth `https://oauth2.googleapis.com/device/code`; token `https://oauth2.googleapis.com/token`; revoke `https://oauth2.googleapis.com/revoke`.
- **Scope (Phase 1, IMAP):** `https://mail.google.com/` (full IMAP access — required for IMAP; there is no read-only IMAP scope). Servers: `imap.gmail.com:993` implicit TLS. Drafts folder: `[Gmail]/Drafts` (autodetect already handles this).
- **Client:** a Google Cloud project + OAuth consent screen, "TV and Limited Input" (device) client type. `https://mail.google.com/` is a **restricted scope** → Google requires app **verification + a CASA security assessment** before unrestricted public use. **This is calendar time (weeks), not code** — start it early. Until verified, it works for the project's own test users (fine for dev + Rosson).
- App passwords still work as the generic-IMAP fallback where a user prefers them.

### Microsoft 365 / Exchange Online
- **Basic Auth (password IMAP) is OFF** for M365 tenants — OAuth is the only path.
- **Endpoints:** device auth `https://login.microsoftonline.com/common/oauth2/v2.0/devicecode`; token `https://login.microsoftonline.com/common/oauth2/v2.0/token`. Use `common` tenant unless a single-tenant deployment.
- **Scope (Phase 1, IMAP):** `https://outlook.office.com/IMAP.AccessAsUser.All offline_access` (`offline_access` is REQUIRED to get a refresh_token). Servers: `outlook.office365.com:993` implicit TLS. XOAUTH2 same shape.
- **Client:** an Azure AD app registration (public client, "allow public client flows" = yes). Admin consent may be required in some tenants.
- **Refresh tokens ROTATE** — always persist the new `refresh_token` from each refresh response.
- On-prem legacy Exchange may still allow password IMAP → the generic-IMAP path covers it; don't special-case.

---

## 10. Security (BINDING)

- Public client, **no embedded secret**; device flow only in Phase 1.
- Request the **narrowest scopes** that permit read + draft-append (the IMAP scope is coarse by necessity; Phase 2 Graph can use `Mail.Read` + `Mail.ReadWrite` which is finer — a Phase-2 argument).
- **Never** request send scopes. **Never** log tokens/SASL/bodies.
- Tokens vaulted (`ext-inbox-<id>-oauth`), 0600 vault, revoked + wiped on unlink.
- Masked `not_found` ownership (base system) unchanged — OAuth doesn't touch the access layer.
- Clock: decide refresh off `token_expires_at` with a 60s safety margin; handle clock skew defensively (a 401 on IMAP auth → force one refresh + retry, then fail).

---

## 11. Pre-mortem — what bites a junior dev here

1. **Reaching for the embedded webview.** It's *right there* in K2 and it feels natural. Google will reject it. Use the system browser + device code. (This is the #1 wrong turn; §2/§3.)
2. **Embedding a client secret.** Device/public clients don't need one; committing one leaks it. Config the `client_id` only.
3. **Google verification timeline.** `https://mail.google.com/` is restricted → weeks of review + a security assessment for public distribution. Start it on day 1; develop against test users meanwhile. Don't discover this at launch.
4. **Microsoft basic auth assumption.** Do NOT build a "just IMAP password" M365 path — it's dead. OAuth or nothing for M365.
5. **Dropping the rotated MS refresh token.** Microsoft returns a NEW refresh_token on refresh; if you keep the old one you're logged out in ~24h–90d. Always persist the latest.
6. **`offline_access` omitted (MS)** → no refresh_token → the inbox dies at the first token expiry (~1h). Include it.
7. **Device-code expiry + `slow_down`.** The `device_code` expires (~15 min) — surface a clean "code expired, try again." Honor `interval` and the `slow_down` back-off, or the provider rate-limits/blocks you.
8. **Polling on the wrong side.** Poll the token endpoint SERVER-SIDE (daemon), not in the CLI/UI; the CLI/UI only long-poll the daemon's `status`. Keeps tokens off the client and works for remote daemons.
9. **XOAUTH2 error handling.** Auth failure comes back as a base64-JSON blob on an IMAP continuation, not a normal `NO`. Parse it or you'll mis-report "connected." (The `⚠ LIVE-BOX` in §5.)
10. **Gmail Drafts folder name** is `[Gmail]/Drafts`, All-Mail/labels aren't folders — the existing autodetect + fallback chain already covers this; don't hardcode `Drafts`.
11. **Assuming `kind=='imap'`.** The base schema forbids it; keep provider/backend selection data-driven so the Phase-2 REST backend slots in.
12. **Token in logs / error strings.** Redact aggressively; a token in a debug line is a full account compromise.
13. **Testing with a real account against the real provider in CI.** Don't. Mock the token endpoint + a loopback IMAP mock (the base system's test patterns); only a manual, gated live-box pass touches a real Google/MS account. No real sends, ever.

---

## 12. Phase 2 (OPTIONAL, later) — REST backends

For tenants that disable even IMAP-OAuth, or for finer scopes/perf, add a REST `ReadBackend`:
- **Gmail API** (`kind='gmail-api'`): a new `MailBackend::GmailApi` variant + `ReadBackend` impl over `gmail.readonly` + `gmail.compose` (drafts). Finer scopes than full-IMAP.
- **Microsoft Graph** (`kind='jmap'` or add `'graph'` to the CHECK): `MailBackend::Graph` over `Mail.Read` + `Mail.ReadWrite`.
Same device-flow auth + vault; the difference is the transport behind the seam. Not needed for the initial ship — IMAP-XOAUTH2 covers the vast majority.

---

## 13. Slice plan

- **O1 — Auth foundation:** migration (auth_kind/provider/token_expires_at), vault token JSON, `oauth/mod.rs` device-flow client (start/poll/refresh/revoke) with a mocked token endpoint + full unit tests. No IMAP yet.
- **O2 — Routes + CLI:** `link/oauth/start` + `status` routes (server-side poll task), `k2 mail link add --provider …` long-poll UX, exit codes, `--schema`/help.
- **O3 — XOAUTH2 in the IMAP backend:** the `login()` auth branch + SASL XOAUTH2 + refresh-before-connect; `⚠ LIVE-BOX` isolated. Gmail + MS presets (host/port/tls/scope/endpoints in one table). This is where read/draft start working end to end.
- **O4 — UI:** Connect-Gmail/Microsoft buttons, code+link card, system-browser open, poll-to-connected.
- **O5 — Docs:** `k2 mail link` help + the agent skills note (provider linking is a human setup action; agents still just `k2 mail`), glossary `mail-link-oauth`.
- **O6 (optional) — Graph/Gmail-API backend** (§12).
- **Live-box acceptance:** one manual pass each against a real Gmail test account and a real M365 test account — connect, read, draft-into-Drafts, refresh across an expiry, unlink+revoke. Never send.

---

## 14. Acceptance criteria

- [ ] `k2 mail link add me@gmail.com --provider gmail` prints a code + URL, and after approval the inbox is connected and appears in `k2 mail inboxes` as `source=linked`.
- [ ] Same for `--provider microsoft` against an M365 account.
- [ ] `k2 mail messages/read/wait/attachments/draft` work on the OAuth inbox with **zero** code paths specific to them outside `login()` + the oauth module.
- [ ] Access token auto-refreshes across an expiry with no user action; MS refresh-token rotation persisted.
- [ ] No OAuth **send scope** requested (`gmail.send`/`Mail.Send`) in Phase 1. (Note: the base system's SMTP linked-send path — app-password, opt-in `send` level — is separate and unaffected; see the §2 amendment.)
- [ ] No embedded webview used; system browser only; no client secret in the repo/bundle.
- [ ] Tokens never logged; vaulted; revoked + wiped on unlink.
- [ ] Tests pass with mocked token endpoint + loopback IMAP mock; no real provider calls in CI.

## 15. Open questions for Rosson (decide before O1)

1. **Whose OAuth app?** One K2-owned Google/Azure client (users consent to "K2"), vs. letting advanced users supply their own `client_id`. K2-owned is the good UX but carries the Google verification burden. (Recommend: K2-owned, plus an optional BYO-client for enterprises.)
2. **Publish/verify now or dev-only first?** Verification is weeks; we can ship to test users while it's in review.
3. **Graph backend (Phase 2) — worth it,** or is IMAP-XOAUTH2 enough for the foreseeable customer base? (Recommend: defer until a real tenant blocks IMAP-OAuth.)
