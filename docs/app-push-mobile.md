# Push notifications for your own phone app (K2 Apps)

This guide is for the developer of a phone app that is the front end of a
custom K2 App: your own bundle id, your own Apple developer team, your own
Firebase project. Since K2 0.46.4, K2 can put notifications on your App's
users' lock screens.

How it works:

- The App owner gives K2 the App's Apple key, its Firebase service account,
  or both, once.
- K2 keeps the keys in its vault on the owner's own server and sends to
  Apple and Google directly. No Alakazam machine sees your notifications.
- Your App, its agent and other Apps can *use* the keys but never read
  them back.
- K2 keeps the device list and makes sure each App reaches only its own
  users. It also handles rate limits, opt-out and the audit trail.

The short version is `k2 study app-push`.

## 1. Once, as the App owner

### Apple (iOS)

1. In your Apple developer account, turn on **Push Notifications** for your
   app id.
2. Create an **APNs auth key** (Keys → +, tick Apple Push Notifications
   service). Download `AuthKey_XXXXXXXXXX.p8`. Note:
   - its **Key ID** (10 characters);
   - your **Team ID** (10 characters, top right of the account page).
3. One key works for every app in your team. We recommend a new key just
   for K2 Apps, so you can revoke it without touching anything else.
4. If Apple asks which environments the key covers, allow both. Debug
   builds get **sandbox** tokens; TestFlight and App Store builds get
   **production** tokens.

### Firebase (Android, or iOS through Firebase)

1. Create a Firebase project and add your Android app (and, if you want
   Firebase to deliver to iOS too, your iOS app; upload your APNs key to
   Firebase for that).
2. Go to Project settings → Service accounts → **Generate new private key**.
   This downloads a JSON file.
3. `google-services.json` / `GoogleService-Info.plist` go **in your app**,
   not to K2.

### Give the keys to K2

Run this from a terminal outside K2 on the server (or a machine that reaches
it as the owner). Touch ID, your login or the vault password confirms it.

```sh
k2 app push setup <app> \
  --apns-key AuthKey_XXXXXXXXXX.p8 --key-id XXXXXXXXXX \
  --team-id YYYYYYYYYY --bundle-id com.you.app \
  --fcm-service-account service-account.json

k2 app push check <app>   # one real call per key to a fake token
k2 app push on <app>      # or approve push:send when the App's deploy asks
```

How K2 handles them:

- K2 checks each key before storing it. The `.p8` must be a P-256 key and
  sign a token. The service account must be a `service_account` whose
  `token_uri` is exactly `https://oauth2.googleapis.com/token`.
- The files are read on your machine. Their contents go to the daemon over
  the local socket; the paths never do.
- `k2 app push status <app>` shows the key ids and fingerprints, never a
  key.
- `k2 app push creds rm <app> --apns|--fcm|--all` removes a key (the same
  confirm).
- An App's agent can never set, replace, remove or read a key. It asks you
  with a permission card.

## 2. In the phone app

All of these calls go to your K2 server (`https://<your box>`) with the
signed-in person's **session pass**: `Authorization: Bearer k2skn_…`.

### Sign in with a device

```http
POST /cli/skin/login
{"username": "dana", "password": "…", "app": "<service id>",
 "device": {"id": "<a stable install id>", "name": "Dana's iPhone"}}
```

- The answer carries `token` (the pass) and `refresh` (a `k2skr_…` refresh
  token). Keep both in the Keychain (iOS) or Keystore (Android).
- `POST /cli/skin/refresh {"refresh": "…"}` trades the refresh token for a
  new pair. Each refresh token works once.
- Use the same `device.id` as the push `deviceId` below. Then the push
  registration lives as long as the device sign-in, through every refresh.

### Get the native token

- **iOS, APNs directly:** call `registerForRemoteNotifications()` and send
  the device token as **hex**.
- **iOS through Firebase, and Android:** send the **FCM registration token**.
- **Expo / React Native:** use `getDevicePushTokenAsync()`, *not* the Expo
  push token.

```swift
func application(_ app: UIApplication,
                 didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
    let hex = token.map { String(format: "%02x", $0) }.joined()
    K2.registerPush(deviceId: installId, platform: "apns", pushToken: hex,
                    environment: isDebugBuild ? "sandbox" : "production")
}
```

### Register on every launch and every token change

```http
POST /cli/skin/push/register
{"deviceId": "<device.id>", "platform": "apns", "pushToken": "<hex>",
 "environment": "sandbox"}
```

The fields:

| Field | Values |
|---|---|
| `platform` | `apns` (a hex device token) or `fcm` (an FCM token) |
| `environment` | APNs only: `sandbox` for Debug builds, `production` otherwise (the default) |
| `previews` | `false` hides agent-reply text on this device; `null` = on |
| `mutedTopics` | the topics this device skips (at most 32) |

The field is `pushToken`, not `token`. A body never names the App or the
person; both come from the pass.

The answer:

```json
{"ok": true, "id": "apd_…", "deviceId": "…", "platform": "apns",
 "environment": "sandbox", "channel": "app", "previews": null,
 "mutedTopics": [], "lastSeenAt": 1792000000, "who": "<32 hex>",
 "configured": {"apns": true, "fcm": false}}
```

- `who` is an opaque key for this person on this App. Every push carries it
  as `k2Who`, so an app with two accounts signed in knows whose push
  arrived.
- `configured` tells you whether the owner has set the keys yet. You can
  integrate before they exist: registration works, and sends answer
  `push_not_configured`.
- A registration never returns the token.

The other calls:

