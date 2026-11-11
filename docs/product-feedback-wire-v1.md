# Send feedback to K2: what your server sends (wire contract v1)

K2 0.46.11 adds **Send feedback to K2**: the button next to Help in the top
bar, "Send Feedback…" in the menus, and `k2 send-feedback` for agents. This
page is the exact contract between your K2 server and Alakazam Labs, so you
can check what leaves your machine. The code is `crates/k2-core/src/product_feedback/`
(`wire.rs` builds and reads every request below) and
`crates/k2-daemon/src/product_feedback_*.rs`.

This is not `k2 feedback` / `k2 tickets`, which asks the people on your own
server and never leaves it.

## The path

1. The desktop dialog or the CLI calls **your server's daemon**
   (`/cli/product-feedback/*`). A window connected to a remote server sends
   through that server, and the dialog says so ("Sent through …").
2. The daemon scrubs the text (values from your vault become `[k2v:NAME]`),
   stores it in its **outbox** (SQLite, `product_feedback_outbox`) and answers
   at once. Nothing is lost while offline.
3. The daemon asks the K2 Connect relay for a **box token** and posts the
   submission to the **intake**. It retries on its own until the intake has
   it.

Nothing leaves while air-gap is on, while the owner turned sending off, or on
a server that isn't paired with K2 Connect.

## 1. Box token

```
POST https://connect.k2.dev/api/identity/assert
Authorization: Bearer <this server's K2 Connect tunnel token>
X-K2-Subdomain: <this server's label>
Content-Type: application/json

{"aud": "https://feedback.akzm.k2.dev"}
```

Answer: `200 {"token": "<ES256 JWT>", "expiresAt": <unix seconds>}`. The JWT
names this server's label, the audience and a hash of the account, and lives
5 minutes. The tunnel token itself is sent only to K2 Connect (which already
holds it), never to the intake. The daemon keeps the box token in memory
until 30 seconds before it expires.

## 2. Submission

```
POST https://feedback.akzm.k2.dev/v1/submissions
Authorization: K2-Box <the box token>
Idempotency-Key: <clientId>
Content-Type: application/json
```

| Field | Value |
|---|---|
| `clientId` | `pf_` + 22 base62, made on your server; a resend uses the same one |
| `category` | `bug`, `idea`, `praise` or `other` |
| `text` | 1 to 20,000 bytes as typed, with vault values replaced |
| `contactEmail` | only when a person ticked "Alakazam may email me about this" |
| `senderKind` | `person` or `agent` |
| `client` | `desktop`, `web` or `cli` |
| `k2Version`, `daemonVersion`, `installKind`, `os`, `osVersion`, `arch`, `locale` | versions and platform |
| `diagnostics` | only with consent: `{"sections": [{"id", "title", "text"}]}`, at most 256 KB |
| `screenshot` | only when a person attached one and saw it: see below |

Nothing else: no names, logins, workspace or agent names, files, terminal
output, chats, mail, tickets, environment variables, tokens or IP addresses.
The one exception is a screenshot a person attaches.

### Screenshot (a person only)

A person may attach one picture in the desktop or web dialog: K2's window
as K2 draws it, or an image they chose, with their marks and solid covers
flattened into it. Agents and `k2 send-feedback` can't attach one.

1. The dialog uploads the flattened PNG to **your server's daemon**
   (`POST /cli/product-feedback/screenshot`, `{image, source, capture?}`,
   base64, at most 2 MiB as a PNG and a 3 MiB body). The daemon checks it
   (a PNG, each side at most 4096 px, not animated, decodes under limits),
   cuts every chunk except `IHDR`, `PLTE`, `tRNS`, `IDAT`, `IEND`, `sRGB`,
   `gAMA` and `pHYs` (so text, EXIF and time chunks go; the pixels don't
   change) and keeps it in memory for an hour as a draft.
2. The dialog shows the person **the daemon's own copy**. Send names that
   draft and the sha256 of those bytes; any other hash is refused (`409
   screenshot_changed`). So a picture leaves only after the person saw
   exactly those bytes.
3. The daemon stores it with the outbox row (`product_feedback_screenshots`)
   and sends it in the same submission:

```json
"screenshot": {
  "mime": "image/png",
  "source": "window",
  "capture": "page",
  "width": 2560,
  "height": 1600,
  "sha256": "<64 lowercase hex: sha256 of the PNG bytes>",
  "data": "<the PNG, standard base64 with padding>"
}
```

| Field | Value |
|---|---|
| `mime` | always `image/png` |
| `source` | `window` (K2's window) or `file` (an image the person chose) |
| `capture` | a `window` picture only: `page` (drawn from the page; terminals, web frames, 3D views and video are labelled blanks) or `webview` (the window's own webview snapshot) |
| `width`, `height` | the PNG's size, 1 to 4096 each |
| `sha256`, `data` | the PNG exactly as the person saw it |

With a screenshot the whole submission is at most 3 MiB and an attempt
waits up to 90 seconds (20 seconds without). Every retry sends the same
bytes under the same `clientId`. The vault scrub works on text only: the
picture goes as the person saw it, and covering anything private is theirs
to do before they send.

### Diagnostics (consent only)

Collected only when a person ticks **Include details about K2 on this
computer** (or answers yes to `k2 send-feedback --diagnostics`), or for an
agent while the owner or an admin turned on agent diagnostics (off by
default). Sections: versions, computer (OS, CPUs, memory, free disk),
K2 state (names of settings that are on, counts, tunnel up/down, air-gap),
installed AI tools and their versions, failure-looking log lines from the
last 24 hours (repeats folded) and the end of K2's log. Each is scrubbed on
your server: vault values, bearer values and PEM blocks, secret-looking flags
and `KEY=value`, URL passwords and query values, K2 credentials, JWTs and
vendor keys, emails, IP addresses, the home folder, and every workspace, App,
agent and peer name (as `<workspace-1>` …). The person reads every section
first, can leave any out or delete lines, and the sections sent are exactly
the ones shown (the send names the preview; the daemon never collects
again).

### Answers

| Answer | What your server does |
|---|---|
| `201 {"id"}` or `200 {"id", "duplicate": true}` | **sent** |
| `401 {"code"}` | fetches a new box token once, then retries later |
| `429 {"retryAfterSecs", "scope"}`, `503 {"code": "intake_paused", "retryAfterSecs"}` | waits that long, then retries |
| `404`, `405`, `502`, `503`, `504`, another `5xx`, no answer | keeps it **queued** ("isn't reachable yet") and retries: 30 s, doubling, up to every 2 hours, ±20 % |
| another `4xx` (`400`, `403 box_blocked`, `413`, …) | **refused**, never retried |

After 7 days of retrying a submission is **failed**; Retry (the Sent tab,
`k2 send-feedback retry`) puts it back. Redirects are never followed.

## Kept

On your server: outbox rows 90 days after they are sent, refused, cancelled
or failed; the diagnostics that were sent 7 days; a screenshot only until
the intake confirms it (or the row is refused or cancelled; a failed one
keeps it for Retry). At Alakazam Labs: submissions 12 months, then the
server label, contact email and ids are removed; diagnostics and
screenshots 90 days. Only Alakazam staff read them.

## Tests and development

`K2_PRODUCT_FEEDBACK_URL` points the daemon at another intake and
`K2_CONNECT_BASE` at another relay (tests and development only). Every K2
test uses loopback fakes (`crates/k2-daemon/tests/it_misc/product_feedback_integration.rs`,
`tests/cli/product_feedback_headless.sh`); none reaches the real services.
