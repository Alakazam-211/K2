//! The template files' text.

use serde_json::{json, Value as J};

/// `k2/web.manifest.json`: a static App for a web build (TC-1), deployed
/// with `k2 app deploy <id> dist --manifest k2/web.manifest.json`.
pub fn web_manifest(app_id: &str, base: &str) -> String {
    let m = json!({
        "schema": 1,
        "kind": "app",
        "id": app_id,
        "name": format!("{base} web").chars().take(40).collect::<String>(),
        "version": "0.1.0",
        "host": {
            "mode": "static",
            "root": ".",
            "spa": true,
            "checks": [
                { "name": "home", "path": "/", "expect": 200 },
                { "name": "deep link", "path": "/k2-check/deep/link", "expect": 200 }
            ]
        }
    });
    let mut s = serde_json::to_string_pretty(&m).expect("a manifest serializes");
    s.push('\n');
    s
}

/// The updater lines for `tauri.conf.json` (or `.json5`) to add by hand
/// (`voice`: and the entitlements file).
pub fn tauri_json_snippet(updater: &J, voice: bool) -> String {
    let mut v = json!({ "bundle": { "createUpdaterArtifacts": true }, "plugins": { "updater": updater } });
    let mut window = String::new();
    if voice {
        v["bundle"]["macOS"] = json!({ "entitlements": "./Entitlements.plist" });
        let keys: serde_json::Map<String, J> = voice_window_keys().into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        v["app"] = json!({ "windows": [J::Object(keys)] });
        window = "// app.windows: add these two keys to your first window (keep its others)\n".into();
    }
    format!(
        "// Merge into your Tauri config (keep what is there):\n{}\n// pubkey: the public key `npx tauri signer generate` prints (never the private key)\n{window}",
        serde_json::to_string_pretty(&v).expect("serializes")
    )
}

/// The same for `Tauri.toml`.
pub fn tauri_toml_snippet(updater: &J, voice: bool) -> String {
    let endpoints: Vec<String> = updater["endpoints"].as_array().map(|a| a.iter().filter_map(J::as_str).map(|e| format!("\"{e}\"")).collect()).unwrap_or_default();
    let macos = if voice {
        format!(
            "\n[bundle.macOS]\nentitlements = \"./Entitlements.plist\"\n\n# In your first [[app.windows]] table, add these two keys (keep its others):\n#   background-throttling = \"disabled\"\n#   additional-browser-args = \"{WEBVIEW2_VOICE_ARGS}\"\n"
        )
    } else {
        String::new()
    };
    format!(
        "# Merge into src-tauri/Tauri.toml (keep what is there):\n[bundle]\ncreate-updater-artifacts = true\n{macos}\n[plugins.updater]\nendpoints = [{}]\n\
         # the public key `npx tauri signer generate` prints (never the private key)\npubkey = \"\"\n",
        endpoints.join(", ")
    )
}

/// `--voice`: what the webview must declare for `getUserMedia` on macOS and
/// iOS (`(key, value XML)`).
pub fn voice_usage_keys(name: &str) -> Vec<(String, String)> {
    vec![
        ("NSMicrophoneUsageDescription".into(), format!("<string>{name} uses the microphone for calls.</string>")),
        ("NSCameraUsageDescription".into(), format!("<string>{name} uses the camera for video calls.</string>")),
    ]
}

/// `--voice`: macOS only, past the usage keys. App Nap would slow a hidden
/// window's timers and its link to the page while a call runs.
pub fn voice_macos_keys() -> Vec<(String, String)> {
    vec![("NSAppSleepDisabled".into(), "<true/>".into())]
}

/// `--voice`: WebView2's (Windows) browser arguments for calls. Setting any
/// replaces wry's defaults, so its `--disable-features` line comes first,
/// then audio without a tap and no slowing of a hidden or covered window.
pub const WEBVIEW2_VOICE_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
--autoplay-policy=no-user-gesture-required \
--disable-background-timer-throttling \
--disable-renderer-backgrounding \
--disable-backgrounding-occluded-windows";

