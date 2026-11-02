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
    if voice {
        v["bundle"]["macOS"] = json!({ "entitlements": "./Entitlements.plist" });
    }
    format!(
        "// Merge into your Tauri config (keep what is there):\n{}\n// pubkey: the public key `npx tauri signer generate` prints (never the private key)\n",
        serde_json::to_string_pretty(&v).expect("serializes")
    )
}

/// The same for `Tauri.toml`.
pub fn tauri_toml_snippet(updater: &J, voice: bool) -> String {
    let endpoints: Vec<String> = updater["endpoints"].as_array().map(|a| a.iter().filter_map(J::as_str).map(|e| format!("\"{e}\"")).collect()).unwrap_or_default();
    let macos = if voice { "\n[bundle.macOS]\nentitlements = \"./Entitlements.plist\"\n" } else { "" };
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

/// `k2/README.md`'s voice section (`--voice`).
pub const VOICE_README: &str = r#"
## Calls (`--voice`)

`k2 app init --template tauri --voice` added the mic and camera keys:
`src-tauri/Info.plist` (macOS), `src-tauri/Info.ios.plist` (iOS),
`src-tauri/Entitlements.plist` with `bundle.macOS.entitlements`. Change the
two usage sentences to your own words; Apple shows them to people. It
printed the Android lines to add once `tauri android init` has made the
manifest; ask for the mic and camera at run time there.

These keys are needed, not enough: no Tauri webview has been tested on a real
call yet, and Linux webviews can't make one. Where calls work: `k2 study tauri`.
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
4. Ask K2 Web for this App's download-host token{token_for}, then keep it
   (never on the command line): `k2 app updates token {app_id} --file <file>`
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
4. Ask K2 Web for this App's download-host token, then keep it (never on the
   command line): `k2 app updates token {app_id} --file <file>`
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
