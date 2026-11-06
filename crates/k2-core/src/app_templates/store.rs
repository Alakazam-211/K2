//! What every Tauri App needs before App Store and Play review, and the
//! runtime fixes K2 carries for its own app (0.46.9, the Tauri App SDK).
//! `k2 app init --template tauri` writes these; `k2 app doctor --store`
//! ([`super::doctor`]) checks a repo for them.
//!
//! The runtime fixes (the merged red-team of 2026-11-05, top-10 row 3):
//! - **Tauri 2.12.5 or newer.** 2.12 moves to tao 0.37, which answers iOS's
//!   scene request without the use-after-free tao 0.35.3 had (tao#1245): an
//!   app with a scene manifest, which iOS 27 requires, crashed at launch.
//!   2.12.4 fixes the IPC handler that panicked on an empty URI path
//!   (tauri#16197). 2.11.5 is the floor for the 2.10 deadlock.
//! - **K2's patched wry** from K2's public repo, by commit: the IPC bridge
//!   answers the main frame only (WebKit gives every frame
//!   `window.webkit.messageHandlers.ipc`), and a nil URL can't abort the
//!   app. Upstream wry 0.57.1 still has neither.
//! - **One HTTP client** (`src-tauri/src/k2_http.rs`): Tauri's HTTP plugin
//!   builds a new client per request, so every call pays a new handshake.

/// The Tauri release the template asks for.
pub const TAURI_FLOOR: (u32, u32, u32) = (2, 12, 5);
pub const TAURI_FLOOR_STR: &str = "2.12.5";
/// Below this, the 2.10 deadlock.
pub const TAURI_DEADLOCK_FIXED: (u32, u32, u32) = (2, 11, 5);
/// tao 0.37 answers iOS's scene request safely (tauri 2.12 pulls it).
pub const TAO_SCENE_FIXED: (u32, u32, u32) = (0, 37, 0);

/// K2's public repo, where its patched wry lives (`third_party/wry`).
pub const K2_REPO: &str = "https://github.com/Alakazam-211/K2";
/// The K2 commit whose `third_party/wry` the template pins. It must be on
/// K2's public main. Move it when K2's wry patches change.
pub const WRY_PATCH_REV: &str = "afe5ccfb0a41eda114f9e849be3aab359e641cc5";
/// wry's version at [`WRY_PATCH_REV`] (Tauri 2.12 asks for ^0.57.0).
pub const WRY_PATCH_VERSION: &str = "0.57.0";

/// The `[patch.crates-io]` row for K2's wry.
pub fn wry_patch_row() -> String {
    format!("wry = {{ git = \"{K2_REPO}\", rev = \"{WRY_PATCH_REV}\" }}")
}

/// Lines to add to the Cargo manifest that holds `[patch.crates-io]` (the
/// workspace root). `header`: the file has no `[patch.crates-io]` yet.
/// `tauri`: the `tauri` requirement is older than [`TAURI_FLOOR_STR`].
/// `reqwest`: no `reqwest` dependency yet (k2_http.rs needs one).
pub fn cargo_snippet(patch: Option<bool>, tauri: bool, reqwest: bool) -> String {
    let mut s = String::new();
    if tauri || reqwest {
        s.push_str("# [dependencies]: change or add (keep your features)\n");
        if tauri {
            s.push_str(&format!("tauri = {{ version = \"{TAURI_FLOOR_STR}\" }}\n"));
        }
        if reqwest {
            s.push_str("# k2_http.rs, the one HTTP client\n");
            s.push_str("reqwest = { version = \"0.12\", default-features = false, features = [\"rustls-tls\"] }\n");
        }
    }
    if let Some(header) = patch {
        if !s.is_empty() {
            s.push('\n');
        }
        s.push_str(
            "# K2's patched wry (0.57.0, from K2's public repo, pinned by commit):\n\
             # the IPC bridge answers the main frame only, and a nil URL can't\n\
             # abort the app. Drop it once upstream wry has both.\n",
        );
        if header {
            s.push_str("[patch.crates-io]\n");
        } else {
            s.push_str("# under your [patch.crates-io]:\n");
        }
        s.push_str(&wry_patch_row());
        s.push('\n');
    }
    s
}