/// `--voice`: the keys added to `app.windows[0]` in `tauri.conf.json`.
/// `backgroundThrottling` works on macOS 14+ (and iOS 17+); WebView2 takes
/// the arguments instead. Both are ignored where they don't apply.
pub fn voice_window_keys() -> Vec<(&'static str, J)> {
    vec![("backgroundThrottling", json!("disabled")), ("additionalBrowserArgs", json!(WEBVIEW2_VOICE_ARGS))]
}

/// `--voice`: `src-tauri/src/k2_voice.rs`, the media permission hook for the
/// App's own origin (`https://<host>`). Built against stock Tauri 2.12+
/// (`tests/app-template/tauri-voice-check` compiles and runs this file).
pub fn k2_voice_rs(host: &str) -> String {
    K2_VOICE_RS.replace("{{HOST}}", host).replace("WEBVIEW2_ARGS_LINE", WEBVIEW2_VOICE_ARGS)
}

const K2_VOICE_RS: &str = r#"//! Calls in this shell's webview. `k2 app init --template tauri --voice` wrote
//! this file; it is yours to change.
//!
//! Your page runs Sew's browser voice kit (WebRTC, an AudioWorklet and
//! WebAssembly) inside the webview: WKWebView on macOS, WebView2 on Windows.
//! This file decides who may use the mic and camera: your App's own pages
//! get them without a second prompt, any other page is refused.
//!
//! Wire it in src-tauri/src/lib.rs (needs Tauri 2.12 or newer):
//!
//!     mod k2_voice;
//!
//!     tauri::Builder::default()
//!         .on_permission_request(k2_voice::on_permission_request)
//!
//! A window that your Rust code builds (not one listed in tauri.conf.json):
//! pass its builder through `k2_voice::voice_window`.
//!
//! macOS still asks once for the microphone (the system prompt, with
//! NSMicrophoneUsageDescription from Info.plist). Windows needs Settings >
//! Privacy > Microphone > "Let desktop apps access your microphone" on.

use tauri::webview::{PermissionKind, PermissionResponse};
use tauri::{Manager, Runtime, Url, Webview, WebviewWindowBuilder};

/// The origins whose pages may use the mic and camera: `https://` plus a
/// host, no path. Add each custom domain the window may load.
pub const APP_ORIGINS: &[&str] = &["https://{{HOST}}"];

/// WebView2's browser arguments for calls (tauri.conf.json carries the same
/// line as additionalBrowserArgs). Setting any replaces wry's defaults, so
/// its `--disable-features` line comes first. Every window that shares a
/// data directory must use the same line.
pub const WEBVIEW2_ARGS: &str = "WEBVIEW2_ARGS_LINE";

/// For `tauri::Builder::on_permission_request`.
pub fn on_permission_request<R: Runtime>(webview: Webview<R>, kind: PermissionKind) -> PermissionResponse {
    decide(webview.url().ok().as_ref(), kind, APP_ORIGINS)
}

/// The mic and camera: allowed when the window's page is on one of
/// `origins`, refused anywhere else. Other permissions: the webview decides.
pub fn decide(page: Option<&Url>, kind: PermissionKind, origins: &[&str]) -> PermissionResponse {
    match kind {
        PermissionKind::Microphone | PermissionKind::Camera => {
            if page.is_some_and(|u| is_app_origin(u, origins)) {
                PermissionResponse::Allow
            } else {
                PermissionResponse::Deny
            }
        }
        _ => PermissionResponse::Default,
    }
}

/// `page`'s origin (scheme, host and port) is one of `origins`.
pub fn is_app_origin(page: &Url, origins: &[&str]) -> bool {
    let origin = page.origin().ascii_serialization();
    origins.iter().any(|o| o.trim_end_matches('/').eq_ignore_ascii_case(&origin))
}

