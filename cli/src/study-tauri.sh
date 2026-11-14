_study_page_tauri() {
    cat <<'EOF'
k2 study tauri — the K2 side of a Tauri app, end to end

THE SHAPE
  A Tauri app is a desktop or phone shell around a web page. Two ways:
  - A thin shell over a live site: the window loads your server App's own
    https page. No web build, no second App: THIN SHELL below.
  - A bundled web build: a static App beside the App's server (1, 2 below).
  Either way K2 runs the parts around it: its users, the update feed, its
  "open in app" links and push. The usual shape: the phone and desktop apps
  talk to the App's server, and the App's server talks to K2.
  <box> on this page is this server's K2 Connect name (k2 connect status).
  Sew is K2's chat and voice server. K2 Web is the team that runs k2.dev:
  reach it through your K2 contact.

THIN SHELL OVER A LIVE SITE
  For a server App already on this box (k2 app list). In the Tauri repo's root:
    k2 app init --template tauri --app <app> --dry-run
    k2 app init --template tauri --app <app>
  It adds the updater to src-tauri/tauri.conf.json and writes no web
  manifest. Point the window at the site: in tauri.conf.json,
  app.windows[0].url = "https://<your App's host>/". Then:
  - Origins: the page sends your site's own Origin, so K2 and Sew see a
    browser, and there is nothing to allow. A bundled page
    (tauri://localhost) is different: it needs bearer auth to your server.
  - Feed and links live on the server App: K2 answers /_k2/updates/… and the
    two link files for any App it fronts (it sits in front of the App: the
    default; not with host.clientIp: false in the manifest), even while the
    App is stopped. Below, use <app> wherever it says <name>-web or <name>-api
    (7's upload route is in the node-api template, not your server: copy it
    if you need it).
  - Two endpoints: k2 app updates list <app> --json prints endpoints. --app
    writes the App's own host first, then endpoints.fallback (the stable
    channel's latest.json on K2's download host), which still answers when
    your box is down. The box must be on K2 0.46.7 or newer.
  - The download-host token (k2dl_…): see 3. Nothing pushes without it.
  - A page on a remote URL calls a Tauri plugin only through a capability
    naming that URL (remote.urls); or do it in the shell's Rust.

START (A BUNDLED WEB BUILD)
  In the Tauri repo's root:
    k2 app init --template tauri --dry-run    see every file and change
    k2 app init --template tauri              write them (never overwrites)
  It writes k2/web.manifest.json and k2/README.md, and adds the updater
  lines to src-tauri/tauri.conf.json (only keys it lacks; order and spacing
  kept). tauri.conf.json5 and Tauri.toml are not edited: it prints the lines
  to add. <name> is --name, else the folder's name made into one (lower
  case, anything else becomes -: "Field Notes" is field-notes). The web
  build's App is <name>-web; the server is <name>-api, written by a second
  run with --template node-api. Create each App once, with its public URL:
  k2 app install <name>-web --tunnel (likewise <name>-api).

1. THE WEB BUILD: A STATIC APP
  host.mode "static": K2 serves the folder itself, no server runs, nothing
  of yours executes on the box. Deploys, rollback, labels and custom domains
  work as for any App.
    npm run build
    k2 app install <name>-web --tunnel          (once)
    k2 app deploy <name>-web dist --manifest k2/web.manifest.json
  --manifest packs that file as the Seed's manifest.json, so dist/ deploys as
  the build wrote it. If dist/ has its own manifest.json with other content
  (a web app manifest), the deploy stops with manifest_conflict: name that
  file manifest.webmanifest (and its <link rel="manifest">). Check first:
  k2 app validate dist --manifest k2/web.manifest.json.
  Static keys (host.*): root (the folder to serve, default .), spa (unknown
  paths without an extension answer index.html), notFound (a 404 page),
  headers (CSP, Permissions-Policy, Referrer-Policy, COOP/COEP/CORP, HSTS;
  framing stays K2's: no frame-ancestors), immutable (globs cached for a
  year; default: hashed files under assets/). HTML and JSON are always
  revalidated. Static Apps and these keys need K2 0.46.7 or newer (k2 app
  validate warns).

2. THE APP'S SERVER
  k2 app init --template node-api writes k2/api/: a small @k2/server server
  with a health route and an upload route (below). Then:
    cd k2/api && npm install && cd -
    k2 app sdk add k2/api
    k2 app install <name>-api --tunnel          (once)
    k2 app deploy <name>-api k2/api