/// `Info.ios.plist` keys every Tauri App needs (`voice`: and the call ones).
/// Values are plist XML, indented for [`super::text::plist_entries`].
pub fn ios_keys(name: &str, voice: bool) -> Vec<(String, String)> {
    let mut v = Vec::new();
    if voice {
        v.extend(super::text::voice_usage_keys(name));
        // A call keeps going with the app in the background. `voip` only
        // with PushKit (k2/README.md).
        v.push(("UIBackgroundModes".into(), "<array>\n    <string>audio</string>\n  </array>".into()));
    }
    // Calls carry end-to-end encryption the App ships itself (not only
    // Apple's TLS): YES, standard algorithms. Without calls: HTTPS only.
    v.push(("ITSAppUsesNonExemptEncryption".into(), if voice { "<true/>" } else { "<false/>" }.into()));
    v.push(("UIApplicationSceneManifest".into(), SCENE_MANIFEST.trim_end().into()));
    v
}

/// iOS 27 requires the scene lifecycle. tao 0.37 (Tauri 2.12) installs its
/// scene delegate for this; tao 0.35.3 crashed here.
const SCENE_MANIFEST: &str = r#"<dict>
    <key>UIApplicationSupportsMultipleScenes</key>
    <true/>
    <key>UISceneConfigurations</key>
    <dict>
      <key>UIWindowSceneSessionRoleApplication</key>
      <array>
        <dict>
          <key>UISceneConfigurationName</key>
          <string>Default Configuration</string>
          <key>UISceneDelegateClassName</key>
          <string>TaoSceneDelegate</string>
        </dict>
      </array>
    </dict>
  </dict>
"#;

/// `src-tauri/PrivacyInfo.xcprivacy`: Apple's privacy manifest.
///
/// The reasons are for what the template's crates call (checked
/// 2026-11-05): Rust's std imports `stat`, `fstat`, `fstatat` and `lstat`
/// (the file timestamp category) in every app; cpal (kessel-audio, the
/// native call plugin) calls `mach_absolute_time` (system boot time) on
/// iOS. Tauri 2.12, tao 0.37, wry 0.57, Sew's crates and kessel-* call no
/// UserDefaults or disk space API.
pub fn privacy_manifest(voice: bool) -> String {
    let boot = if voice { BOOT_TIME } else { "" };
    PRIVACY_MANIFEST.replace("{{BOOT_TIME}}", boot)
}

const BOOT_TIME: &str = r#"    <!-- Calls: the native call plugin's audio (cpal) reads
         mach_absolute_time to time audio between events in the app. -->
    <dict>
      <key>NSPrivacyAccessedAPIType</key>
      <string>NSPrivacyAccessedAPICategorySystemBootTime</string>
      <key>NSPrivacyAccessedAPITypeReasons</key>
      <array>
        <string>35F9.1</string>
      </array>
    </dict>
"#;

