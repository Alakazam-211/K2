# Mini-PRD — `k2 mail draft` compose + folder-aware fetch

**Date:** 2026-08-25 · **Owner:** Rosson · **Status:** Ready for implement  
**Product:** `k2` CLI + daemon linked IMAP/XOAUTH2 (Gmail)  
**Incident:** agent cannot put a **new** To/Subject into the human's Gmail Drafts; `list --folder "[Gmail]/All Mail"` then `draft`/`read` lies `"the source message is no longer on the server"`  
**Related:** parent `prd-email-server-v1.md` §17.5 (reply drafts only), `prd-email-oauth-providers-v1.md`, wiki `Feature - Email System` (stale “never send from linked”)

**Amends** parent §17.5: `k2 mail draft` stays the review-first verb, but it is no longer reply-only and no longer Inbox-only. A draft is still **not** a send.

---

## 1. Problem

`k2 mail draft <id> --body` is the only verb that APPENDs `\Draft` into a human's real Drafts folder. Two holes:

1. **Compose does not exist.** The verb always threads onto `<id>` and inherits To/Subject. `k2 mail send` has `--to`/`--subject`/`--cc`/`--attach` but actually sends (or 403s / queues in the K2 outbox). Never APPEND Drafts. For “agent drafts, human edits in Gmail,” there is no destination.
2. **Fetch is Inbox-only.** `list --folder "[Gmail]/All Mail"` encodes that mailbox's UIDVALIDITY as `uid:11:…`. `fetch_raw` / `read` / `draft` always `SELECT INBOX` (`external_imap.rs` ~579, `select_inbox_for_uid` ~700). Validity mismatch → `ExtError::NotFound("the source message is no longer on the server")`. The same daemon just listed it.

---

## 2. Locked product

| Want | Today |
|---|---|
| Reply sitting in Gmail Drafts | `k2 mail draft <id> --body` — fetch always SELECT INBOX |
| Brand-new draft | **does not exist**. `send` actually sends (or 403s). Never APPEND Drafts |
| Send | `k2 mail send` / `reply` if send-level **AND** workspace Sending=`on` |

`k2 mail draft` is the **right** verb for a reply into the human's Gmail Drafts. It is the **wrong** (missing) verb for a new To/Subject. Linked Gmail is IMAP/XOAUTH2.

Same access as today: `can_draft`. **Not** behind `mailAgentSend` (a draft is not a send). Linked inboxes only (hosted still uses `k2 mail reply` / approval outbox).

---

## 3. Decisions

