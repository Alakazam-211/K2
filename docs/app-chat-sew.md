# Chat, voice and calls in your App (Chat / Voice Hosting)

For an agent building a K2 App that adds chat (and later voice channels,
person-to-person calls, video and screen share) on the box's Chat / Voice Hosting core
(core id `sew`), powered by Sew. `k2 hostchat` manages the core. The K2 side
ships in K2 0.46.6; the short version is `k2 study app-chat`.

What works when is decided by **Sew's feature flags** (`k2 hostchat home` shows
them), never by version numbers:

| Flag | What it unlocks |
|---|---|
| `app_scoped_sessions` | Sew keeps an App's sessions to that App's own Spaces. Until then an App can't give chat itself, and an App holding a person's chat session can see all chat that person can |
| `home_client` | the owner's home client and "App-only" marks reach Sew |
| `app_origin` | K2 sends each App's `wss://` signalling origin: **voice and calls for a custom App** (expected with Sew 0.4.1) |
| `k2_managed_avatar` | K2's picture replaces one a member set in Sew |
| `app_governed_spaces` | Sew 0.6.0: the **chat key** and App-governed Spaces (below). `k2 app chat <app>` says whether this box's Sew has it |

## The chat key (Sew 0.6.0, K2 0.46.7)

With Sew's `app_governed_spaces` flag there is a shorter path than giving each
person chat (the next sections): one **chat key** per App, and the App decides
who sees which channel.

1. **The key.** The owner, in person (a vault confirm from a terminal outside
   K2): `k2 app grant <app> --kind chat-key [--max-level member|moderator|admin]
   [--max N] [--caps "…"] [--uploads on|off]`. It is a permission K2 keeps, not a
   secret: nothing reaches the App. Every App user with a login then gets chat on
   their first pass, up to N people (default 500); until Sew confirms their
   member the pass answers `503 chat_member_pending` (retry). An agent asks on a
   permission card: `k2 permission request apps.chat-key app=<app> --why "…"`.
   Narrowing or `--revoke` needs no confirm (the owner, an Admin, or the App's own
   agent); `--revoke` also ends the App's chat sessions.
