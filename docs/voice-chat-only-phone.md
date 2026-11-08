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
mustChangePassword, …}`, as for any login. A reviewer login is made with a
**fixed** password (`k2 users add … --keep-password --fixed-password`): it
goes straight in, `POST /cli/auth/change-password` answers
`403 password_fixed`, and wrong guesses lock only the address they came
from (the lockout of this role is per username AND address). QR pairing
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
- `conversationId` (`vc:<agent id>:<login>`) and `conversationAddress`
  (`vc:<agent address>:<login>`): this login's own conversation with the
  agent. Key the chat cache on (server, login, `conversationId`).
- An agent or Garden the owner removed drops out of the list.

## A shared agent's conversation

Each Voice & Chat only login has its **own conversation** with each shared
agent, at the address `vc:<agent>:<login>`: its own posts and the agent's
replies to it. It never sees the agent's own Thread, the owner's messages,
or another login's conversation. The agent gets each post as a guest:
`[guest <login> (Voice & Chat only)] [thread:vc:<agent>:<login>]` and every
line of the person's words quoted (`> `); it answers with
`k2 thread vc:<agent>:<login> "…"`. Share only a **dedicated demo agent**:
K2 refuses to share one with connections, vault grants, a mailbox, compute
grants or no permission prompts (`409 agent_not_demo`) unless the owner
forces it.

- **Each item's sender.** The person's own posts are stored the way an App
  guest's are, in the existing `doc.author` field:
  `{"kind": "guest", "login": "<login>", "name": "<login> (Voice & Chat only)"}`
  (`doc.from` is the login, `doc.via` is `"compose"`). Draw "You" when
  `author.login` is whoami's `username`, else `author.name`. The agent's
  replies have no `author` (an agent's post) and `doc.from` is its address.
- **Isolation.** Every item in an answer is this conversation's
  (`conversation_id` / `conversationId` on every answer). The agent's
  replies to `vc:<agent>:<login>` are stored only in that conversation:
  never in the agent's own Thread, never in another login's conversation,
  and nothing from either ever appears here. `replyTo` resolves only a
  `seq` of this conversation. (Pinned by
  `shared_routes_tests::replies_stay_in_their_conversation`.)
- **Read:** `GET /cli/shared/thread?agent=<address or id>` plus any of
  `limit`, `after_seq`, `since_seq`, `before_seq` → `GET /cli/thread`'s
  answer for this login's conversation with that agent, from the same
  handler, so the same shapes: a page
  (`items[]`, …) or, with `after_seq`, the `thread-delta-v1` delta
  (`items[]`, `changed[]`, `removed[]`, `has_more`, `after_seq`, `rev`,
  `instanceId`). The phone's cache and merge work unchanged. Two
  differences: every `attachments[]` and `attachmentCount` is removed (this
  role can't open Thread files, so show no chip), and the answer adds
  `activity` (`idle` | `working` | `waiting`) for the working strip, and
  `conversationId` / `conversationAddress`. Other
  params (`addr`, `conversationId`, `cards`, …) are ignored: it is always
  that agent's own Thread. There is no stream for this role: poll with
  `after_seq` (every 2–3 s while the chat is open is fine).
- **Post:** `POST /cli/shared/thread/post {"agent", "text", "clientMsgId"?,
  "replyTo"?}` → `POST /cli/thread/post`'s answer for a person's compose
  post (`via: "compose"`, `from`/`author` = this login), without
  `attachments`. It is delivered to the agent like any person's message,
  which wakes it. Only those fields are taken (anything else is
  `400 usage`); `"via"` other than `"compose"` is `403 compose_only`;
  control characters (but newlines) and bidi overrides are removed; at most
  2000 characters. No files, cards, slash commands or context lines.
- **Limits:** one post per 10 s and 200 a day per login (`429
  rate_limited` / `daily_limit`, `error.retryAfterMs`); no post while the
  agent is still on this person's last message (`429 agent_busy`: show it,
  and send when `activity` is `idle`); reads (`/cli/shared`,
  `/cli/shared/thread`) a burst of 20 then one a second (`429
  rate_limited`): a 2–3 s poll fits.
- **Reply:** send `"replyTo": <seq>` (a message's `seq` in this conversation)
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
  that seq in this conversation), `400 reply_header_not_allowed`,
  `400 too_large`, `429 rate_limited | daily_limit | agent_busy`.

## A shared Garden

The remote Garden door (`contract-gardens-phone-v1`), unchanged in shape,
admits this login for the Gardens shared with it:

- `GET /cli/zen/remote/gardens` lists only those, with `"person": "shared"`
  and `"readOnly": true`.
- `page`, `frame`, `sources-changes`, `blob/get` work for a shared Garden;
  any other Garden is `403 not_granted`.
- `POST /cli/zen/remote/call` runs only reads: `sources.list`,
  `sources.read`, `sources.history` (only the sources the owner shared with
  this login by name, `k2 users share … --source`; any other answers
  `cap_not_granted`), `store.get`, `store.list`, `store.changes`,
  `blobs.get`, `blobs.info`, `blobs.list`, `widget.self`, and never another
  widget's space (`from`). Anything else, `sources.refresh` included, is
  `403 not_granted` ("read-only"); the frame should show it as a refused
  verb.
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
role resolves to its own `k2:connect:<uuid>` member there too, with three
limits K2 adds: recording start/stop and muting or moving a member answer
`403 voice_chat_not_allowed`, and `GET /members` names only the people in
the person's own room. `GET /cli/voice/rooms` lists who is in a room only
for the room the person is in (every room keeps its `count`).
`WS /cli/voice/events` allows 3 sockets per login (`429 too_many_sockets`).

## Push

Off for this role in 0.46.10: it can't register a device, and K2 sends it
no Thread, ticket or project push. Call and message notifications for real
Voice & Chat only people are a 0.46.11 item.

## Live role changes

Moving a login into or out of Voice & Chat only keeps its session. The next
request answers by the new role; an open Member socket closes within about
5 s. On a `403 role_required`, re-read whoami: `voice_chat` → show the
Voice & Chat only screens; `member` → the normal app.