/// A window built in Rust: the same settings tauri.conf.json gives the
/// first window (no slowing while hidden on macOS 14+, the WebView2 line).
#[allow(dead_code)]
pub fn voice_window<'a, R: Runtime, M: Manager<R>>(b: WebviewWindowBuilder<'a, R, M>) -> WebviewWindowBuilder<'a, R, M> {
    b.background_throttling(tauri::utils::config::BackgroundThrottlingPolicy::Disabled)
        .additional_browser_args(WEBVIEW2_ARGS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_apps_own_pages_get_the_mic() {
        let own = Url::parse("https://acme.example.com/room/1").unwrap();
        let other = Url::parse("https://evil.example.net/").unwrap();
        let origins = &["https://acme.example.com"];
        assert_eq!(decide(Some(&own), PermissionKind::Microphone, origins), PermissionResponse::Allow);
        assert_eq!(decide(Some(&own), PermissionKind::Camera, origins), PermissionResponse::Allow);
        assert_eq!(decide(Some(&other), PermissionKind::Microphone, origins), PermissionResponse::Deny);
        assert_eq!(decide(None, PermissionKind::Microphone, origins), PermissionResponse::Deny);
        assert_eq!(decide(Some(&own), PermissionKind::Geolocation, origins), PermissionResponse::Default);
        let http = Url::parse("http://acme.example.com/").unwrap();
        assert_eq!(decide(Some(&http), PermissionKind::Microphone, origins), PermissionResponse::Deny, "the scheme counts");
        let port = Url::parse("https://acme.example.com:8443/").unwrap();
        assert_eq!(decide(Some(&port), PermissionKind::Microphone, origins), PermissionResponse::Deny, "the port counts");
    }
}
"#;

/// `--voice`: the macOS hardened runtime's entitlements.
pub fn voice_entitlements() -> Vec<(String, String)> {
    vec![
        ("com.apple.security.device.audio-input".into(), "<true/>".into()),
        ("com.apple.security.device.camera".into(), "<true/>".into()),
    ]
}

/// `--voice`: the Android manifest lines.
pub fn android_permissions() -> Vec<String> {
    ["RECORD_AUDIO", "MODIFY_AUDIO_SETTINGS", "CAMERA"].iter().map(|p| format!("<uses-permission android:name=\"android.permission.{p}\" />")).collect()
}

/// `<key>`/value lines, to paste inside a plist's `<dict>`.
pub fn plist_entries(entries: &[(String, String)]) -> String {
    entries.iter().map(|(k, v)| format!("  <key>{k}</key>\n  {v}\n")).collect()
}

/// A whole plist.
pub fn plist(entries: &[(String, String)]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n{}</dict>\n</plist>\n",
        plist_entries(entries)
    )
}

/// `k2/README.md`'s voice section (`--voice`); `host` is the App's host.
pub fn voice_readme(host: &str) -> String {
    VOICE_README.replace("{{HOST}}", host)
}

const VOICE_README: &str = r#"
## Calls (`--voice`)

Your page runs Sew's browser voice kit (`@sew/voice`: WebRTC, a noise filter
in an AudioWorklet, WebAssembly) inside the window's webview: WKWebView on
macOS, WebView2 on Windows. `k2 app init --template tauri --voice` wrote:

- `src-tauri/Info.plist` (macOS): `NSMicrophoneUsageDescription` and
  `NSCameraUsageDescription` (without them WKWebView hides
  `navigator.mediaDevices` altogether), and `NSAppSleepDisabled` (App Nap
  would slow a call while the window is hidden). `src-tauri/Info.ios.plist`:
  the two usage keys. Change the sentences to your own words; Apple shows
  them to people.
- `src-tauri/Entitlements.plist` with `bundle.macOS.entitlements`:
  `com.apple.security.device.audio-input` and `.camera` (the hardened
  runtime refuses the mic without them).