const PRIVACY_MANIFEST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <!-- Apple's privacy manifest. k2 app init --template tauri wrote it; it is
       yours to change. Add it to the iOS target (k2/README.md, Store review).
       k2 app doctor --store checks it. -->
  <key>NSPrivacyTracking</key>
  <false/>
  <key>NSPrivacyTrackingDomains</key>
  <array/>
  <!-- What your App keeps about its people. A K2 App keeps it on the App
       owner's own K2 server. Keep this list the same as App Store Connect's
       App Privacy answers (k2/README.md, What user data your App stores
       and why). -->
  <key>NSPrivacyCollectedDataTypes</key>
  <array>
    <!-- The sign-in: the email address a person signs in with. -->
    <dict>
      <key>NSPrivacyCollectedDataType</key>
      <string>NSPrivacyCollectedDataTypeEmailAddress</string>
      <key>NSPrivacyCollectedDataTypeLinked</key>
      <true/>
      <key>NSPrivacyCollectedDataTypeTracking</key>
      <false/>
      <key>NSPrivacyCollectedDataTypePurposes</key>
      <array>
        <string>NSPrivacyCollectedDataTypePurposeAppFunctionality</string>
      </array>
    </dict>
    <!-- The person's account id on the App. -->
    <dict>
      <key>NSPrivacyCollectedDataType</key>
      <string>NSPrivacyCollectedDataTypeUserID</string>
      <key>NSPrivacyCollectedDataTypeLinked</key>
      <true/>
      <key>NSPrivacyCollectedDataTypeTracking</key>
      <false/>
      <key>NSPrivacyCollectedDataTypePurposes</key>
      <array>
        <string>NSPrivacyCollectedDataTypePurposeAppFunctionality</string>
      </array>
    </dict>
    <!-- The install id your server passes in guests.login, so each
         device keeps its own sign-in. -->
    <dict>
      <key>NSPrivacyCollectedDataType</key>
      <string>NSPrivacyCollectedDataTypeDeviceID</string>
      <key>NSPrivacyCollectedDataTypeLinked</key>
      <true/>
      <key>NSPrivacyCollectedDataTypeTracking</key>
      <false/>
      <key>NSPrivacyCollectedDataTypePurposes</key>
      <array>
        <string>NSPrivacyCollectedDataTypePurposeAppFunctionality</string>
      </array>
    </dict>
    <!-- Add a dict like the ones above for each kind your App keeps:
         messages, notes, files: NSPrivacyCollectedDataTypeOtherUserContent
         call recordings or voice notes: NSPrivacyCollectedDataTypeAudioData
         photos or videos people upload: NSPrivacyCollectedDataTypePhotosorVideos
         a display name: NSPrivacyCollectedDataTypeName
         your own crash reports: NSPrivacyCollectedDataTypeCrashData
         your own analytics: NSPrivacyCollectedDataTypeProductInteraction
         (purpose NSPrivacyCollectedDataTypePurposeAnalytics) -->
  </array>
  <!-- APIs that need a reason, for what the template's crates call.
       Add one if you or a crate you add calls more:
         UserDefaults (NSPrivacyAccessedAPICategoryUserDefaults): CA92.1
         free disk space (NSPrivacyAccessedAPICategoryDiskSpace): E174.1
         timestamps of files the person picked (FileTimestamp): 3B52.1 -->
  <key>NSPrivacyAccessedAPITypes</key>
  <array>
    <!-- Rust's std reads file metadata (stat) for files in the app's own
         container. -->
    <dict>
      <key>NSPrivacyAccessedAPIType</key>
      <string>NSPrivacyAccessedAPICategoryFileTimestamp</string>
      <key>NSPrivacyAccessedAPITypeReasons</key>
      <array>
        <string>C617.1</string>
      </array>
    </dict>
{{BOOT_TIME}}  </array>
</dict>
</plist>
"#;

/// Android manifest lines: notifications for every App; the mic, camera and
/// their foreground-service types for calls.
pub fn android_permissions(voice: bool) -> Vec<String> {
    let mut names = vec!["POST_NOTIFICATIONS"];
    if voice {
        names.extend([
            "RECORD_AUDIO",
            "MODIFY_AUDIO_SETTINGS",
            "CAMERA",
            "FOREGROUND_SERVICE",
            "FOREGROUND_SERVICE_MICROPHONE",
            "FOREGROUND_SERVICE_CAMERA",
        ]);
    }
    names.iter().map(|p| format!("<uses-permission android:name=\"android.permission.{p}\" />")).collect()
}

/// `src-tauri/src/k2_http.rs`: the app's one HTTP client.
pub const K2_HTTP_RS: &str = r#"//! One HTTP client for this app's Rust code. `k2 app init --template tauri`
//! wrote this file; it is yours to change.
//!
//! Each reqwest client keeps its own pool of connections. A client built per
//! request pays a new TCP and TLS handshake every time, and Tauri's HTTP
//! plugin does that today. So build one and share it: `k2_http::client()`.
//!
//! Wire it once:
//!   src-tauri/src/lib.rs:  mod k2_http;
//!   src-tauri/Cargo.toml:  reqwest = { version = "0.12", default-features = false, features = ["rustls-tls"] }
//!
//! The settings suit an App behind K2 Connect: HTTP/1.1 only (the tunnel
//! offers no HTTP/2), an idle connection closes after 50 s (K2 closes it at
//! 60 s), and up to 6 idle connections per host for calls in parallel.
//! From your page, use the webview's own fetch() for your App's host: it
//! keeps its connections too.
#![allow(dead_code)]

use std::sync::OnceLock;
use std::time::Duration;

/// The app's one client. Clones share its connections.
pub fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| builder().build().expect("the HTTP client builds"))
}

/// The same settings, for a client that needs more (a proxy, a timeout).
/// Build it once too.
pub fn builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .http1_only()
        .pool_idle_timeout(Duration::from_secs(50))
        .pool_max_idle_per_host(6)
        .tcp_nodelay(true)
        .connect_timeout(Duration::from_secs(15))
}