3. UPDATES (k2 app updates)
  Tauri's updater asks the App's own host for new versions:
    https://<name>-web.<box>.k2.dev/_k2/updates/<channel>/{{target}}/{{arch}}/{{current_version}}
  init writes the stable channel; for testers, build a copy with stable
  changed to beta in each endpoint (the fallback has a latest.json per
  channel too). The archives come from K2's download host, never from your
  box. The feed only moves people already on your app: the first install
  (a .dmg, an .msi…) is yours to hand out. npx tauri build makes the archives.
  Make the updater's key once, on your own Mac:
    npx tauri signer generate -w ~/.tauri/myapp.key
  Its PUBLIC key goes in two places: tauri.conf.json
  plugins.updater.pubkey (the app checks updates with it) and K2 (K2 checks
  archives with it before anything is uploaded):
    k2 app updates key <name>-web --set "$(cat ~/.tauri/myapp.key.pub)"
  The download host needs this App's own token (k2dl_…). K2 Web mints one per
  App: ask K2 Web for it, naming the id that k2 app updates token <name>-web
  prints with no flags. Keep it (never on the command line); until then, a
  push stops with dl_token_missing:
    k2 app updates token <name>-web --file <file>
  Then push:
    k2 app updates push <name>-web --channel beta --version 1.5.0 \
        --artifact darwin-aarch64=out/MyApp.app.tar.gz \
        --artifact linux-x86_64=out/MyApp.AppImage
    k2 app updates promote <name>-web --from beta --to stable
  Each archive's signature is <file>.sig beside it (tauri signer sign writes
  it). --artifact repeats, one per platform (<os>-<arch>: darwin, linux,
  windows; x86_64, aarch64). promote moves the beta's current release.
  A bad release: k2 app updates rollback <name>-web --channel stable --to
  1.4.2 points the channel back (k2 app rollback is the web build, not
  this). Tauri installs only a newer version, so people already on the bad
  one stay on it: push a higher version (1.5.1).

4. SIGNING: ON YOUR OWN MAC
  The private updater key and your Apple Developer ID stay yours: never in
  K2's App records, never on a node someone else owns.
  - Interactive: the keys stay in your login Keychain (or a key file on your
    Mac); you, or your agent in your own terminal, run
      codesign …  and  npx tauri signer sign …
    then k2 app updates push.
  - Unattended: your own Mac enrolled as your K2 compute node; you put the
    key in your K2 vault (encrypted, yours), and it is handed only to that
    one job on that Mac as a file (k2 compute run --secret NAME …; k2 study
    vault).
  K2's App records hold only the public key; a private key is refused.
  Windows: K2 doesn't sign for Apps yet. Sign on your own machine (Azure
  Artifact Signing or an OV certificate, from Tauri's
  bundle.windows.signCommand or jsign). K2's feed checks only the updater
  signature: an unsigned installer still updates, but Windows may warn or
  block it, so sign before anyone outside your team installs.