- `src-tauri/tauri.conf.json`, your first window: `backgroundThrottling:
  "disabled"` (macOS 14+: a hidden or minimised window keeps running) and
  `additionalBrowserArgs` (WebView2: audio without a tap, no slowing while
  hidden or covered; it keeps wry's own `--disable-features` line). Every
  window that shares a data directory needs the same line.
- `src-tauri/src/k2_voice.rs`: the mic and camera for your App's own origin
  (`https://{{HOST}}`) without a second prompt; any other page is refused.
- On Android: the manifest lines it printed, once `tauri android init` has
  made the manifest. Ask for the mic and camera at run time there.

### Wire it (once)

Where your code calls `tauri::Builder::default()`, usually `run()` in
`src-tauri/src/lib.rs` (Tauri 2.12 or newer: `cargo update -p tauri`):

```rust
mod k2_voice; // top of that file

tauri::Builder::default()
    .on_permission_request(k2_voice::on_permission_request)
    // ...your other .plugin() and .setup() calls, then .run(...)
```

Add each custom domain the window loads to `k2_voice::APP_ORIGINS`, as
`"https://app.example.com"`. A window built in Rust goes through
`k2_voice::voice_window(builder)` for the two window settings.

If the mic fails: no `navigator.mediaDevices` means Info.plist lacks the
keys; `NotAllowedError` means the page's origin isn't in `APP_ORIGINS`, or
macOS said no (System Settings > Privacy & Security > Microphone).

### Your page's CSP

Your server sends it (Tauri adds none to a remote page). With the room on,
for example:

```
default-src 'self';
script-src 'self' 'wasm-unsafe-eval';
worker-src 'self' blob:;
connect-src 'self' https://{{HOST}} wss://{{HOST}} ipc: http://ipc.localhost;
media-src 'self' blob: mediastream:;
```

- The voice door is `https://{{HOST}}/_k2/voice/sew/`: your own origin.
  CSP doesn't cover TURN or the media itself: don't set a `webrtc`
  directive.
- Serve the kit's AudioWorklet script and its `.wasm` from your own origin
  (your web build, not a CDN), the `.wasm` as `application/wasm` (K2's
  static Apps do). `'wasm-unsafe-eval'` needs Safari 16's WebKit (macOS 13
  or newer); on macOS 12 WebAssembly needs `'unsafe-eval'`.
- `ipc: http://ipc.localhost` only if the page calls a Tauri plugin (below).
- Keep the window's data folder (no `incognito`): the kit keeps its
  encryption keys in IndexedDB.

### Windows

The person must turn on Settings > Privacy > Microphone > "Let desktop apps
access your microphone" (and Camera for video).

### A native plugin instead

Sew's `tauri-plugin-sew-voice` runs the call in Rust (no WebRTC in the
webview); its JS API follows `@sew/voice` where it can. A remote page calls
it only through a capability that names its URL (`remote.urls`), with
`ipc: http://ipc.localhost` in `connect-src`.

### What is tested

`k2 study tauri-voice` says what K2 has proven on which system, and the check
to run on your own Mac. Linux webviews can't make a call.
"#;

/// `k2/README.md` for `--app`: a thin shell over a server App already on
/// the box. No web build, no second App.
pub fn tauri_readme_existing(app_id: &str, host: &str, fallback: Option<&str>, dl_id: Option<&str>) -> String {
    let second = match fallback {
        Some(f) => format!("\n  and, when your box is down, `{f}`"),
        None => String::new(),
    };
    let token_for = dl_id.map(|d| format!(" ({d})")).unwrap_or_default();
    format!(
        r#"# The K2 side of this Tauri shell

`k2 app init --template tauri --app {app_id}` wrote this. Nothing here holds a
secret. The shell is a window onto `{app_id}`, which already runs on your box;
there is no web build and no second App.

- `src-tauri/tauri.conf.json`: the updater asks
  `https://{host}/_k2/updates/stable/...` for new versions{second}.
  The build makes updater archives (`bundle.createUpdaterArtifacts`).

## Next

1. Point the window at the live site: `app.windows[0].url` =
   `"https://{host}/"` in `src-tauri/tauri.conf.json`. The page then sends your
   site's own Origin, so K2 and Sew treat it like a browser.
2. Make the update signing key **on your own Mac** (the private key stays
   yours): `npx tauri signer generate -w ~/.tauri/myapp.key`
   Put the public key it prints in `plugins.updater.pubkey`.
3. Give K2 the same public key (never the private one):
   `k2 app updates key {app_id} --set "$(cat ~/.tauri/myapp.key.pub)"`
4. Ask K2 Web (the team that runs k2.dev, through your K2 contact) for this
   App's download-host token{token_for}, then keep it (never on the command
   line): `k2 app updates token {app_id} --file <file>`
5. Ship a signed update to testers, then to everyone:
   `k2 app updates push {app_id} --channel beta --version 1.0.1 --artifact darwin-aarch64=<file>.app.tar.gz`
   `k2 app updates promote {app_id} --from beta --to stable`
6. See how it is doing: `k2 app status {app_id}`

## Where the signing keys live

- The updater key and your Apple Developer ID stay on **your own Mac**.
- K2's App records hold only the updater's **public** key; a private one is
  refused.

More (open-in-app links, logins per device, push, calls): `k2 study tauri`.
"#
    )
}