#[cfg(test)]
mod tests {
    #[test]
    fn one_client_for_the_whole_app() {
        assert!(std::ptr::eq(super::client(), super::client()));
    }
}
"#;

/// `k2/README.md`'s runtime and store sections (every Tauri App).
pub fn store_readme(voice: bool) -> String {
    let mut s = RUNTIME_README.replace("{{FLOOR}}", TAURI_FLOOR_STR).replace("{{WRY_ROW}}", &wry_patch_row());
    s.push_str(&STORE_README.replace("{{EXPORT}}", if voice { EXPORT_VOICE } else { EXPORT_PLAIN }));
    if voice {
        s.push_str(STORE_VOICE_README);
    }
    s.push_str(DATA_README);
    s
}

const RUNTIME_README: &str = r#"
## Runtime fixes (every Tauri App)

K2's own app carries these; yours should too.

- **Tauri {{FLOOR}} or newer** (`tauri = "{{FLOOR}}"` in `src-tauri/Cargo.toml`,
  then `cargo update -p tauri`). Tauri 2.12 brings tao 0.37: with iOS 27's
  required scene manifest (below), tao 0.35 crashed at launch. 2.12.4 fixes
  an IPC handler that aborted on a request with an empty path, and 2.11.5
  fixed the 2.10 deadlock.
