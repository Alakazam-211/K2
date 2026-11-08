# Voice & Chat only logins: the phone's API (0.46.10)

What K2 Companion calls for a **Voice & Chat only** login (`voice_chat`), a
login role below Member. Every route below is on the server the person signed
in to, with that login's own session token (`Authorization: Bearer <token>`
or `?token=`). The server decides everything; the phone only draws what the
answers allow.

## Detect it

- `GET /boot-status` → `features` contains `"voice-chat-role-v1"` on a server
  that has the role.
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
 "agents":  [{"id": "<project id>", "address": "demo", "name": "Demo"}],
 "gardens": [{"id": "g-…", "name": "Phone"}]}
```

An agent or Garden the owner removed drops out of the list.

## A shared agent's Thread

- **Read:** `GET /cli/shared/thread?agent=<address or id>` plus any of
  `limit`, `after_seq`, `since_seq`, `before_seq` → exactly
  `GET /cli/thread`'s answer for that agent (`items[]`, `seq`, …). Other
  params (`addr`, `conversationId`, …) are ignored: it is always that
  agent's own Thread. There is no stream for this role: poll with
  `after_seq` (every 2–3 s while the chat is open is fine).
- **Post:** `POST /cli/shared/thread/post {"agent", "text", "clientMsgId"?}`
  → `POST /cli/thread/post`'s answer for a person's compose post
  (`via: "compose"`, `from`/`author` = this login). It is delivered to the
  agent like any person's message, which wakes it. Only those fields are
  taken (anything else is `400 usage`); `"via"` other than `"compose"` is
  `403 compose_only`; lines after the first are stored quoted (`> `); at
  most 8000 characters. No files, cards, slash commands or context lines.
- **Refusals** (`{ok:false, error:{code, hint}}`): `403 not_shared` (another
  agent), `403 voice_chat_only` (not a live Voice & Chat only login),
  `400 usage`, `403 compose_only`.

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

## Live role changes

Moving a login into or out of Voice & Chat only keeps its session. The next
request answers by the new role; an open Member socket closes within about
5 s. On a `403 role_required`, re-read whoami: `voice_chat` → show the
Voice & Chat only screens; `member` → the normal app.