/// `k2/README.md` for the Tauri template: the next commands and where the
/// signing keys live (TC-13, TC-26).
pub fn tauri_readme(app_id: &str, host: &str) -> String {
    format!(
        r#"# The K2 side of this Tauri app

`k2 app init --template tauri` wrote this folder. Nothing here holds a secret.

- `k2/web.manifest.json`: the web build as a static K2 App (`{app_id}`). K2
  serves the files itself: no server runs.
- `src-tauri/tauri.conf.json`: the updater asks
  `https://{host}/_k2/updates/stable/...` for new versions, and the build
  makes updater archives (`bundle.createUpdaterArtifacts`).

## Next

1. Make the update signing key **on your own Mac** (the private key stays
   yours): `npx tauri signer generate -w ~/.tauri/myapp.key`
   Put the public key it prints in `src-tauri/tauri.conf.json` →
   `plugins.updater.pubkey`.
2. Create the App once, with its public URL: `k2 app install {app_id} --tunnel`
3. Give K2 the same public key (never the private one):
   `k2 app updates key {app_id} --set "$(cat ~/.tauri/myapp.key.pub)"`
4. Ask K2 Web (the team that runs k2.dev, through your K2 contact) for this
   App's download-host token, then keep it (never on the command line):
   `k2 app updates token {app_id} --file <file>`
5. Deploy the web build: `npm run build && k2 app deploy {app_id} dist --manifest k2/web.manifest.json`
6. Ship a signed update to testers, then to everyone:
   `k2 app updates push {app_id} --channel beta --version 1.0.1 --artifact darwin-aarch64=<file>.app.tar.gz`
   `k2 app updates promote {app_id} --from beta --to stable`
7. See how it is doing: `k2 app status {app_id}`

## Where the signing keys live

- The updater key and your Apple Developer ID stay on **your own Mac**: in
  your login Keychain or a key file there, used from your own terminal.
- For unattended builds on your own Mac enrolled as your K2 compute node, the
  key can sit in your K2 vault and is handed only to that one job as a file.
- K2's App records hold only the updater's **public** key; a private one is
  refused.

## Uploads from the phone app

Tauri's HTTP plugin holds a whole request body in memory, so send big files
in parts of at most 16 MiB to your App's own server, which hands each part to
K2 with `files.uploadChunk`. `k2 app init --template node-api` writes that
server route. More: `k2 study tauri`.
"#
    )
}

/// The `tauri-linux` compute preset (compute CE-3), appended to a repo's
/// `.k2/compute.toml` that already keeps presets.
pub const COMPUTE_PRESET: &str = r#"
# k2 app init --template tauri: build the Linux bundle on a compute node
# (k2 compute run --preset tauri-linux -- npm run tauri build).
[presets.tauri-linux]
os = "linux"
mem = "8G"
needs = ["rust@stable", "node@22", "sys:pkg-config", "sys:libwebkit2gtk-4.1-dev", "sys:libssl-dev", "sys:librsvg2-dev"]
"#;

/// The `node-api` template's files: `(relative path, content)`.
pub fn node_api_files(app_id: &str, base: &str) -> Vec<(String, String)> {
    let manifest = json!({
        "schema": 1,
        "kind": "app",
        "id": app_id,
        "name": format!("{base} api").chars().take(40).collect::<String>(),
        "version": "0.1.0",
        "caps": ["files:write"],
        "host": {
            "mode": "cmd",
            "start": ["node", "server.mjs"],
            "health": { "path": "/healthz" },
            "checks": [{ "name": "health", "path": "/healthz", "expect": 200, "bodyContains": "\"ok\":true" }]
        }
    });
    let package = json!({
        "name": app_id,
        "version": "0.1.0",
        "private": true,
        "type": "module",
        "scripts": { "start": "node server.mjs" }
    });
    let mut m = serde_json::to_string_pretty(&manifest).expect("serializes");
    m.push('\n');
    let mut p = serde_json::to_string_pretty(&package).expect("serializes");
    p.push('\n');
    vec![
        ("k2/api/manifest.json".into(), m),
        ("k2/api/package.json".into(), p),
        ("k2/api/server.mjs".into(), NODE_SERVER.to_string()),
        ("k2/api/README.md".into(), node_readme(app_id)),
    ]
}