5. "OPEN IN APP" LINKS (k2 app links)
    k2 app links set <name>-web --ios TEAMID.com.you.app \
        --android com.you.app:AB:CD:… --path '/tickets/*' --path '/messages?addr=*'
  K2 serves /.well-known/apple-app-site-association and assetlinks.json on
  the App's hosts (static or server App), with or without phone pairing: its
  labels (<app>.<box>.k2.dev) and each custom domain k2 app domains <app>
  shows as live. In the app, list each host exactly
  (applinks:<app>.<box>.k2.dev, applinks:app.you.com), never a wildcard. A
  domain that goes live later answers by itself. Each set
  replaces the whole list: give every app and path again (an app left out
  is removed). One that leaves out a path already set is refused
  (link_path_dropped) unless --drop '<path>' names it as k2 app links shows
  it (phones and Apple's CDN keep the old file for days). k2 app links check
  <name>-web (the owner) compares what the internet sees.

6. PUSH: THROUGH THE APP'S SERVER
  Tauri has no first-party push plugin: the app needs a native plugin that
  gets the device token. The phone sends the token to YOUR server; your
  server registers it with K2 as that person:
    await k2.asGuest(pass).push.register({ … })
  (pass = the person's session pass). Keys and sending: k2 study app-push.
  Who holds the keys: the App's owner gives K2 their own APNs key and
  Firebase account once, and the daemon on your box calls Apple and Google
  itself; K2's Companion relay isn't on this path.
  Desktop: K2 has no desktop push. Your server tells the running app over its
  own socket, and the shell shows an OS notification. While a desktop has a
  Thread open, your server sends push.viewing, so the phone stays quiet.

7. BIG UPLOADS FROM THE PHONE
  Tauri's HTTP plugin holds a whole request body in memory (about 50 MB is
  the practical ceiling). Send files in parts of at most 16 MiB (raw bytes)
  to your server (https://<name>-api.<box>.k2.dev), with the person's session
  pass as Authorization: Bearer <pass>; it hands each part to K2 as them:
    PUT /upload/<uploadId>?room=<room>&filename=<name>&offset=<bytes>[&last=1]
    → k2.asGuest(pass).files.uploadChunk({ workspace: room, dir: 'uploads',
        filename, uploadId, offset, base64, isLast })
  The phone picks <uploadId> (any new id per file); <room> is a room the
  person may write to; offset = bytes already sent. The node-api template
  has this route. After an error, resend from the end of the last part that
  answered 200. For files that must keep going while the app is in the
  background (iOS URLSession, Android WorkManager), use K2's transfer door
  instead: k2 study app-transfers.

8. HOW IS IT DOING
    k2 app status <name>-web       health, checks, errors, channels
    k2 app list                    each App, static or server, and its URL
  host.checks in a manifest (up to 10: {name, path, expect, as?, header?,
  bodyContains?}) are your own probes: run after each deploy and by
  k2 app status; they never block a deploy.

9. LOGINS: ONE SIGN-IN PER DEVICE
  From an app, your server passes device {id, name} in guests.login. One id
  per install, kept in the app's storage, never one per platform. Each person
  keeps up to 20 devices per App, or fewer if you set it (k2 app login
  settings <app> --max-devices 5); K2 signs out another device only past
  that cap, or when the same id signs in again. If a second device signs out
  the first, check for a cap of 1 and a fixed id, then your own server.
  More: k2 study app-profile.

10. CALLS
    k2 app init --template tauri [--app <app>] --voice
  writes what a call in the window needs: the mic and camera keys, the
  hardened runtime's entitlements, src-tauri/src/k2_voice.rs (the mic for
  your App's own origin only), the window settings for a hidden window and
  WebView2, and the CSP in k2/README.md. Tested on macOS; Windows compiles,
  not yet tested; iOS and Android not yet tested; Linux webviews can't call.
  Calls answer on each live host with Sew 0.7.2+; otherwise mark the one
  the shell loads: k2 app domain primary <app> <host>.
  All of it, and the check to run on a real Mac: k2 study tauri-voice.
  Native calls (Sew's engine, tauri-plugin-sew-voice: the call runs in the
  app's Rust and needs no WebRTC in the webview): your server mints a
  short-lived sewcc_ call credential for the signed-in person and returns it
  with api_base https://<your App's host>/_k2/voice/sew/. The app holds no K2
  pass: it talks to that door and to TURN directly, with the TURN host taken
  from the call's join, never hard-coded. The call's media signalling,
  wss://<your App's host>/rtc/v1 with Sew's media token as the bearer, is
  K2's too (sew-signal approved): your server adds nothing for it, and
  keeps its own /rtc relay for the web page. Setup: k2 study chat-voice
  (NATIVE APPS).

11. BEFORE THE STORES (every Tauri App)
  init --template tauri also writes src-tauri/PrivacyInfo.xcprivacy (Apple's
  privacy manifest: add it to the iOS target), src-tauri/Info.ios.plist (the
  scene manifest iOS 27 requires; ITSAppUsesNonExemptEncryption false, true
  with --voice) and src-tauri/src/k2_http.rs (one shared HTTP client). It
  prints what it won't edit: Cargo lines (Tauri 2.12.5 or newer; K2's
  patched wry, so an iframe can't call your Tauri commands) and Android
  lines (POST_NOTIFICATIONS; with --voice the mic, camera and
  foreground-service types). k2/README.md says why each matters, the export
  rules for calls, and what user data your App stores (on the App owner's
  own K2 server) and how to declare what you add. Before each submission:
    k2 app doctor --store [<folder>]
  Still yours: in-app account deletion (no self-delete for an App's people
  yet), a login for the reviewers (a normal account, not a 7-day test
  guest), and more than a website in the App (Apple's guideline 4.2).

MORE: k2 study app-deploys, k2 study app-push, k2 study app-api,
k2 study app-profile, k2 app init --help, k2 app links --help.
EOF
}

_study_page_auth_pages() {
    cat <<'EOF'
k2 study auth-pages — your own sign-in page, passwords straight to K2

THE SHAPE
  Your page, on your App's own host, posts the password to /.k2/auth/* on
  that same host. K2 answers before your server sees a byte and gives back a
  one-time code (60 s, one use). Your server swaps the code, with the PKCE
  verifier it kept, for the person's pass: the same pass guests.login gives
  (k2 study app-api). No password reaches your server; no K2 pass or cookie
  reaches the browser. It starts off: the owner runs k2 app auth door on.
  K2 shows no screens: you design every one (web page or a phone's native
  screens), with your own branding, steps and words.

  server:  const s = k2.auth.begin()   // {challenge, verifier, setCookie}, per attempt
           res.setHeader('set-cookie', s.setCookie)   // keeps s.verifier, HttpOnly
           render the form with s.challenge only
  page:    const p = await fetch('/.k2/auth/ping').then(r => r.json()).catch(() => null)
           if (!p || p.k2 !== true) stop: no door here, send no password
           const r = await fetch('/.k2/auth/sign-in', { method: 'POST',
             headers: { 'content-type': 'application/json' },
             body: JSON.stringify({ username, password, challenge }) })
           const a = await r.json()   // {ok: true, code} | {ok: false, error: {code}}
           post a.code (never the password) to your own server
  server:  const session = await k2.auth.exchange({ code, verifier: k2.auth.verifierFrom(req) })
           your own cookie from session.pass; also send k2.auth.clearCookie()
  One challenge serves one attempt until a code is made: a refused sign-in
  uses nothing, so the page retries (or goes on to set-password) with it.
  Never share one challenge between people or fix it in the page.
  No SDK: POST {code, verifier, device?} to /cli/skin/auth/exchange on your
  gateway link (K2_GATEWAY_URL) with the App pass (K2_PASS_FILE), or to
  /v1/apps/<service id>/auth/exchange with an App key (scope signin or users).
  device {id, name}: a device sign-in, with a refresh token when the App's
  Stay signed in is on. Answers carry authAt (when K2 checked the password).

IS THE DOOR ON? (your server asks before the page posts a password)
  const d = await k2.auth.doorStatus()   // the App pass; your own App only
  → {on, box, appOff, hosts: [{host, door}]}, all true/false: on = box (the
  owner's switch) and not appOff (this App's own). Every public host of the
  App is listed; door: false where none answers (a raw k2.dev label). No
  SDK: GET /cli/skin/auth/door on your link. The page's ping stays its check.

DEVICE SIGN-IN AND REFRESH (an app that stays signed in)
  Exchange with device {id, name} (one id per install: k2 study app-profile).
  While Stay signed in is on (k2 app stay-signed-in; 30 days by default) the
  answer adds refresh (k2skr_…, one use: keep it in the Keychain) and
  passExpiresAt and refreshExpiresAt (seconds since the epoch). Before
  passExpiresAt, or on a 401:
    const s = await k2.guests.refresh({refresh})
    // {pass, refresh, passExpiresAt, refreshExpiresAt, username, principalId,
    //  app, device}; raw JSON names the pass "token". Keep the new refresh.
  It always goes with the App pass, never the person's pass (that one has
  usually ended); the refresh token is the credential, and only your App's
  work. No SDK: POST /cli/skin/refresh {refresh} on your link with the App
  pass. 401 refresh_invalid, or 401 refresh_reused (used twice: K2 signed
  that device out): sign in again.

PHONES AND TAURI
  Native HTTP (no Origin): post to https://<app host>/.k2/auth/*; your
  server keeps s.verifier against a one-time id it hands the app with the
  challenge. A bundled page's fetch sends Origin tauri://localhost: list it
  (k2 app auth origins <app> --origin tauri://localhost); listed origins get
  CORS on every verb and ping. Any other Origin must be the request's own host.

WHERE THE DOOR IS
  Yes: <app>.<box>.k2.dev (K2's front, on unless host.clientIp: false),
  every custom name, the App's front port. No: host.clientIp: false on the
  k2.dev label, a --cmd service, a name your own web server or a CDN
  answers. Anything but {"k2": true} from ping: send no password. Your own
  form posting to your server (k2.login) stays allowed; then your server
  must never log passwords.

THE VISITOR'S ADDRESS (X-K2-Client-IP)
  K2 drops every copy a visitor sends. It sets X-K2-Client-IP (and
  X-Forwarded-For, X-Real-IP) only when it knows the visitor: through the
  box's own :443 (k2 front), or through K2 Connect once its relay passes
  addresses (k2 tunnel status: Visitor IPs). Until then, and for callers on
  the box itself, there is none: per-visitor limits wait and per-App ones
  hold, so one flood of wrong passwords can close the App's sign-in for
  everyone, 5 minutes at a time. Forward it on k2.login (clientIp).

THE VERBS (POST, JSON, Content-Length, at most 8 KiB)
  sign-in          {username, password, challenge}              → {ok, code}
  set-password     {token, password, challenge?}                → {ok, code?}
  change-password  {username, oldPassword, password, challenge} → {ok, code}
  First password: sign-in answers 403 password_change_required:
    {"ok":false,"error":{"code":"password_change_required","hint":"…","changeToken":"k2chg_…","changeExpiresAt":1767225600},"mustChangePassword":true,"changeToken":"k2chg_…","changeExpiresAt":1767225600}
  Read body.changeToken (error.changeToken is the same; 15 minutes); post
  {token: changeToken, password, challenge} to set-password. Invite or
  reset: your page gets k2inv_… after the # (k2 app login settings
  --set-password-url); send it without the #. With a
  challenge you get a code, unless the person can't use this App (then the
  password is set, no code); without one, no code: send them to sign in.
  The token must be this App's; the owner's own reset never works here.
  change-password needs username (the browser holds no pass); it ends the
  person's other K2 sessions, and wrong old passwords count toward lockout.

ERRORS ({ok: false, error: {code, hint, retryAfterSecs?}}; the words are yours)
  401 bad_login  423 locked  429 rate_limited (Retry-After, seconds)
  403 password_change_required  400 password_unchanged | password_too_short
  | password_too_long | challenge_required | bad_request  401 bad_token
  403 bad_origin | app_suspended  411 length_required (send Content-Length)
  413 too_large  503 retry (K2 restarting)  404 not_found (not a door verb)
  405 method  · exchange: 401 bad_code (sign in again)

ON THE PAGE
  A real <form>, autocomplete username / current-password / new-password.
  Don't log, keep or forward the password. No analytics, session replay or
  third-party scripts there; keep your service worker off /.k2/. The door
  keeps the password from your server, not from scripts on your own page.
  authAt within 15 minutes: pairing.start needs no password. Enterprise SSO
  isn't here yet.
EOF
}

_study_page_tauri_voice() {
    cat <<'EOF'
k2 study tauri-voice — calls inside a Tauri app's window

The rest of the Tauri side (updates, signing, links, push): k2 study tauri.
Sew is K2's chat and voice server. <host> below is your App's host: the one
the window loads (<app>.<box>.k2.dev, or your own domain).

Two ways to put a call in the app:
- THE WEBVIEW (most Apps): your page runs Sew's browser voice kit
  (@sew/voice: WebRTC, a noise filter in an AudioWorklet, WebAssembly)
  inside the window: WKWebView on macOS, WebView2 on Windows. Your server
  gets the call credential (sewcc_…) through Sew's App kit (k2 study
  chat-voice); the page talks to https://<host>/_k2/voice/sew/ and to Sew's
  TURN (the relay for the call's audio).
- A NATIVE PLUGIN: Sew's tauri-plugin-sew-voice runs the call in Rust, with
  no WebRTC in the webview; its JS API follows @sew/voice where it can. A
  remote page reaches it over Tauri's IPC: a capability naming the page's
  URL (remote.urls), and ipc: http://ipc.localhost in connect-src. Its guide
  comes from Sew (through your K2 contact). K2 hasn't tested it for an App.
For the webview, in the Tauri repo's root (--dry-run first shows it all):
  k2 app init --template tauri [--app <app>] --voice
It writes these. A file that is there is never changed: it prints the keys
that file lacks, for you to paste in.
- src-tauri/Info.plist (macOS): NSMicrophoneUsageDescription and
  NSCameraUsageDescription (without them WKWebView has no
  navigator.mediaDevices at all), NSAppSleepDisabled (macOS's App Nap would
  slow a hidden app). Info.ios.plist: the two usage keys, and more (Stores,
  below). Android: printed.
- src-tauri/Entitlements.plist and bundle.macOS.entitlements:
  com.apple.security.device.audio-input and .camera, which a signed,
  notarised build (hardened runtime) needs for the mic.
- tauri.conf.json, first window: backgroundThrottling "disabled" (macOS
  14+: without it a hidden window's page stops; on macOS 13 keep the
  window open during a call), and additionalBrowserArgs for WebView2
  (Chromium flags: audio without a click, no slowing while hidden or
  covered, plus Tauri's own defaults).
- src-tauri/src/k2_voice.rs: the mic and camera for pages on
  k2_voice::APP_ORIGINS (it starts as ["https://<host>"]; add each custom
  domain the same way) with no second prompt; any other page is refused.
  Wire it once, where your code calls tauri::Builder::default() (usually
  run() in src-tauri/src/lib.rs; needs Tauri 2.12 or newer):
    mod k2_voice;                 (top of that file)
    tauri::Builder::default()
        .on_permission_request(k2_voice::on_permission_request)
        // …your other .plugin() / .setup() calls, then .run(…)
  A window your Rust code builds: pass its WebviewWindowBuilder through
  k2_voice::voice_window(builder) for the two window settings.
- k2/README.md: the CSP your server sends while the room is on, e.g.
    script-src 'self' 'wasm-unsafe-eval'; worker-src 'self' blob:;
    connect-src 'self' https://<host> wss://<host>
  Serve the kit's worklet and .wasm from your own origin (.wasm as
  application/wasm). CSP doesn't cover TURN. 'wasm-unsafe-eval' needs
  macOS 13+; on macOS 12 WebAssembly needs 'unsafe-eval' instead.
Windows: the person turns on Settings > Privacy > Microphone > "Let
desktop apps access your microphone".
Calls answer on every live host of your App once Sew is 0.7.2+ and your
kit server sets behindK2Front (k2 hostchat home shows Sew's version; k2
study app-chat); otherwise only on the primary host (k2 app domain primary
<app> <host>).
Stores: --voice sets ITSAppUsesNonExemptEncryption true (calls are end-to-end
encrypted; k2/README.md has the export rules) and iOS background audio, and
prints Android's foreground-service lines. Check: k2 app doctor --store.
If the mic fails: no navigator.mediaDevices → Info.plist lacks the keys;
NotAllowedError → the page's origin isn't in APP_ORIGINS, or macOS said no
(System Settings > Privacy & Security > Microphone).
WHAT K2 HAS TESTED: tests/app-template/tauri-voice-check (in K2's public
repo, github.com/Alakazam-211/K2) builds the files --voice writes into a
Tauri 2.12.5 app with K2's patched wry (not yours), signs it ad hoc with
the hardened runtime, and loads a page from a remote origin with the CSP
above, hidden.
  macOS 26.4, no screen, no mic: passed: mediaDevices; WebRTC between two
    peers through an end-to-end encryption frame transform, 50 packets/s for
    10 s while hidden; the AudioWorklet with WebAssembly inside it;
    WebAssembly under 'wasm-unsafe-eval' (refused without it); audio without
    a click; IndexedDB kept across a restart. Without backgroundThrottling
    the hidden page stopped. A hidden page's JS timers still slow (about 3 a
    second): audio doesn't, so keep call logic off setInterval.
    Not yet: the mic itself (prompt, capture, allow and refuse by origin:
    unit tests only), a minimised window, a Developer ID build, a real call.
  Windows (WebView2): compiles (cargo check); never run.
  iOS, Android: not tested. Linux: no; Ubuntu's and Arch's webview has no
    WebRTC: open the call in Chrome.
ON A REAL MAC (Rust installed, a mic), in a clone of that repo:
  K2_CHECK_MIC=1 sh tests/app-template/tauri-voice-check/run.sh
macOS's microphone prompt shows about 15 s into run 1: click Allow within
30 s. Every check line must say PASS, then ALL REQUIRED CHECKS PASSED.
Then in your own app: join a call, minimise the window for 2 minutes and
talk from a phone; you keep hearing.
To be asked again: tccutil reset Microphone
ON WINDOWS (Rust, a mic, the privacy switch on), PowerShell, in
tests\app-template\tauri-voice-check\src-tauri:
  $env:K2_CHECK_MIC="1"; $env:K2_CHECK_PORT="47123"; cargo run
No prompt shows (the app allows its own origin). The same PASS lines.
Browsers: Chrome, Edge, Chromium yes (Sew tests these); Safari 15.4+ yes,
not tested by Sew yet; Firefox no (refused on an encrypted call).

MORE: k2 study tauri, k2 study app-chat, k2 study chat-voice,
k2 app init --help.
EOF
}

