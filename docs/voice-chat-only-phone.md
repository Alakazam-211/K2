# Voice & Chat only logins: the phone's API (0.46.10)

What K2 Companion calls for a **Voice & Chat only** login (`voice_chat`), a
login role below Member. Every route below is on the server the person signed
in to, with that login's own session token (`Authorization: Bearer <token>`
or `?token=`). The server decides everything; the phone only draws what the
answers allow.

## Detect it

- `GET /boot-status` → `features` contains `"voice-chat-role-v1"` on a server
  that has the role. With this login's token (Bearer or `?token=`) the
  answer is the **full** view, as for any live login (so `view != "full"`
  after sending a token still means a stale session), and `features` is the
  same list every view carries: `voice-rooms-v1` (while the box hosts
  rooms), `gardens-remote-v1`, `voice-chat-role-v1`, …. With no token the
  default `compat` view carries that same list; only a box set to the
  `ghost` public view drops it.
- `GET /cli/auth/whoami` → `{"role": "voice_chat", "voiceChatOnly": true,
  "canClone": false, …}`. `voiceChatOnly` is always present on a 0.46.10+
  server (false for everyone else). Read it after every sign-in and on every
  `403 role_required` (the role can change live, with no sign-out).

## Sign in

`POST /cli/auth/login {"username", "password"}` → `{token,
mustChangePassword, …}`, as for any login. A reviewer login made with
`mustChangePassword: false` goes straight in. QR pairing
(`/cli/auth/device/pair-code`) is refused for this role
(`400 voice_chat_no_phone`): it always signs in with its password.

## What it may call (the whole list)