fn node_readme(app_id: &str) -> String {
    format!(
        r#"# {app_id}: the App's own server

`k2 app init --template node-api` wrote this folder. K2 runs `node server.mjs`
with `$PORT` set and checks `GET /healthz`.

- `cd k2/api && npm install`, then `k2 app sdk add k2/api` (once, and again
  after a K2 update): it puts `@k2/server` in `vendor/`.
- Create the App once, with its public URL: `k2 app install {app_id} --tunnel`
- Deploy: `k2 app deploy {app_id} k2/api`

## Uploads in parts

`PUT /upload/<uploadId>?room=<room>&filename=<name>&offset=<bytes>&last=1`
with the raw bytes of one part (at most 16 MiB) as the body and the signed-in
person's session pass as `Authorization: Bearer <pass>`. The server hands each
part to K2 (`files.uploadChunk`) as that person, so K2 checks their rights;
the last part (`last=1`) lands the file in the room's `uploads/` folder.
A phone app sends parts in order and starts again at `offset` after an error.
"#
    )
}

/// `k2/api/server.mjs`.
const NODE_SERVER: &str = r#"// The App's own server (k2 app init --template node-api). K2 runs it with
// $PORT set; GET /healthz is its health check.
import http from 'node:http'
import { fromEnv } from '@k2/server'

const k2 = fromEnv()
// One part of a phone upload: at most 16 MiB. Tauri's HTTP plugin holds a
// whole body in memory, so phones send big files in parts.
const PART_MAX = 16 * 1024 * 1024

function send(res, status, body) {
  res.writeHead(status, { 'content-type': 'application/json' })
  res.end(JSON.stringify(body))
}

// The request body, or null when it is larger than `max`.
function readBody(req, max) {
  return new Promise((resolve, reject) => {
    const parts = []
    let size = 0
    req.on('data', (c) => {
      size += c.length
      if (size > max) {
        req.destroy()
        resolve(null)
        return
      }
      parts.push(c)
    })
    req.on('end', () => resolve(Buffer.concat(parts)))
    req.on('error', reject)
  })
}

// PUT /upload/<uploadId>?room=&filename=&offset=&last=1, body = one part,
// Authorization: Bearer <the person's session pass>.
async function upload(req, res, url) {
  const uploadId = decodeURIComponent(url.pathname.slice('/upload/'.length))
  const room = url.searchParams.get('room')
  const filename = url.searchParams.get('filename')
  const offset = Number(url.searchParams.get('offset') ?? '0')
  const pass = (req.headers.authorization ?? '').replace(/^Bearer\s+/i, '')
  if (!uploadId || !room || !filename || !Number.isInteger(offset) || offset < 0 || !pass) {
    return send(res, 400, { error: 'usage', hint: 'PUT /upload/<id>?room=&filename=&offset= with Authorization: Bearer <pass>' })
  }
  const body = await readBody(req, PART_MAX)
  if (body === null) return send(res, 413, { error: 'part_too_large', max: PART_MAX })
  try {
    const r = await k2.asGuest(pass).files.uploadChunk({
      workspace: room,
      dir: 'uploads',
      filename,
      uploadId,
      offset,
      base64: body.toString('base64'),
      isLast: url.searchParams.get('last') === '1',
    })
    send(res, 200, r)
  } catch (e) {
    send(res, e?.status ?? 502, { error: e?.code ?? 'upload_failed', hint: e?.message ?? String(e) })
  }
}

const server = http.createServer((req, res) => {
  const url = new URL(req.url ?? '/', 'http://app')
  if (req.method === 'GET' && url.pathname === '/healthz') return send(res, 200, { ok: true })
  if (req.method === 'PUT' && url.pathname.startsWith('/upload/')) {
    upload(req, res, url).catch((e) => send(res, 500, { error: 'internal', hint: String(e) }))
    return
  }
  send(res, 404, { error: 'not_found' })
})

server.listen(Number(process.env.PORT), '127.0.0.1')
"#;