| Call | Does |
|---|---|
| `GET /cli/skin/push/device?deviceId=` | the same answer |
| `POST /cli/skin/push/prefs {deviceId, previews?, mutedTopics?}` | changes choices without the token |
| `POST /cli/skin/push/unregister {deviceId}` | removes it; answers `{ok, removed}` |

Signing the device out also removes its registration on the server. So do
a password change, the owner removing the person, and the App being removed.

### Receive

- **iOS** shows `aps.alert` for you; your own keys are at the top level of
  the payload.
- **Android** gets `notification` and `data`.

K2 adds these keys:

| Key | Meaning |
|---|---|
| `k2App` | the App's id |
| `k2Send` | the send id |
| `k2Kind` | `app` (your App sent it) or `agent` (an agent replied in a room) |
| `k2Topic` | the topic, when the send named one |
| `k2Who` | match it to `who` from register |

No username, person id, room name or server name ever rides a push. Apple
and Google don't see who.

### Sensitive content

Notifications are not end-to-end sealed in this version. Only Apple and
Google see the text in transit, as for any app. If that matters, send a
short generic line and an id in `data`, then fetch the rest from your App
when the person opens the notification.

## 3. Sending, from your App

### From the App's server, inside the wall

The App's manifest must ask for the `push:send` cap; the owner approves it
once.

```js
const {sendId} = await k2.push.send({
  to: {guest: principalId},          // or {guests: [...]} or {room: 'sales'}
  title: 'Your job was approved',
  body: 'Tap to see the details',
  data: {jobId: 'j_42'},             // strings only
  topic: 'jobs',                     // people can mute a topic
  collapseId: 'job:j_42',
})
const s = await k2.push.status({sendId})  // {queued, sent, dead, failed, wrongApp, …}
```

### From a backend outside the wall

Use an App key with the `push` scope. The owner mints it with
`k2 app key mint <app> --scopes push`.

```http
POST /v1/apps/<service id>/push/send
Authorization: Bearer k2sk_…
{"to": {"guests": ["dana", "eli"]}, "title": "…", "body": "…"}
```

### The fields

| Field | Rule |
|---|---|
| `to` | exactly one of `guest`, `guests` (at most 1,000) or `room` (one of the App's rooms) |
| `title` | at most 200 bytes; required unless `silent` |
| `body` | at most 1,024 bytes; required unless `silent` |
| `data` | at most 32 string values, 2 KiB; `aps`, `k2*`, `from`, `notification`, `message_type`, `collapse_key`, `google.*` and `gcm.*` are reserved |
| `topic` | `a-z 0-9 . _ -`, at most 32 characters |
| `collapseId` | at most 64 bytes |
| `threadId` | at most 256 bytes |
| `sound` | a sound name, or `none` |
| `badge` | 0 to 99,999 |
| `silent` | `true` wakes the app with no banner (`content-available` / FCM data-only) |
| `priority` | `high` or `normal` |
| `ttlSecs` | up to 28 days |
| `mutable` | iOS: a Notification Service Extension may change it |
| `androidChannel` | the Android notification channel id |

The whole payload must fit Apple's and Google's 4,096 bytes.

### What a send answers

A send answers at once with `202 {sendId, queued, skipped}`. `skipped`
counts:

| Count | Means |
|---|---|
| `not_a_guest` | names that aren't your App's users. An unknown name and another App's user count the same. |
| `no_devices` | users with no registered phone |
| `muted_topic` | devices that muted the topic |
| `rate_limited` | users over their hourly budget |
| `platform_not_configured` | devices on a platform the App has no key for |

### The App's agent and the owner

`k2 app push send <app> --to dana --title … --body …`. Then
`k2 app push log <app>` for the counts, and `k2 app push test <app> --to dana`
for a fixed test line.

## 4. Limits

These are the defaults, per hour:

| Limit | Default |
|---|---|
| `push.send` calls per App | 600 |
| alert pushes per user per App | 30 |
| silent pushes per user per App | 60 |
| device sends per App | 20,000 |
| sends waiting to go out per App | 5,000 (`push_queue_full`) |
| registers per pass | 30 |
| phones per user | 20 (the least recently seen goes) |
| phones per App | 10,000 (the owner changes it with `k2 app push set <app> --max-devices N`) |

## 5. When something goes wrong

| Code | Means |
|---|---|
| `push_off` | the owner hasn't turned the App's notifications on (or turned them off) |
| `push_not_configured` | no Apple or Google key yet |
| `guest_required` | register needs a person's own session pass |
| `channel_unavailable` | `channel: "k2-companion"` isn't available yet |
| `not_for_host` | the box login has no push |
| `too_many_recipients` | more than 1,000 recipients in one call; batch with `guests` |
| `outside_app_rooms` | the room isn't one of the App's rooms |
| `rate_limited` | over a limit; wait `retryAfterMs` |
| `too_large` | the payload doesn't fit 4,096 bytes |
| `airgap` | the server is air-gapped |
| `vault_closed` | the server is still starting |
| `app_pass_required`, `cap_not_granted` | `push.send` needs the App's own link pass with `push:send` |

When Apple or Google refuse:

- **A gone token** removes that device (`dead`). `wrongApp` counts tokens
  from another app: check the bundle id in `k2 app push status`.
- **A refused key** pauses that platform for 10 minutes and leaves the
  App's agent a note. `k2 app push status` shows the reason code; replace
  the key with `k2 app push setup`.
- **Sandbox vs production:** a Debug build's token sent as `production`
  (or the other way round) is refused. Register each build with the right
  `environment`.

## 6. Switches

- `k2 app push off <app>` stops one App at once.
- `K2_APP_PUSH=off` in the daemon's environment stops every App on the
  server.
- K2 Companion's own phones are `k2 push`, a separate list.