2. **The manifest's `chat` block** (needs `requires.k2 >= 0.46.7`; `k2 app
   validate` checks it): static `channels` (`slug`, `name`, `type` text|voice,
   `private`, `userLimit`), who sees a private one (`roles`, or `rooms`: anyone
   whose role has a room line there), `agents` (workspaces in private text
   channels, at level member: the App's own or a room of its roles) and `levels`
   (role → member|moderator|admin, never above the key's `max_level`). Public
   channels list nobody: every member of the Space sees them.
3. **A Space the App owns.** `k2 chat space create <slug> --name "<Name>" --app
   <app>` (an agent with Manage Chat / Voice Hosting): owned by the App's service
   owner (`app:<app id>`, never a person, never signs in), bound in one step.
4. **The switch.** The owner or an Admin, in person: `k2 hostchat app-space
   governance <app> <space-id> app` (an agent asks on a card:
   `sew.space-governance`). Box users (the owner, Admins) leave the Space; they
   are never members of App chat. K2 then keeps every App user's private
   channels and level in step with their role, within seconds of any change.
5. **Channels made at run time.** `chat.channels` (Sew's bulk channel ops:
   `ensure`, `rename`, `archive`, `unarchive`; 200 a call) and
   `chat.setChannels` (each person's App-managed list, in full; 500 people a
   call), cap `chat:channels`, 60 calls a minute per App. The server SDK's
   helpers build and batch them:

   ```ts
   import { applyChannelOps, channelOp, setAllChannels } from '@k2/server'
   const app = k2.asApp()
   await applyChannelOps(app, [
     channelOp.ensure('job-1042', 'Job 1042', { private: true }),
     channelOp.ensure('site-call', 'Site call', { type: 'voice', userLimit: 12 }),
   ])
   await setAllChannels(app, [{ username: 'dana', channels: ['job-1042'] }])
   ```

   K2 sends each person their role's channels plus the App's list, so the
   manifest and the App's own calls never overwrite each other.
6. **See it.** `k2 app chat <app>`: the key, each Space and who governs it, the
   channels, and each person's last sync. A held person says why: banned in the
   Space, deactivated in chat, the Space at the App's member limit, or an unknown
   channel.

An App user whose name matches a box login (the owner, an Admin) is a separate
account; their chat handle gets a short suffix. An existing App Space moves the
same way: the key, then the switch.

## The shape

| Who | Owns |
|---|---|
| K2 | who a person is (their App login), who may chat (grants), the caps ceiling, the person's Sew account, their name and picture in Sew, the identity map, the owner's settings |
| Sew | the chat API, the gateway socket, media and TURN, end-to-end encryption, roles and permissions inside a Space |
| Your App | its BFF (server), its UI, which Space and channel each thing in your domain maps to |

**One identity.** Your App user *is* their chat user. There is no chat sign-up and
no link step: K2 derives the Sew member `k2:principal:<principalId>` from the App
user. Store K2 principal ids in your database, never emails or usernames (both
change), and resolve Sew member ids with `chat.member` / `chat.whois`.

**K2 is the only source of the profile.** The name Sew shows is the person's K2
profile display name, else their full name, else their username; the picture is
their K2 profile picture. Change them through K2 (`profile.*`); never in Sew.

## Give one App user chat

`<app>` is your App's id from `k2 app list` (its name works too).

1. **Chat / Voice Hosting is on in K2 mode.** `k2 hostchat status` says identity `k2`. If it isn't
   installed, the owner runs `k2 hostchat enable` (never `--identity standalone`:
   an install can't switch later).
2. **A Space for your App.** The owner turns on "Manage Chat / Voice Hosting" for your workspace
   once (`k2 agent access set <workspace> chat_manage on`). Then
   `k2 chat space create <slug> --name "<Name>"` (it prints the Space id) and
   `k2 hostchat app-space bind <app> <space-id>`. Members of a bound Space come
   only from your App's users who have chat.
3. **Chat caps on the person's role** (App-wide: any room line counts). `--caps`
   replaces the line, so keep what it has (`k2 app role list`):
   `k2 app role room <role> <room> --caps <current caps>,chat:read,chat:post`.
4. **The person gets chat.** Look at `k2 hostchat home`:
   - *app-scoped sessions: no* (today): only the owner gives chat. Ask them to run
     `k2 app user chat <username> member --app <app>`. Tell them what it means:
     **this App can see all chat this person can** until Sew has
     `app_scoped_sessions`.
   - *app-scoped sessions: yes*: the owner runs `k2 app grant <app> --kind guests-chat [--max N]`
     once (default ceiling 100 people); then your server gives chat itself:
     `k2.asApp().guests.setChat({username, scope: 'member'})`.
5. **Check:** `k2 app user chat <username> --app <app>` shows `member` and the
   chat id.
6. **Sign them in to chat from your server:**
   `k2.withGuest(session, () => k2.chat.pass())` gives a 60-second pass; Sew
   `POST /api/v1/auth/k2 {pass}` (via `K2_BOX_SEW_API`) gives a Sew session
   (`sews_`, 1 hour). Seal it beside the K2 session; renew under 10 minutes left.
   Right after step 4, Sew may answer `not_provisioned` for a few seconds: retry.

Your manifest asks for what it needs; your next deploy is held until the owner
approves the caps (`k2 app approvals`), unless your workspace ticks the Chat / Voice Hosting core
(`--core sew=on`):

```json
"requires": { "sew": ">=0.3.1" },
"boxServices": [ { "name": "sew-api" }, { "name": "sew-signal" } ],
"caps": ["chat:read", "chat:post"]
```

Add `"voice:join"` (and `"voice:video"`) and `"requires": {"voice": true}` when
you add calls, once Sew reports `app_origin`.

## The caps ceiling

A person's chat pass carries `caps` = their role's chat caps ∩ your manifest's chat
caps, as the owner approved them. `chat:read` is always added. The ceiling never
grants anything: Sew's roles inside the Space still decide.

| Cap | Lets the person |
|---|---|
| `chat:read` | read your App's Spaces |
| `chat:post` | post, react, upload |
| `chat:respond` | answer cards |
| `voice:join` | join voice channels and DM calls (custom Apps: with `app_origin`) |
| `voice:video` | camera and screen share in a call (needs `voice:join`) |

If your manifest declares chat caps and a person's role holds none of them, the mint
answers `403 no_chat_caps`: a pass never goes out without its ceiling. Every App
session's pass carries your App's id as its `app` claim; Sew scopes the session by
it once it has `app_scoped_sessions`.

## Other flows (all from your BFF, never the browser)

**DM or call a person.** `chat.member({principal})` → `memberId`, then Sew
`POST /api/v1/dms` and (with `app_origin`) `POST /channels/{dm}/call/join` through
your BFF.

**Show who's who.** Messages, voice states and call records carry Sew member ids:
`chat.whois({memberIds})` tells you which are your users (principal id, username,
the name and picture hash K2 gave Sew). Anyone else comes back as only
`{memberId, kind, isThisAppUser: false}`; a deleted person is `deleted: true`.
Sew's Space objects carry `app_id` and `app_display_name`.

**Make a channel.** `chat.createChannel({slug, name, private?, members?, space?})`
(needs Sew 0.5.0+, flag `app_channel_create`; your manifest asks for the App-only cap
`chat:channels`, which the owner approves once) makes a text channel in your App's
own bound Space (`space` only when it has several). `members` are your usernames,
each with chat as a member; K2 adds them to the Space first, then makes the channel.
The same slug again is the same channel: `created: false`, and its name and
visibility stay what they were. The answer lists `added`, `notAdded` (`not_found`,
`no_chat`, `guest`, Sew's `not_provisioned`, `deactivated`, `not_space_member`) and
`skipped` (banned in the Space; K2 never adds them again).

**Take chat away.** `guests.setChat({username, scope: 'none'})` (a grant your App
gave), or the owner's `k2 app user chat <username> none`. The Sew member is
deactivated (history stays); giving chat again brings back the same member id.

## Rules your App lives by

- **Server only.** The pass route refuses browser-shaped requests
  (`browser_not_allowed`). The browser holds only your App's own cookie.
- **Your BFF proxies Sew.** `/sew/*` to Sew's `/api/v1` with the `sews_` bearer;
  the gateway socket by a 30-second `sewtk_` ticket; for calls,
  `wss://<your origin>/rtc/v1` relayed to `K2_BOX_SEW_SIGNAL` (K2 tells Sew your
  origin). Check the exact `Origin` and `Host` on every POST and upgrade; use a
  `__Host-` cookie; CSP `connect-src 'self'`.
- **Only your own users, only your own grants.** `guests.setChat` and the identity
  map answer for your App's users only (anyone else is `not_found`). A chat grant
  the owner (or another App) made is the owner's: `not_app_granted`. A person who
  also signs in to another App keeps chat when you remove it: `shared_person`.
- **Never** Sew's mgmt socket, its Postgres, keys, the TURN secret or `/etc/sew`,
  and never `Sew-On-Behalf-Of`. Accounts, Spaces and settings go through K2.
- **Agents never join calls.** They post as themselves (`k2 chat`) and get call
  records and transcripts as messages.
- **Usernames** must be 2–64 of `a-z 0-9 _ -` and not `here`, `channel`,
  `everyone` or `all`; otherwise `username_unusable`.
- **Browsers:** calls are end-to-end encrypted; Firefox can't join calls yet.

## Errors

| Code | When | Do |
|---|---|---|
| `chat_not_app_scoped` (409) | Sew doesn't have `app_scoped_sessions` yet | the owner gives chat (step 4) |
| `grant_required` (403) | no `k2 app grant <app> --kind guests-chat` | ask the owner |
| `app_pass_required` (403) | not your App's own pass | use `k2.asApp()` |
| `cap_not_granted` (403) | your pass lacks `chat:read` (identity map) or `chat:channels` | add it to the manifest; the owner approves |
| `not_found` (404) | not your App's user (or nobody) | — |
| `no_chat_member` (404) | your user has no Sew member yet | give chat, then retry |
| `no_chat_grant` (403) | the person has no chat | step 4 |
| `no_chat_caps` (403) | their role holds none of your chat caps | step 3 |
| `app_claim_invalid` (403) | your App id can't be Sew's `app` claim | report it: K2's ids always fit |
| `not_app_granted` / `chat_off_for_person` (409) | the owner manages this person's chat | ask the owner |
| `shared_person` / `has_assistants` (409) | removing would strand the person | the owner decides |
| `chat_limit` (409) | your ceiling is full (`maxMembers`) | the owner raises `--max` |
| `username_unusable` (422) | the username can't be a chat handle | rename the user |
| `sew_feature_missing` (409) | this Sew can't make channels (needs 0.5.0+) | the owner upgrades Sew |
| `space_unbound` (409) | the Space isn't bound to your App (or you have none) | step 2 |
| `app_mismatch` (409) | the Space belongs to another App | use your own Space |
| `chat_unavailable` (409/503) | Chat / Voice Hosting is off or not reachable | `k2 hostchat status` |
| `rate_limited` (429) | 60 calls a minute per App | wait `retryAfterMs` |
| `chat_key_required` (409) | the App has no chat key (chat.channels, chat.setChannels, the switch) | the owner: `k2 app grant <app> --kind chat-key` |
| `chat_member_pending` (503) | a keyed App's user: Sew hasn't confirmed their member yet | retry after `Retry-After` |
| `agent_subject` (item) | that username is an agent's login | list agents in the manifest's `chat` block instead |
| `not_app_governed` (409) | the Space is still Sew-governed | the owner or an Admin: `k2 hostchat app-space governance … app` |
| `home_client_cannot_govern` (409) | the home-client App can't hold a key or govern | use another App |
| `sew_unreachable` (503) / `sew_refused` (502) | K2 couldn't reach Sew, or Sew refused (the hint says why) | `k2 hostchat status`; fix the op |

Every code above (and more): `k2 explain <code>`.

## Home client and App-only

With `home_client`, the owner picks a **home client** (`k2 hostchat home client
<app>`, usually Hearth) whose sessions see every Space a person is in, with App
Spaces labelled; an App the owner marks **App-only** (`k2 hostchat home app <app>
--hidden on`) never shows there. Recording is off by default everywhere.
