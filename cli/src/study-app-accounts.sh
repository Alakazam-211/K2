_study_page_app_pairing() {
    cat <<'EOF'
k2 study app-pairing — pair your App's own phone app by QR

THE MODEL
  A person signed in to your web App presses "Connect your phone". The web
  App asks K2 for a one-time code and draws it as a QR. The phone's camera
  opens YOUR phone app (a universal link / App Link on your App's own host);
  it sends the code to K2 and gets a device sign-in: a pass plus a rotating
  refresh token, for that App and that person. No password on the phone.
  Codes: K2A-…, 5 minutes, one use, bound to the App, the person and the
  web sign-in that made them (a logout or password change kills them).

ONCE, AS THE OWNER (from a terminal outside K2; Touch ID confirms)
  iOS: Associated Domains for your app id; add applinks:<host> for EACH
  host exactly (applinks:jobs.you.k2.dev), never a wildcard (a wildcard
  makes iOS read k2.dev's own file). Android: an autoVerify intent filter,
  https, your hosts, path /.well-known/k2-pair; your signing SHA-256 (and
  Play App Signing's).
  k2 app pairing setup <app> --ios TEAMID.com.you.app \
      --android com.you.app:AB:CD:… [--native-origin capacitor://localhost]
  k2 app pairing check <app>   fetches both files from each host
  v1 pairs on your App's own k2.dev labels. While pairing is on, K2 answers
  /.well-known/apple-app-site-association and assetlinks.json there: list
  your App's other link paths with --link-path. Apple's CDN can take a day;
  phones re-check about weekly. Stay signed in must be on (k2 app
  stay-signed-in <app> <days>; 30 days by default).

IN THE WEB APP (the person's own session pass)
  const p = await k2.asGuest(pass).pairing.start()     // {password} when
    reauth_required: a sign-in older than 15 minutes needs it again
  draw p.qr.rows (1 = dark), show "Expires in 5:00"; poll
  pairing.status({pairingId}) every 2 s: paired → show device.name with
  "Not you? End it" (guests.revokeDevice {id: device.deviceLoginId});
  expired → start again. pairing.cancel({pairingId}) drops it.

IN THE PHONE APP (plain HTTPS to your box; the link names no server)
  1 Read the code from the link's fragment (#K2A-…), or let the person
    paste it from the landing page.
  2 POST /cli/skin/pair/check {code, app} → show "Sign in to <appName> as
    <username>?". Required: a QR someone else made signs in someone else.
  3 On yes: POST /cli/skin/pair {code, app, device: {id, name},
    push: {platform, pushToken, environment}} → the device-login answer
    (token, refresh, …, via: pairing) and push {registered, code?}. A push
    refusal never fails the pairing.
  4 Keep token and refresh in the Keychain / Keystore; POST
    /cli/skin/refresh as after a device login; sign out with
    POST /cli/skin/logout.
  A webview app (Tauri, Capacitor) sends an Origin: the owner lists it
  (--native-origin), else pairing_not_from_browser. A BFF redeems through
  its link (its own limits), never for many phones on the main port.

A DESKTOP APP AND NO PHONE APP
  k2 app pairing setup <app> --desktop-only [--native-origin tauri://localhost]
  turns pairing on with no phone app (status: desktopOnly: true). K2 serves
  no iOS or Android file and nothing opens the QR: show the code (p.code)
  in big type, the person types it into your desktop app, and the desktop
  app does steps 2 to 4 above itself, over plain HTTPS to your box (check
  and redeem are Public: the code is the credential). The same owner
  confirm and checks as a phone app; --ios, --android and --link-path are
  refused with it (--host and the store links are fine).
  A Mac app that opens universal links can go under --ios instead.
  Or skip pairing: your desktop app signs in through the sign-in door with a
  device (k2 study auth-pages: DEVICE SIGN-IN AND REFRESH).

WHO
  The App's agent: k2 app pairing status|off (Build and run Apps), log
  (when it may manage the App's users). Never setup: it asks the owner.
  A phone that is signed in can't pair another (pairing_from_device).

REFUSALS
  pairing_off  device_signin_off  reauth_required  bad_login  locked
  host_not_paired  no_public_host  pairing_not_served  guest_required
  pair_code_invalid (unknown, used, expired, another App)  rate_limited
  pairing_not_from_browser  not_for_host  usage
  K2_APP_PAIRING=off in the daemon's environment stops every App's
  pairing. K2 Companion's own phones are k2 users pair-phone.
EOF
}

_study_page_app_account_deletion() {
    cat <<'EOF'
k2 study app-account-deletion — a person deletes their own account in an App

THE STORE RULES (0.46.9, feature app-account-deletion-v1)
  Apple (App Store 5.1.1(v)): an App where people can make an account must
  let them delete it from inside the App, not only by email or a website.
  Google Play: the same, plus a web page for people who no longer have the
  App; you give its address in the Play Console.
  K2 gives both one verb. Your App draws the button and the page.

THE VERB (k2.asGuest(pass).profile.deleteAccount() in the server SDK)
  profile.deleteAccount   POST /cli/skin/account/delete
  Send the person's own session pass (k2skn_) as Authorization: Bearer,
  and a JSON body: {} or {"password": "..."}. Any door works: your App's
  server, its web page or its phone app, on the same host as its other K2
  calls. The App and the person come from the pass, never the body.
  Recent sign-in: the sign-in behind the pass must be at most 15 minutes
  old. A phone's sign-in counts from when the phone signed in, not from
  its last refresh, so a phone usually needs the password. Someone with no
  password signs in again first (a fresh sign-in counts).
  Ask "Delete your account?" in your App first: the person can't undo it.
  Refusals are JSON {ok: false, error: {code, hint}}:
    401 reauth_required   the sign-in is older: ask for the password, resend
    401 bad_login         a wrong password: ask again
    423 locked            3 wrong passwords lock the login for 15 minutes
                          (error.retryAfterSecs)
    403 password_change_required   someone else set that password: the
                          person signs in and chooses their own first
    403 not_for_host, agent_login  the box login or an agent's own login:
                          never deleted this way
  Either way the call ends: the pass stops working (both modes). Sign the
  person out in the App and clear what it stores on the device.

THE ANSWER (show it to the person)
  state      requested | deleted
  scope      account: the login itself goes
             app: the login also opens another App or the server, so only
             this App's access goes
  done       what went now (sessions, devices, push_devices, pairings, ...)
  pending    what goes when a request completes
  kept       [{item, why}]: what K2 keeps, with a sentence you may show
  requestId, dueAt   while requested (dueAt: seconds since the epoch)
  Immediate mode: state deleted, pending empty.

MODES (per App: k2 app set <app> --account-deletion request|immediate)
  request (the default): K2 signs them out of the App everywhere and turns
    their access off at once. The deletion completes when the owner or the
    App's agent runs k2 app account-deletion complete <app> <user>, or by
    itself when the grace period ends (--deletion-grace, 1 to 90 days,
    default 30). Undo while it waits: k2 app account-deletion cancel.
  immediate: the deletion runs during the call.
  A change of mode or grace period applies to new requests; a waiting
  request keeps its date.
  Either way the owner gets an Inbox notice and the App's agent an inbox
  note, and each step is a line in k2 app history <app> --users.

WHAT GOES (scope account)
  The login, this App's access, sessions, devices, push devices, pairing,
  names and picture, email, read state, wallet items, the App's client logs
  from them, and their chat sessions (they are signed out of chat).
  With scope app: this App's access, sessions, devices, push devices,
  pairing and client logs.
WHAT STAYS (the kept list says why)
  Thread posts and files they sent to agents (the owner's). Files they put
  in a room's folder (the room's). Chat messages in Sew: Sew deactivates
  them but has no erase yet. Your App's own data: your App deletes it, K2
  can't see inside it. One History line with the username (the owner's
  record).

THE WEB PAGE FOR GOOGLE PLAY
  Serve a page on your App's own web side, like
  https://<your app>/delete-account. The person signs in there
  (guests.login) and confirms; your server then calls profile.deleteAccount
  with the pass it just got (a fresh sign-in needs no password). Give that
  address in the Play Console. K2 serves no page.

THE OWNER AND THE APP'S AGENT
  k2 app account-deletion <app>          pending requests, the settings
  k2 app set <app>                       mode, grace period, log retention
  The App's agent completes and cancels with "Let this agent manage its
  Apps' users" on. After any deletion, delete what your App keeps for them.
EOF
}

_study_page_app_profile() {
    cat <<'EOF'
k2 study app-profile — a person's own profile, picture and sessions in an App

THE MODEL (0.46.5, feature app-user-profile-v1)
  A person signed in to an App (their own session pass, k2skn_, from
  guests.login) reads and changes their own account from inside the App:
  the App's web page (the helper, same origin), its server for them (the
  App's link with the person's pass: AR50) or its phone app (the main port,
  Authorization: Bearer). The App and the person come from the pass, never
  the body. An App's own pass, an App key, the owner token: guest_required.
  One person is one account on this box: the name and picture are theirs
  in every App; the role, rooms and sessions shown are this App's only.

THE VERBS (k2.asGuest(pass).profile.* in the server SDK)
  profile.get            GET  /cli/skin/profile[?image=1|?ifImageHash=<h>]
    username (fixed), fullName, displayName, email, imageHash, role and
    rooms here, createdAt, lastSignInAt, test, agentLogin, can {editNames,
    editImage}, counts. The picture (image) only when asked: cache by hash.
  profile.update         POST /cli/skin/profile {fullName?, displayName?}
    64 / 32 characters; controls, bidi and zero-width are dropped; null or
    "" clears. K2's own views show the username beside a self-set name.
  profile.setImage       POST /cli/skin/profile/image {image: data URL}
    PNG, JPEG or WebP, at most 2 MiB; K2 turns it upright (EXIF), shrinks
    it to 256 px and stores its own JPEG (≤ 64 KiB, no EXIF, nothing you
    sent). SVG/GIF: type_refused; wrong bytes: type_mismatch.
  profile.clearImage     POST /cli/skin/profile/image/clear
  profile.sessions       GET  /cli/skin/sessions
    browsers and devices signed in to this App: id, kind, client (a coarse
    label, never an IP), via, createdAt, lastSeenAt, current, push.
  profile.revokeSession  POST /cli/skin/sessions/revoke {id}
  profile.signOutOthers  POST /cli/skin/sessions/revoke-others  → {ended}
    this pass stays; the others and their notification devices go.
  password.change        POST /cli/skin/password/change
    {oldPassword, password, stay: true} keeps this pass signed in.
  A server signing someone in may pass clientLabel ("Chrome on Windows")
  in the guests.login body on the link; the main port reads User-Agent.

DEVICES: ONE ID PER INSTALL
  A login from an app (phone or desktop shell) passes device {id, name} in
  guests.login. It gets its own pass and refresh token (k2 app
  stay-signed-in), with refresh, passExpiresAt and refreshExpiresAt (seconds
  since the epoch) in the answer; guests.refresh trades the refresh token
  for the next pair (k2 study auth-pages: DEVICE SIGN-IN AND REFRESH).
  The id is made once per install and kept in the app's
  own storage, never one per platform: a fixed id like "ios" makes each
  phone replace the one before it. A login with no device is a plain 7-day
  pass, uncounted, with no refresh token. A person keeps up to 20 devices
  per App, or fewer if the App sets it (1 to 20):
    k2 app login settings <app> --max-devices 5
  K2 signs out another device only past that cap: a new device ends that
  person's least recently used ones until it fits (reason device_limit).
  Lowering the cap signs nobody out until their next new device; someone on
  8 with a cap of 5 then goes down to 5. The owner, or the App's own agent
  with the switch "…and manage them end to end" (k2 app access).

WHO DECIDES (per App; both on by default)
  k2 app profile settings <app> [--names on|off] [--image on|off]
  The owner or an Admin. The App's own agent with "manage its Apps'
  users" may only turn them off, for its own workspace's Apps. Off:
  edit_off (nothing written). K2_APP_PROFILE=off in the daemon's
  environment turns every person's edits off (reads stay).

PER PERSON
  Test guests change their names and picture. An agent's own login reads
  only (agent_login). 20 changes per person per 10 minutes (rate_limited).

THE OWNER
  k2 app user show <u> --app <app>          profile, picture hash, sessions
  k2 app user image <u> --clear
  k2 app user sessions <u> --app <app> [--revoke <id> | --revoke-all]
                                            the App's own agent may too (never a box login)

NOT YET
  Email change, a self-service forgot door, and K2's own account page come
  with K2's own sender (0.46.6). Usernames never change. Password reset
  today: k2 study apps (PASSWORD RESET).
EOF
}

_study_page_app_transfers() {
    cat <<'EOF'
k2 study app-transfers — big files that survive the app in the background

THE SHAPE
  A phone pauses or kills an app in the background, so the app can't keep
  its own connection open for a 200 MB file. K2 splits a transfer in two:
  - Your App's server starts it as the person (their session pass) and gets
    back a door path and a short-lived token.
  - The app, or the phone's OS for it, sends the bytes straight to K2's
    transfer door on your App's own host, with that token in a header. No
    pass and no cookie travel with the bytes, and your server never carries
    them.
  The door is /_k2/transfers/ on your App's addresses (<app>.<box>.k2.dev
  and its live custom domains), on a local App's front port
  (http://127.0.0.1:<front port>, an App installed without --tunnel) and on
  this server's own address. K2 answers it before your App, App up or down.
  A browser may call it straight from any origin (CORS *: the token is a
  bearer, never a cookie). Each answer closes the connection
  (HTTP/1.1, one request per connection). K2 answers url as a path: the app
  puts it after the origin it already talks to. Times (expiresAt,
  tokenExpiresAt, K2-Upload-Expires) are milliseconds since the epoch.
  Refusals are JSON {"error": {"code", "hint"}}, sometimes with more fields.

UPLOAD, STEP BY STEP
  1. Your server, as the person:
       const up = await k2.asGuest(pass).files.uploadCreate({
         workspace: room, dir: 'uploads', filename, size, sha256 })
     → { uploadId, url, transferToken, tokenExpiresAt, expiresAt, missing,
         partMax, dir }
     The rules of files.uploadChunk run here, once: files:write (or
     files:upload into a drop folder: leave dir out and it lands in the
     person's own folder there), the folder scope, at most 10 GiB, free disk.
     sha256 (lower-case hex of the whole file) is optional; it is checked at
     the end. tokenTtlSecs (60 to 86400) sets how long the token works: give
     a long one when the phone may sleep through the upload.
     Hand url and transferToken to the app. The App's own pass can't start
     one (guest_pass_required): it is always the person's.
  2. The app sends ranges, in any order, several at once if it likes:
       PUT https://<your App's host><url>
       Authorization: Bearer <transferToken>
       Content-Range: bytes <first>-<last>/<size>     (last is inclusive)
       K2-Content-Sha256: <hex of this range>          (optional)
       <the range's bytes: at most 16 MiB, with Content-Length>
     → 200 and the status JSON of step 3. Ranges need no alignment, may
     overlap, and may be sent again. partMax (bytes, 16 MiB) is the most one
     PUT carries. A few PUTs at once is fine (each token has about 8 requests
     a second). A wrong K2-Content-Sha256 is 422 checksum_mismatch and
     nothing is written. An empty file needs no PUT: just complete.
  3. Ask what K2 holds at any time (after a crash, when the app wakes):
       HEAD <url>  → K2-Upload-Received: 0-8388607,16777216-25165823
                     K2-Upload-Size, K2-Upload-Expires (ms), K2-Upload-Done
       GET  <url>  → the same as JSON: received and missing (lists of
                     [first, last], inclusive), receivedBytes, complete, done
     Send what missing lists. Your server can ask too: files.uploadStatus.
  4. Finish: POST <url>/complete with {"sha256": "<hex of the file>"} (or {})
     → { relPath, size, sha256, done: true }. The file lands under its name,
     or "name (1).ext" when that is taken: nothing is ever replaced. Asking
     again answers the same. Before every byte arrived: 409
     upload_incomplete with missing. A file sha256 that doesn't match: 422
     checksum_mismatch; the upload stays, K2 drops what it held, and every
     range is sent again. Either side may complete: your server calls
     files.uploadComplete({uploadId, sha256}), or reads files.uploadStatus
     ({uploadId}: done and relPath) to learn the app finished. To stop:
     DELETE <url>, or files.uploadAbort({uploadId}).

DOWNLOAD
  Your server, as the person: files.downloadToken({workspace, relPath}) for
  a file they may save (files:download), or thread.attachmentToken({addr,
  id, n}) for a Thread attachment.
     → { url, transferToken, expiresAt, size, etag, name, mime }
  The app: GET https://<your App's host><url> with Authorization: Bearer
  <transferToken>. The answer has ETag and Accept-Ranges: bytes. Range
  works (206 with Content-Range; past the end is 416). To resume, send
  Range: bytes=<bytes you have>- and If-Range: <etag>. If the file changed
  since, the answer is 200 with the whole new file (start over), never a
  mix of versions. A token that expired mid-download: ask your server for a
  new one; the etag stays the same while the file does. A link in a browser
  can't set a header, so a download (never an upload) may carry the token
  as ?token= instead.

IN THE BACKGROUND ON iOS
  Use a background session: URLSessionConfiguration.background(withIdentifier:).
  iOS uploads from files only, so write each range to its own temp file,
  then make one uploadTask(with: request, fromFile:) per range, with
  request.httpMethod = "PUT" and the headers above set on the request. iOS
  sends them while the app sleeps and wakes it when they end
  (application(_:handleEventsForBackgroundURLSession:completionHandler:)).
  On wake: HEAD the url, queue what is still missing, then POST complete.
  Downloads: downloadTask(with: request) with the Authorization header. Keep
  the etag; iOS's resume data resumes with Range and If-Range.

IN THE BACKGROUND ON ANDROID
  Use a WorkManager worker (a CoroutineWorker; for a big file make it a
  long-running worker with setForeground, and add a network constraint).
  Its loop: HEAD, PUT each missing range, POST complete. Return
  Result.retry() on a network error, a 5xx or a 429: WorkManager backs off
  and runs it again, even after the app is killed or the phone restarts.
  Keep uploadId, url and the token in the worker's input data (or your
  app's own encrypted storage). Downloads: the same loop with Range and
  If-Range, appending to a part file.

FROM RUST (A TAURI APP) AND FROM THE WEB
  Rust: the k2-transfer-client crate (MIT OR Apache-2.0). It resumes from
  the door's status, retries with backoff and checks sha256:
    let ep = Endpoint::new("https://<your App's host>", &url, &token);
    client.upload_file(&ep, path, UploadOptions::default(), |p| ...).await?;
    client.download_to(&ep, dest, |p| ...).await?;
  The crate runs in your app's process, so it stops when the phone suspends
  the app: on a phone, hand the PUTs to the OS (above) through a small
  native plugin, and use the crate on the desktop and to finish up when the
  app wakes. A web page or Node: uploadToDoor({ base, url, token, data }):
    import { uploadToDoor, TransferError, type UploadToDoorOptions,
      type DoorUploadStatus, type DoorUploadDone } from '@k2/server'
  (k2 app sdk add). Without the SDK, every field: k2 study app-transfers-http.

TOKENS AND LIMITS
  - An upload token opens its one upload; a download token opens its one
    file. Neither opens anything else. On every request K2 reads the
    person's pass and rights again: a pass that ended (signed out, revoked,
    expired) is 401 pass_invalid, so the person signs in again and you start
    a new upload; a lost cap, room or folder is 403 cap_not_granted.
  - An upload token works for 6 hours (tokenTtlSecs: 60 to 86400). On 401
    transfer_token_expired, your server calls files.uploadToken({uploadId,
    tokenTtlSecs}) and the app goes on with the new token: the upload lives
    on. A download token works for 1 hour (tokenTtlSecs too).
  - An upload lives 7 days, then 410 transfer_expired; K2 deletes what it
    held.
  - One range: 16 MiB. One file: 10 GiB. A person has at most 32 unfinished
    uploads and 256 download tokens at once (429 too_many_transfers). A drop
    folder's caps count the whole declared size of each unfinished upload.
  - About 8 requests a second per token (429 with Retry-After).
  - Retry network errors, 5xx and 429 with backoff. Don't retry another
    4xx: fix the request (bad_range), resend (checksum_mismatch), or send
    what missing lists (upload_incomplete).

MORE: k2 study app-transfers-http (raw HTTP, every field, curl),
k2 study app-api (every verb), k2 study tauri, k2 study app-errors.
EOF
}

_study_page_app_push() {
    cat <<'EOF'
k2 study app-push — notifications to a custom App's own phone app

THE MODEL
  Your App's own phone app (your bundle id, your Apple team, your Firebase
  project) gets notifications from K2. The owner gives K2 the App's Apple
  key and/or Firebase service account once; K2 keeps them in the vault and
  the daemon sends to Apple and Google itself. No K2 Connect relay. The App,
  its agent and other Apps can use the keys, never read them. Push is per
  App and off until the owner approves push:send (or runs k2 app push on).

ONCE, AS THE OWNER (from a terminal outside K2; Touch ID confirms)
  Apple: turn on Push Notifications for the app id; create an APNs key
  (.p8); note its Key ID and your Team ID. Firebase (Android, or iOS through
  Firebase): Project settings > Service accounts > Generate new private key.
  google-services.json / GoogleService-Info.plist go in the app, not to K2.
  k2 app push setup <app> --apns-key AuthKey_XXXX.p8 --key-id XXXX \
      --team-id YYYY --bundle-id com.you.app --fcm-service-account sa.json
  k2 app push check <app>      a fake token: says whether each key works
  k2 app push on <app>         (confirmed again; or approve push:send in a deploy)

IN THE PHONE APP (the guest's own session pass, Authorization: Bearer)
  1 Sign in with a device: POST /cli/skin/login {username, password, app,
    device: {id, name}}; keep the pass and refresh token in the Keychain /
    Keystore; POST /cli/skin/refresh for the next pair.
  2 Ask for notification permission; take the NATIVE token: the APNs
    device token as hex, or the FCM token (Expo: getDevicePushTokenAsync).
  3 On every launch and token change: POST /cli/skin/push/register
    {deviceId: <device.id>, platform: apns|fcm, pushToken, environment:
    sandbox|production}. Debug builds are sandbox. Answer: id, who,
    configured {apns, fcm}; never the token.
  4 Sign-out: POST /cli/skin/push/unregister {deviceId} before the next
    person signs in (else token_in_use). push.device / push.prefs read and
    change previews and mutedTopics.
  5 Receive: your own data keys plus k2App, k2Send, k2Kind (app, agent or
    chat), k2Topic and k2Who (match it to register's who to tell accounts
    apart).
  6 Sensitive content: send a short line and an id; fetch the rest on open.

VIA THE APP'S SERVER (phone → your server → K2, with the person's own pass)
  await k2.asGuest(pass).push.register({deviceId, platform, pushToken})
  While they read a Thread, every 30 s (conversation: its id from thread.*):
  await k2.asGuest(pass).push.viewing({conversation, visible: true})
  Each call quiets that Thread's agent-reply pushes for 90 s; visible: false
  ends it. A guest's own Thread socket can send {"t":"viewing","visible":true}.
  Tauri has no push plugin: a native one hands you the APNs or FCM token.

FROM THE APP
  On its link (push:send):  await k2.push.send({to: {guest}, title, body,
    data, topic})  then k2.push.status({sendId}); k2.push.devices().
  A BFF outside the wall: POST /v1/apps/<id>/push/send with an App key of
    scope push (k2 app key mint <app> --scopes push).
  to: {guest} | {guests: [...]} (at most 1,000) | {room} (one of the App's
  rooms). Only this App's guests; anyone else is skipped.not_a_guest.

CHAT PUSHES (DMs and channels in the App's Sew Space; off by default)
  k2 app push set <app> --chat on   (the server also needs K2_APP_CHAT_PUSH=1)
  DMs always push; a channel at the person's level, All messages or
  @mentions only (default @mentions): the phone app sets it on its own Sew
  session, POST /api/v1/channels/{id}/notify/set {level: all|mentions}.
  A tap opens YOUR phone app. data: k2Kind chat, k2Reason (dm, mention,
  reply, all, keyword), k2Conv (the Sew channel id), k2ConvKind (dm or
  channel), k2Msg (newest message id), k2Count, k2Mention (1 or 0).
  collapseId and threadId are chat:<k2Conv>; Android channel "chat".
  Not sealed: the title is the App's name and the body fixed words (New
  message, Mentioned you, Replied to you, N new messages) unless every
  preview switch is on (the owner's, the App's --previews, the device's).
  While a chat is open, every 30 s: push.viewing({conversation:
  "chat:<k2Conv>", visible: true}). Clear a chat's pushes when it opens.
  A DM goes to the App only when both people use only that App.

THE APP'S AGENT
  k2 app push status|log <app> (Build and run Apps); send, test and
  set --agent-replies / --chat (…and manage them end to end). Never the
  keys: ask the owner with a permission card.

LIMITS (an hour)
  600 push.send calls per App; 30 alerts and 60 silent pushes per guest;
  20,000 device sends per App; 5,000 waiting (push_queue_full); 20 phones
  per guest; 10,000 per App (k2 app push set --max-devices).

REFUSALS
  push_off (not approved or turned off)  push_not_configured (no key)
  push_server_only (--register server: the App's server registers the token)
  not_for_host  guest_required (register is a guest's own pass)  token_in_use
  channel_unavailable (k2-companion isn't ready)  too_many_recipients
  outside_app_rooms  rate_limited (retryAfterMs)  too_large (4,096 bytes)
  airgap  vault_closed  app_pass_required / cap_not_granted (push.send)
  skin_room / pass_wrong_link (push.viewing, register)  Any id: k2 explain <id>

WHEN APPLE OR GOOGLE SAY NO
  A gone token removes that device. A token "from another app" before the
  key's first accepted send means a wrong bundle id / project: sends pause,
  nothing is deleted. A refused key pauses that platform 10 minutes and
  leaves the App's agent a note; k2 app push status shows the code.
  K2_APP_PUSH=off in the daemon's environment stops every App's push. The
  owner's k2 push log shows each phone added or removed, and why. K2
  Companion's phones are k2 push. QR pairing can register: k2 study app-pairing.
EOF
}

_study_page_app_runtime() {
    cat <<'EOF'
k2 study app-runtime — K2 updates and your App: the re-host

WHAT A RE-HOST IS
  After a K2 update, K2 moves each App to a fresh instance on the new
  runtime: the same live Seed, a new instance, the health gate, the switch,
  a drain, the old instance kept warm for the watch window (auto-rollback
  to it). Nobody sees an outage when two copies fit. Its history row says
  rehost, not deploy; owner clients get app_deployed with deployKind rehost.

WHEN IT HAPPENS
  Due: the live instance went live on another K2 or engine version
  (k2 app list: engine <version> [re-host due]). After the engine heal,
  once the old daemon let go of the App ports, and after core upgrades (a
  running Sew, Postgres or mail upgrade, or a newer pinned Sew waiting to
  install; at most 30 min). One App at a time, fewest connections first,
  10 s apart; deferred Apps are tried again every hour. Stopped and
  unwalled (process) Apps are never re-hosted.
  Deferred, the old instance keeps serving: requires_unmet, busy,
  sessions_in_memory (a new instance signs everyone out; the owner gets a
  card), outside_window, policy_manual, policy_off (or K2_APPS_REHOST=off).

THE PER-APP SETTING (an App that can't run two copies)
  No memory or disk for two, extra ports, the process exception, or the
  handover policy. The App panel asks what K2 does then:
  pause   "Pause briefly and update": stop, then start (the default)
  defer   "Skip it and let me update it myself": deferred as needs_handover
          or no_headroom; the owner gets a card
  k2 app rehost policy <app> --when-no-overlap pause|defer
  k2 app rehost policy <app> --policy auto|handover|window|manual|off
                             [--window 02:00-05:00]   (box time)
  Changing it needs the owner, an Admin, or switch 2 for your own App.

THE COMMANDS
  k2 app rehost status [<app>] [--json]  the wave; one App's fingerprint,
                                         due, sessions, the wave's word
  k2 app rehost <app> [--yes] [--pause]  move it now (owner, or switch 1
                                         for your own App); --yes even if
                                         everyone signs in again
  k2 app list [--json]                   the engine column (engine block)
  GET /cli/app/rehost[?app=]             the same, a Member read

REHOST_FAILED AND HOW TO CLEAR IT
  A re-host whose health gate failed, or that rolled back in its watch
  window, holds the App at rehost_failed: it keeps running on its previous
  instance, and K2 won't try again on this K2 version. Read why
  (k2 app history <app>, k2 app errors <app>), fix it, then
  k2 app rehost <app>. The next K2 version clears it too.

THE BREAKER
  Two re-hosts in a row that fail or roll back stop the wave, and the
  owner gets one notice. Every App keeps its current instance; the hourly
  retry goes on with the Apps not held. Deferrals don't count.

YOUR APP UNDER A RE-HOST (two copies run for a while)
  sqlite in /data: WAL and busy_timeout, or writes fail SQLITE_BUSY.
  Periodic work through App heartbeats, not a timer in each copy; queue
  consumers idempotent. Keep sessions sealed in /data, not in memory.
  Schema changes: expand first, contract in a later deploy.
  0.46.4 manifest keys: host.sessions memory|sealed|none, host.overlap
  handover, host.health.readyPath (probed once, logged, never a gate).
  K2 0.46.3 and older refuse a Seed that uses them.

AFTER A K2 UPDATE (the App's agent)
  1. k2 app rehost status <app>: done, or deferred/failed and why.
  2. k2 app list: no [re-host due] left on your App.
  3. k2 app engine status: the engine matches this K2.
  4. k2 app history <app>: a rehost row went live; k2 app errors <app>.
  5. Check the App answers as before; tell your owner what still waits.
  K2 tells you of a deferral older than an hour and of a rehost_failed.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_app_surfaces() {
    cat <<'EOF'
k2 study app-surfaces — room members, and an App's own self and usage

Two surfaces new in K2 0.46.1. Each is one catalog row with two
transports: the App gateway (an App and its guests) and the Garden bridge
(a widget on this computer, acting as the owner).

ROOM MEMBERS (room.members, cap room:members, never in the defaults)
  Who holds a grant on a room: guests and agent logins, their role and
  caps in that room. Built from grants (a switched-off grant is no grant),
  never from who is online. Connect logins have no rooms: they are the
  one line "admins of this box". Grant it per room like any cap:
  `k2 app role room <role> <room> --caps thread:read,room:members`.
  App     GET /cli/skin/members?workspace=<handle>[&app=<own id>]
          Its own App's guests only (a guest of the box login: the box
          login's). The owner may hide names in a room: then username and
          displayName are null for every App caller.
  Garden  k2.room.members({workspace, app?}): every App's guests, every
          name (a Garden is the owner). namesHidden still says the switch.
  Agents  k2 app members [<room>] [--app <id>|host|all]: an agent reads
          its own room; the owner and admins any room.
  Owner   k2 app room-names <room> hide|show (outside K2).

AN APP'S OWN SELF AND USAGE (app.self, app.usage; no cap, own App only)
  App     GET /cli/skin/self   id, live Seed hash, health, deployedAt
                               (seconds since the epoch)
          GET /cli/skin/usage?hours=N   quotas, live connections and
                               subscriptions, totals, per-verb calls from
                               its own audit (default 24, at most 720)
          The App's own pass on its own gateway link (asApp) only: a guest
          gets not_exposed; app=<another App> is not_found, the same answer
          as an App that doesn't exist.
  Garden  k2.app.self({app}), k2.app.usage({app, hours?}) for any App on
          this computer; k2.widget.self(): this widget's id, live hash,
          author and its last 7 days of saves (uses7d).
  Agents  k2 app self <app>, k2 app usage <app> [--hours N] (an agent: its
          own workspace's Apps).

AN APP'S ROOMS AND GUESTS (0.46.4; the App's own pass, never a guest's)
  App     GET /cli/skin/app-rooms   app.rooms: own room, granted, ceiling (no grant needed)
          GET /cli/skin/guests/list?cursor=&limit=   guests.list (needs guests-rooms)
  Garden  k2.app.rooms({app}), k2.guests.list({app}) (cap guests:rooms)
  Login without the owner token: grant once, app.rooms + guests.list at
  boot, guests.login, guests.setRooms (k2 study app-move).

PROJECT CHAT (0.46.3, App only; never on the Garden bridge)
  A pass sees a project only when EVERY member agent is a room it holds
  with thread:read; one it can't reach and the project is skin_room, the
  same as one that doesn't exist. Posting also needs project:write in
  every member room (never a default; in no App core).
  App     GET  /cli/skin/projects            project.list (+ own unread)
          GET  /cli/skin/projects/messages?group=&after_seq=|before_seq=
          POST /cli/skin/projects/post {group, body}   as guest:<username>
          POST /cli/skin/projects/read {group, seq?}   a guest only
          WS   /cli/skin/projects/events?group=   {collection:"project"|"read",...}
  Read state is the guest's own person key (guest:<principalId>).

FILES (0.46.4, App only; k2 app role paths, k2 study apps)
  files:upload  upload doors drop into the line's --upload folders, in
                <folder>/<username>/; no list, read or overwrite, own files included.
  files:download + the line's --download entries: GET /cli/fs/downloads lists,
                GET /cli/fs/download?path= saves (chunks; App only, no Garden door).

WHEN ACCESS CHANGES ON AN OPEN SOCKET (0.46.8; every App socket)
  K2 rechecks an open socket. From the moment access is gone no more frames
  leave it, and it closes within about a second, idle or not (5 s without the
  live-access-v1 feature in me.get). me.events tells you too, and says when access
  arrives (k2 study apps, ACCESS CHANGES).
  What keeps each one open, in its room: /cli/overlay/events and project chat
  thread:read; /cli/fs/events files:read; /cli/activity/events any one of
  activity:read, heartbeats:read, tickets:read, room:members (lose one of
  several and only that kind of frame stops, with no notice: me.get shows it).
  me.get is GET /cli/skin/me: this pass's rooms and caps now (k2 study app-api).
  On a close:
    4013   access_lost. The pass works; this socket's room or caps are gone.
           Don't reopen it. Read me.get and redraw. Open it again only if
           me.get shows them again.
    4003   The pass stopped working, or it expired and has been replaced. Read
           me.get first: 401 means sign them in again; if it answers, reopen. Your
           server's own pass rotates: read K2_PASS_FILE, then reopen.
    other  A drop, or a close code lost on the way. A browser reports a refused
           socket as 1006 too, so read me.get before each reopen:
             401                   signed out. Stop.
             200, room and caps there  reopen, with a growing wait. After 6 refusals
                                   in a row (about 30 s), stop: the App may be
                                   out of sockets (its owner: k2 app quota), or
                                   on thread.events the room's Chat moved (find
                                   its conversation again).
             200, room gone        as 4013.
             no answer, 429, 5xx   a restart or a network drop. Keep retrying,
                                   with a growing wait.
           That is also how a page that was offline learns what changed.
  K2 replays nothing: after any reopen, read again what you show.
  One difference: thread.stream (/cli/thread/stream) serves every room you hold,
  so a lost room only shrinks its list (a new inbox_subscribed frame). It never
  sends 4013.
  Server SDK: it does all of this, and rereads K2_PASS_FILE when it reconnects.
  subscribe's onError gets a K2Error when it stops retrying: code access_lost
  is a 4013; code failed with status 401 is a dead pass (sign in again); code
  failed with status 0 is refused sockets (read me.get, then subscribe again if
  the room is there).

EVENTS (WS /cli/skin/me/events?scope=me; any live pass. k2 study apps, ACCESS CHANGES)
  {"kind":"access_changed"}  this pass's access changed: read me.get. No details.
  (rooms_changed, on /cli/activity/events, only means an agent's look changed.)
EVENTS (WS /cli/activity/events?workspace=<handle>; room:members opens it)
  {"kind":"room_members_changed","workspace":"<handle>"}  refetch members
  {"kind":"app_deployed","workspace":"<handle>","app":"<id>"}  to the App's
  own pass and its guests only; refetch app.self. A Garden widget reloads
  on stale_widget when its own code changes.

ERRORS
  400 usage | 401 pass_invalid | 403 skin_room (a room the pass doesn't
  hold, the same as one that doesn't exist) | cap_not_granted (room:members)
  | not_exposed | 404 not_found | Garden: not_bound (not a local agent).

Types: sdk/server/src/generated/verbs.ts (AppSurfaces.room, .app) and
sdk/generated/k2.d.ts (k2.room, k2.app, k2.widget.self). k2 study app-api.

K2 is MIT-licensed open source. To change this box, `k2 feedback ask` with a brief (`k2 study ticket-brief`). To talk in this chat, `k2 thread`.
EOF
}

_study_page_app_heartbeats() {
    cat <<'EOF'
k2 study app-heartbeats — heartbeats in an app (client contract)

An app with heartbeats:read / heartbeats:write on a room can see and
change THAT room's heartbeats. Never another workspace's, never the
machine's scheduler, wake settings, Keep awake or power routes.
Grant: `k2 app role room <role> <room> --caps heartbeats:read,heartbeats:write`.
heartbeats:write does not imply heartbeats:read. files:* and
activity:read imply neither.

ROUTES (send workspace=<room handle>; never a path)
  heartbeats:read   GET  /cli/heartbeat/list?workspace=
                    GET  /cli/heartbeat/show?workspace=&name=
                    GET  /cli/heartbeat/status?workspace=&name=&limit=   (1-200)
                    GET  /cli/heartbeat/fires-list?workspace=&limit=     (max 200)
  heartbeats:write  POST /cli/heartbeat/add      {workspace,name,frequency,spec,instructions}
                    POST /cli/heartbeat/edit     {workspace,name,frequency?,spec?,instructions?}
                    POST /cli/heartbeat/enable   {workspace,name,enabled:true|false}
                    POST /cli/heartbeat/rename   {workspace,from,to}
                    POST /cli/heartbeat/archive  {workspace,name}   (your "remove")
                    POST /cli/heartbeat/fire     {workspace,name}
  heartbeats:read   WS   /cli/activity/events?workspace=
                         ← {"kind":"heartbeat_changed","workspace":"<handle>"}
                         ← {"kind":"heartbeat_changed","workspace":"<handle>","name":"…","live":true}
  Instructions (the WAKEUP.md body) come back from show only with
  files:read in the room; otherwise show says "instructionsHidden": true.
  heartbeats:write may also write exactly .k2/heartbeats/<name>/WAKEUP.md
  through POST /cli/fs/write-file. files:write cannot touch .k2/heartbeats/.
  Remove is archive: the owner can restore it. Hard delete is owner-only.
  Support check: list answers gateway 404 = not supported on this server;
  403 "missing capability heartbeats:read" = supported, not granted.

WHAT THE K2 CORE DOES
  Scheduling. Firing, into the room's pinned chat. Catch-up: a slot missed
  while the machine slept fires once on wake if it is under 12 h old;
  older slots are skipped. Computing nextFireAt and waitReason. The overdue
  watchdog. Waking and keeping the machine awake. Validation (frequency,
  interval floor, empty instructions).

WHAT THE APP MUST DO
  1. Count down from nextFireAt. It is UTC RFC3339. Show the distance from
     the device clock to that time. Never compute a next fire from the
     schedule. Never show "now" or "0m 0s".
  2. Map waitReason to your own words (reference copy below). An unknown
     reason (a newer server): show "waiting: <reason>" plus waitDetail.
  3. Refetch on heartbeat_changed. The frame carries no row data.
     Debounce: one refetch per 500 ms burst (one change can send two).
  4. Refetch once at nextFireAt + 120 s for each enabled row, if no frame
     arrived. One timer, not a poll. If that deadline is already past, do
     not re-arm.
  5. Send instructions on add. Required. Edit can replace them. You can
     read them back only with files:read.
  6. Show "firing…" for up to 120 s after nextFireAt, and while firing is
     true. After that the daemon will have named the reason.
  7. Treat silence as "no change", not "idle". No frame means nothing
     changed. Pinned-chat heartbeats (the default) send no live frames.
  8. Fire now resets an interval heartbeat: the next slot counts from the
     fire. A failed fire counts toward the 5-failure auto-disable, the same
     as the owner's. Fire now is rate limited; honour Retry-After.

LIMITS
  Fire now: once per heartbeat per app login per 5 minutes, and 12 per
  room per hour. Other writes: 60 per login per room per 10 minutes.
  Over a limit: 429 {"error":"rate_limited","retryAfterSecs":N} plus a
  Retry-After header. App heartbeats fire at most every 5 minutes
  (hourly every_seconds >= 300). Instructions: required, up to 16 KiB.

ERRORS
  400 instructions_required | instructions_too_large | interval_too_short |
      unknown frequency | enabled must be true or false | missing workspace
  403 skin_room (room not on the pass) | missing capability <cap> |
      heartbeats_area (files write into .k2/heartbeats)
  404 no_such_heartbeat (unknown or archived name; never a silent success)
  409 disabled | in_flight (a fire holds the lease) | backoff (with
      nextFireAt) | wakeup_empty
  429 rate_limited

WAIT REASONS (waitReason → meaning → reference copy)
  scheduled                Fires at nextFireAt.
                           "in 12m 04s" / "Tomorrow 7:00 AM"
  in_flight                Firing now (lease held).
                           "firing…"
  window_closed            Outside its firing window; nextFireAt is when it opens.
                           "waiting: window opens 10:00 PM"
  backoff                  Last fire failed; retry at nextFireAt. waitDetail = "failure 2 of 5".
                           "retry in 2m (failure 2 of 5)"
  schedule_error           The schedule can't be parsed; waitDetail says why.
                           "invalid schedule: <detail>"
  wakeup_empty             WAKEUP.md has no instructions. Stays enabled.
                           "waiting: WAKEUP.md is empty"
  wakeup_waiting           WAKEUP.md is missing or can't be read. Stays enabled; each
                           due fire is skipped and retried until it can be read.
                           waitDetail = since when, how many fires skipped, why.
                           "waiting: WAKEUP.md missing, K2 keeps retrying"
  no_agent                 The workspace has no agent to wake.
                           "waiting: no agent in this workspace"
  no_project               The workspace folder is gone.
                           "waiting: workspace folder not found"
  disabled:user            Turned off by a person or app.
                           "disabled"
  disabled:failures        Turned off after 5 failed fires.
                           "disabled: repeated failures"
  disabled:wakeup_missing  Turned off by an older K2 when WAKEUP.md could not be
                           read. K2 no longer does this; such rows stay off until
                           turned on.
                           "disabled: WAKEUP.md missing"
  disabled:wakeup_empty    Old auto-disable relabelled (the file was only empty).
                           "disabled: WAKEUP.md is empty"
  overdue                  More than 120 s past nextFireAt; waitDetail names the gate.
                           "overdue 6m: <gate>"
  no_ticks                 The daemon's scheduler hasn't ticked for 180 s; waitSince = last tick.
                           "waiting: scheduler not ticking (8h)"

EXAMPLE
  09:58:30, nextFireAt 10:00:00Z, scheduled → "in 1m 30s". At 10:01 →
  "firing…". heartbeat_changed at 10:00:40; refetch shows lastFired
  10:00:38 and nextFireAt 10:15:00 → "in 14m 20s". Had no frame come, the
  app refetches once at 10:02:00.

HISTORY
  status / fires-list rows: {firedAt,name,decision,reason,durationMs,actor}.
  decision "changed" = a write (reason names it: added, disabled, renamed
  from x, instructions edited). actor: owner-token | user:<name> |
  agent:<handle> | app:<username> | app-token:<name>; null = the scheduler.
EOF
}