| # | Decision |
|---|----------|
| **D1** | **Compose-draft.** `k2 mail draft --to <addr> --subject <s> [--cc] [--attach] (--body\|--body-file) [--from <linked-addr>]`. No `<message-id>`. IMAP APPEND `\Draft` (daemon already appends on the reply path). `--to` + `--subject` required. `--from` picks the linked inbox (`can_draft`); one draftable linked inbox may be implicit; N → require `--from` (do **not** reuse send's hosted-only implicit resolver). Reply form stays `k2 mail draft <id> --body…`. Mutually exclusive: id XOR `--to`/`--subject`. |
| **D2** | **Folder-aware fetch.** `fetch_raw` / `read` / `draft` SELECT the folder whose UIDVALIDITY matches the token (LIST survey, then SELECT). **Prefer actually fetching** the listed folder over a token-format change. `mark_seen` and `select_inbox_for_uid` (move/flag/archive/delete) share the same Inbox SELECT — fold them through the same helper so the Inbox-only hole does not remain on manage. Until the SELECT works, fail with an **honest** hint (`this id is not Inbox — listed from another folder; re-list Inbox or wait for folder-aware fetch`), never “no longer on the server.” v1 does **not** rewrite `uid:<validity>:<uid>`. |
| **D3** | **`--attach` on draft**, matching `send` (caps, workspace-relative paths, daemon reads bytes): reply-draft **and** compose-draft. IMAP only this slice (reuse `routes_send` attach parse + `compose_*_rfc822` multipart). |
| **D4** | **Skill/docs.** `.k2/skills` is generated from `crates/k2-core/src/skills/content.rs` (three Email blocks). Teach: `draft` = Gmail Drafts; reply needs a listed id (Inbox **or** folder-aware); compose uses `--to`/`--subject`; sending from linked Gmail **is** possible when gated `on`. Delete remaining “cannot send from external / sending is impossible” copy (`cli/k2` link-add print ~20151, catalog JSON, `cmd_help_mail_draft`, wiki `Feature - Email System`). Bump skill versions on implement (this PRD does not). |

**Out of v1:** Graph compose-draft (Graph has no IMAP APPEND — Microsoft is `createReply` today; note it, do not fake APPEND). Hosted K2-mailbox drafts in Gmail. Putting compose behind `mailAgentSend`. Graph send. Graph draft attachments. Encoding folder into the uid token (follow-up if UIDVALIDITY collisions show up).

---

## 4. Implementation sketch

### 4.1 CLI — `cli/k2` `cmd_mail_draft` (~21097) + `_mail_py` `verb == "draft"` (~20112)

Reply path unchanged except `--attach` (and optional `--cc` is compose-only). Compose path: require `--to` and `--subject`, no positional id. Usage error if both id and `--to` (or `--subject` without `--to`). Mirror `cmd_mail_send` attach/cc/from parsing. POST `/cli/mail/draft` body grows `to` / `subject` / `cc` / `from` / `attachments` (camelCase, same as send).

Help + catalog (`mail draft` usage/description ~11546) name both forms.

### 4.2 Daemon — `routes_external.rs` `handle_draft`

`DraftBody`: `id` optional. Compose: missing id + to/subject. Reply: id, no to/subject. Hosted id still teaches `k2 mail reply`. Gate **only** `access::can_draft` — do not call `require_linked_send_gate` / `mailAgentSend`. Graph + compose → teaching error (out of v1: createReply is reply-only; no APPEND). IMAP: `save_reply_draft` or new `save_compose_draft`.

### 4.3 IMAP — `external.rs` + `external_imap.rs`

- Replace hard-coded `SELECT INBOX` in `fetch_raw` / `mark_seen` / `select_inbox_for_uid` with **SELECT by token UIDVALIDITY** (LIST names, SELECT until `mailbox.uid_validity` matches, then UID FETCH/STORE/MOVE). Inbox tokens (`uid:1:…`) still work. All Mail (`uid:11:…`) works. No match → honest hint naming Inbox vs listed folder, **not** “no longer on the server.”
- `save_compose_draft`: From = linked account, To/Subject/Cc from args, **no** In-Reply-To/References, APPEND `\Draft` to resolved Drafts folder (`pick_drafts_folder` / pinned `--drafts-folder`). Health stamp same as reply.
- Attachments: multipart/mixed on both compose and reply RFC822. Caps = send (`MAX_ATTACHMENTS` 10 / 25 MiB).

Fakes: `FakeOps` must record the SELECTed folder (or accept tokens from non-Inbox validity) so tests do not pretend Inbox.

### 4.4 Skills — `skills/content.rs` Email blocks (manager / k2so-agent / custom)

```
k2 mail draft <message-id> --body <t> [--attach]
k2 mail draft --to <addr> --subject <s> --body <t> [--cc] [--attach] [--from <linked>]
```

One sentence: draft lands in the human's Gmail Drafts; `send`/`reply` from linked Gmail need level `send` **and** Sending=`on`. Microsoft-OAuth stays draft-only until Graph send.

---

## 5. Tests (loud)

| Case | Expect |
|---|---|
| CLI: `draft --to a@b --subject s --body t` (no id) | POST has to/subject/body, no id (**bash** — `cmd_mail_draft` is not cargo) |
| CLI: `draft <id> --body t` | existing reply POST (id + body) |
| CLI: id **and** `--to` | usage, exit 2 |
| CLI: compose missing `--subject` or `--body` | usage, exit 2 |
| Route: compose, `can_draft` ok, `mailAgentSend=off` | 200 APPEND, **not** 403 gated |
| Route: hosted id | existing teach `k2 mail reply` |
| Route: Graph + `--to`/`--subject` | usage/engine teaching (no fake APPEND) |
| IMAP: `fetch_raw` token `uid:<All Mail validity>:<uid>` | SELECT that folder, Some(raw) — **not** NotFound |
| IMAP: Inbox token `uid:1:…` | still fetches |
| IMAP: validity matches no LIST folder | honest hint, **not** “no longer on the server” |
| Compose APPEND | To/Subject set, `\Draft`, no In-Reply-To; folder = Drafts |
| Reply APPEND | still threads (In-Reply-To / Re: subject) |
| `--attach` reply + compose | multipart; over-cap = usage (same numbers as send) |
| Skill generators | compose flags present; **no** “cannot send from external” / “sending is impossible” |

No `unwrap_or` in assertions. No skip-if-missing. IMAP tests stay on fakes / loopback mock — no live Gmail in CI.

---

## 6. Success

1. Agent: `k2 mail draft --to someone@x --subject "Hello" --body "…"` → draft appears in the linked Gmail Drafts with that To/Subject; human opens Gmail, edits, sends. Workspace Sending may be `off`.
2. `k2 mail messages <addr> --folder "[Gmail]/All Mail"` then `read` / `draft <that-id>` works (or, until SELECT lands, an honest “not Inbox” hint — not “no longer on the server”).
3. `k2 mail draft <inbox-id> --body` still threads a reply into Drafts; `--attach` works on both forms.
4. `k2 mail send` unchanged: still gated; still does not APPEND Drafts.
5. Hosted addresses still cannot use `draft` (teach `reply`). Graph compose-draft is an explicit out-of-v1 teaching error.
6. Regenerated skills tell agents: draft = Gmail Drafts; compose = `--to`/`--subject`; linked send exists when gated `on`.

---

## 7. Wiki / index

When accepted: this file; wiki `Feature - Mail Draft Compose` (point at this PRD); `_Index`; `roadmap-prds-feedback-polish.md` Open B. Parent §17.5 stays historical; this file is the compose/folder-aware SSOT.