- **K2's patched wry.** WebKit gives every frame of a page the Tauri bridge
  (`window.webkit.messageHandlers.ipc`), so an iframe with someone else's
  HTML could call your commands. K2's wry answers the main frame only, and a
  nil URL can't abort the app. Add this to the `Cargo.toml` that has your
  workspace (`src-tauri/Cargo.toml` if there's only the one), then
  `cargo update -p wry`:

  ```toml
  [patch.crates-io]
  {{WRY_ROW}}
  ```

  `k2 app init` printed the lines if they were missing. Cargo fetches K2's
  public repo once for it.
- **One HTTP client.** Tauri's HTTP plugin builds a new client per request,
  so every call pays a new handshake. `src-tauri/src/k2_http.rs` is one
  shared client for your Rust code (add `mod k2_http;` to `lib.rs` and the
  `reqwest` line it names). In the page, call your App's host with the
  webview's own `fetch()`, not the plugin, for anything repeated.

`k2 app doctor --store` checks all of these.
"#;

const STORE_README: &str = r#"
## Store review (App Store and Play)

`k2 app init` wrote the files below. A file you already had is never
changed: the keys it lacks were printed for you to add. Check the repo any
time with `k2 app doctor --store`.

### iOS

- `src-tauri/PrivacyInfo.xcprivacy`, Apple's privacy manifest. Tauri doesn't
  copy it into the app: after `npx tauri ios init`, add it to the iOS target.
  In `src-tauri/gen/apple/project.yml`, under your `<app>_iOS` target's
  `sources:`, add

  ```yaml
      - path: ../../PrivacyInfo.xcprivacy
        buildPhase: resources
  ```

  (or drag it into the target in Xcode, "Copy Bundle Resources"). It
  declares the APIs that need a reason, for what the template's crates call:
  file timestamps (Rust's std reads file metadata) and, with calls, system
  boot time (the native call plugin's audio). Add a reason if you add a
  crate that reads UserDefaults or free disk space.
- `src-tauri/Info.ios.plist` (Tauri merges it into the app's Info.plist):
  - `UIApplicationSceneManifest`: iOS 27 requires the scene lifecycle.
    tao 0.37 (Tauri 2.12) installs its scene delegate; leave
    `UIApplicationSupportsMultipleScenes` true (with false, tao 0.35 showed
    a black screen; an iPad may then open a second window).
  - `ITSAppUsesNonExemptEncryption`: see Export compliance below.
- App Store Connect, App Privacy: answer it from the next section. The
  privacy manifest's list and those answers must say the same.
- Account deletion: a store App with sign-up needs a way to delete the
  account from inside the App (Apple guideline 5.1.1(v); Play asks for a web
  link too). K2 has no self-delete for an App's people yet. Until it does,
  your App's server takes the request and the App's owner removes the
  person (`k2 app user remove <username>`). Say in the App how long that
  takes.

{{EXPORT}}

### Android

`npx tauri android init` writes
`src-tauri/gen/android/app/src/main/AndroidManifest.xml`. Add the
`<uses-permission>` lines `k2 app init` printed, inside `<manifest>`:

- `POST_NOTIFICATIONS`: Android 13+ shows no notification until the person
  allows it. Ask at run time, after they turn on something that notifies.
- Play Data safety: answer it from the next section.
"#;

const EXPORT_PLAIN: &str = r#"### Export compliance

Without calls, the App's only encryption is HTTPS (TLS), which is exempt:
`ITSAppUsesNonExemptEncryption` is `false`, so App Store Connect doesn't
hold each build for the question. Set it to `true` the day you add
encryption of your own (end-to-end encrypted calls or messages, encrypted
files): `k2 app init --template tauri --voice` explains what that means."#;

const EXPORT_VOICE: &str = r#"### Export compliance (calls are end-to-end encrypted)

Calls carry end-to-end encryption your App ships itself (Sew's call
encryption, on top of WebRTC), not only Apple's TLS. So
`ITSAppUsesNonExemptEncryption` is `true`, and App Store Connect asks about
it once per App:

- Your App uses standard encryption algorithms (not proprietary ones), and
  it is a mass-market product. Answer App Store Connect's questions that way.
- The US: a mass-market App with standard encryption needs an annual
  self-classification report to the US Bureau of Industry and Security (due
  each February for the year before). It's yours to file, as the publisher.
- France: Apple asks whether the App is available in France. If it is, you
  need a declaration to ANSSI (France's cybersecurity agency) for the
  encryption, and you upload its receipt to App Store Connect. Or leave
  France out of the App's countries until you have it.
- Never mark the App "exempt" while calls are in it: that is a false
  declaration.
- Chat is encrypted on the server, not end to end. Don't call it "end-to-end
  encrypted" in the App or its store page."#;

const STORE_VOICE_README: &str = r#"
### Calls in the store

- iOS: `UIBackgroundModes` has `audio`, so a call keeps going with the App
  in the background. Apple asks why in review: say "voice and video calls
  continue while the app is in the background". Add `voip` only if you add
  PushKit for incoming calls (with CallKit, which Apple then requires).
- Android: the lines also ask for the foreground-service permissions a call
  needs (`FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_MICROPHONE`,
  `FOREGROUND_SERVICE_CAMERA`). A call in the background runs in a
  foreground service of type `microphone|camera`; without one, Android stops
  the call when the App goes to the background. K2's template doesn't write
  the service: it comes with a native call plugin. Add `mediaProjection` for
  screen sharing and `phoneCall` (with `MANAGE_OWN_CALLS`) only with a
  Telecom call service. Remove what you don't use before you upload.
- Play asks for a foreground-service declaration for each type, with a short
  video of the feature in use (a call that keeps going in the background).
  Record it before you submit: review waits for it.
"#;

const DATA_README: &str = r#"
## What user data your App stores and why

A K2 App keeps its people's data on the App owner's own K2 server: the box
you run, not K2's company and not a third party. For the stores, the App's
owner is the developer who collects it. What K2 keeps for every App:

| Data | Where | Why | App Store label | Play Data safety |
|---|---|---|---|---|
| Email address | your K2 server | sign-in | Contact Info: Email Address | Personal info: Email address |
| Account id | your K2 server | sign-in | Identifiers: User ID | Personal info: User IDs |
| Install id (the device id in `guests.login`) | your K2 server | one sign-in per device | Identifiers: Device ID | Device or other IDs |

All three: linked to the person, not used for tracking, purpose App
Functionality. K2 shares none of it with anyone.

On the phone itself: the sign-in (keep it in the Keychain on iOS and the
Keystore on Android, never in plain files or synced storage) and caches of
what the person has opened. Data that stays on the device isn't "collected"
for the stores.

**What you add is yours to declare.** Your App may keep more, and that's
your choice: messages and files (User Content), call recordings (Audio
Data), photos and videos, a display name, your own crash reports or
analytics. For each kind your App keeps:

1. Add it to `src-tauri/PrivacyInfo.xcprivacy` (the file lists the names).
2. Answer App Store Connect's App Privacy and Play's Data safety the same way.
3. If a third party gets it (an analytics or crash service), say so in both,
   and add that service's own privacy manifest.
4. Say it in your privacy policy (both stores ask for its URL).

The stores also ask whether data is encrypted in transit: yes (HTTPS to your
App's host). And whether people can ask for it to be deleted: see Account
deletion above.
"#;