| Route | Use |
|---|---|
| `GET /cli/auth/whoami`, `POST /cli/auth/logout`, `POST /cli/auth/change-password`, `GET /cli/users/policy` | its own account |
| `GET /cli/voice/rooms`, `POST /cli/voice/join`, `…/session/leave`, `…/session/state`, `…/session/stats`, `…/session/keys/join`, `…/session/keys/key_packages`, `POST /cli/voice/recording/consent` | voice rooms, exactly as a Member (audio, video and screen share ride the call credential and Sew's media path) |
| `WS /cli/voice/events` | `{"kind":"hello"}`, then `{"kind":"voice_rooms_changed","rev":n}` only; re-read `GET /cli/voice/rooms` on each. 404 `voice_rooms_off` while the server's voice preview is off |
| `GET /cli/shared` | what the owner shared with this login |
| `GET /cli/shared/thread`, `POST /cli/shared/thread/post` | one shared agent's Thread |
| `GET /cli/zen/remote/gardens`, `/page`, `/frame`, `/lib`, `/sources-changes`, `/blob/get`; `POST /cli/zen/remote/call`, `/lib/fetch` | the shared Gardens, read-only |
| `GET /cli/presence/summary` | compat only: always `{"workspaces":[],"agentActivity":[],"activeProjectIds":[]}`; a new phone needn't call it |

Every other `/cli/*` route, `/events` and `/status` answer **403**
`{"error":"role_required","required":"member","role":"voice_chat","message":"This server gives you Voice & Chat only: …"}`
(`required` names the route's own floor: `member`, `admin`, `owner` or
`owner-token`).
That is not a dead session: don't sign in again, re-read whoami and hide
what isn't allowed. The phone's normal agent list (`projects/list`,
`agent/list`, `sessions/events`, `/cli/thread*`, terminals) is refused; draw
the Agents tab from `/cli/shared` instead.

## The Agents tab: `GET /cli/shared`

```json
{"ok": true, "role": "voice_chat",
 "agents":  [{"id": "<project id>", "address": "demo", "name": "Demo",
              "color": "#0d9488", "icon": "data:image/png;base64,…",
              "activity": "idle"}],
 "gardens": [{"id": "g-…", "name": "Phone"}]}
```

- `color`: the agent's `#rrggbb`, or `null`. `icon`: its picture as an image
  data URL (png, jpeg, webp or gif, size-checked; the same view an App
  gets), or `null` (draw initials). No icon route is needed.
- `activity`: `idle` | `working` | `waiting` (waiting on a person), from the
  live turn's counts-only projection; nothing else about what the agent is
  doing.
- An agent or Garden the owner removed drops out of the list.

## A shared agent's Thread

- **Read:** `GET /cli/shared/thread?agent=<address or id>` plus any of
  `limit`, `after_seq`, `since_seq`, `before_seq` → `GET /cli/thread`'s
  answer for that agent, from the same handler, so the same shapes: a page
  (`items[]`, …) or, with `after_seq`, the `thread-delta-v1` delta
  (`items[]`, `changed[]`, `removed[]`, `has_more`, `after_seq`, `rev`,
  `instanceId`). The phone's cache and merge work unchanged. Two
  differences: every `attachments[]` and `attachmentCount` is removed (this
  role can't open Thread files, so show no chip), and the answer adds
  `activity` (`idle` | `working` | `waiting`) for the working strip. Other
  params (`addr`, `conversationId`, `cards`, …) are ignored: it is always
  that agent's own Thread. There is no stream for this role: poll with
  `after_seq` (every 2–3 s while the chat is open is fine).
- **Post:** `POST /cli/shared/thread/post {"agent", "text", "clientMsgId"?,
  "replyTo"?}` → `POST /cli/thread/post`'s answer for a person's compose
  post (`via: "compose"`, `from`/`author` = this login), without
  `attachments`. It is delivered to the agent like any person's message,
  which wakes it. Only those fields are taken (anything else is
  `400 usage`); `"via"` other than `"compose"` is `403 compose_only`; lines
  after the first are stored quoted (`> `); at most 8000 characters. No
  files, cards, slash commands or context lines.
- **Reply:** send `"replyTo": <seq>` (a message's `seq` in the same Thread)
  and only your own words as `text`. K2 writes the desktop's reply text from
  the stored message: `↩ #<seq> <from>:`, the message quoted as `> ` lines
  (whole up to 600 characters, else its first 600 and `…`), a blank line,
  then your words, each line indented two spaces (so none of them can start
  a line that reads as someone else). The phone's reply parser reads it as
  is. A `text` that itself starts with `↩` is refused
  (`400 reply_header_not_allowed`): never send the header.
- **Refusals** (`{ok:false, error:{code, hint}}`): `403 not_shared` (another
  agent), `403 voice_chat_only` (not a live Voice & Chat only login),
  `400 usage`, `403 compose_only`, `404 reply_not_found` (no message with
  that seq), `400 reply_header_not_allowed`.

## A shared Garden

The remote Garden door (`contract-gardens-phone-v1`), unchanged in shape,
admits this login for the Gardens shared with it:

- `GET /cli/zen/remote/gardens` lists only those, with `"person": "shared"`
  and `"readOnly": true`.
- `page`, `frame`, `sources-changes`, `blob/get` work for a shared Garden;
  any other Garden is `403 not_granted`.
- `POST /cli/zen/remote/call` runs only reads: `sources.list`,
  `sources.read`, `sources.history` (each source's card view; a source for
  admins only is refused), `store.get`, `store.list`, `store.changes`,
  `blobs.get`, `blobs.info`, `blobs.list`, `widget.self`. Anything else,
  `sources.refresh` included, is `403 not_granted` ("read-only"); the frame
  should show it as a refused verb.
- `report`, `pause`, `resume`, `blob/put` and `agents-order` are refused:
  don't send them for a read-only Garden.
- Garden changes ride the app socket, which this role doesn't get: re-read
  `gardens?since=<cursor>` when the Garden screen opens or is pulled to
  refresh.

## Calls: the call door `/_k2/voice/sew/*`

The same as a Member's. `POST /cli/voice/join` returns Sew's join answer
with the call credential (`sewcc_…`); the engine's routes on the call door
(the call socket, keys, leave, state, stats, video and `local:screen`
publish) take only that credential, which carries no K2 role: Sew scopes
it, and a Voice & Chat only login is a Sew `member` exactly like a Member.
The web kit's routes on that door run as the person's own login, and this
role resolves to its own `k2:connect:<uuid>` member there too.

## Push

Off for this role in 0.46.10: it can't register a device, and K2 sends it
no Thread, ticket or project push. Call and message notifications for real
Voice & Chat only people are a 0.46.11 item.

## Live role changes

Moving a login into or out of Voice & Chat only keeps its session. The next
request answers by the new role; an open Member socket closes within about
5 s. On a `403 role_required`, re-read whoami: `voice_chat` → show the
Voice & Chat only screens; `member` → the normal app.
