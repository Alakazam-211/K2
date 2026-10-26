# Pair your own phone app by QR (K2 Apps)

This guide is for the developer of a custom K2 App that has its own phone
app (your own bundle id or Android package). Since K2 0.46.5, a person who
is signed in to your web App can sign your phone app in with one scan:

1. On the web App they press **Connect your phone**. The web App asks K2
   for a one-time code and draws it as a QR.
2. They point the phone's camera at it. The link is on your App's own host
   (`https://<your App's host>/.well-known/k2-pair#<CODE>`), and K2 serves
   that host's iOS and Android link files naming *your* app, so the phone
   opens your app, not a browser.
3. Your app sends the code to K2 and gets exactly what a device sign-in
   gets: a pass and a rotating refresh token, for that App and that person.
   In the same call it can register for your App's push notifications.

No password is typed on the phone. The short version is
`k2 study app-pairing`.

## The rules K2 keeps

- A code (`K2A-XXXXX-XXXXX-XXXXX-XXXXX`) works **once**, for **5 minutes**,
  for **your App only**, and only while the person's web sign-in that made
  it is still live. Logging out, a password change or reset, losing access
  to the App, or the owner turning pairing off kills it.
- The code rides in the URL **fragment**, which no browser or OS sends to a
  server: it never reaches a log on the way.
- The link names no server. Your app already knows its box (the base URL it
  uses for `/cli/skin/login`) and redeems there. A QR that said where to
  redeem would let someone else's QR point your app at their server.
- A phone that is signed in can't pair another phone, and a web sign-in
  older than 15 minutes must type its password again to start.
- What a paired phone gets is an ordinary device sign-in: the App's "Stay
  signed in" length (30 days by default, sliding), the 20-device limit, and
  it ends on a password change, when the person loses the App, or when they
  end it from the web App (`guests.revokeDevice`). The App's "Stay signed
  in" must be on: with it off, a start answers `device_signin_off`.

## 1. Once, as the App owner

v1 pairs on your App's own **k2.dev labels**: its label
(`<app>.<you>.k2.dev`) and any labels you took or attached
(`k2 app label take`, `k2 app domain add`). A custom domain works only when
you front it to the label yourself (a CDN or your own proxy whose origin is
the label).

### iOS

1. In your Apple developer account, add the **Associated Domains**
   capability to your app id.
2. In Xcode, add `applinks:<host>` for **each** host, exactly:
   `applinks:jobs.you.k2.dev`. Never a wildcard (`applinks:*.you.k2.dev`):
   a wildcard makes iOS read the root domain's file, which names K2's own
   app, so yours never opens.

### Android

1. In your manifest, an intent filter with `android:autoVerify="true"`:

   ```xml
   <intent-filter android:autoVerify="true">
     <action android:name="android.intent.action.VIEW" />
     <category android:name="android.intent.category.DEFAULT" />
     <category android:name="android.intent.category.BROWSABLE" />
     <data android:scheme="https" android:host="jobs.you.k2.dev"
           android:path="/.well-known/k2-pair" />
   </intent-filter>
   ```

2. Get your signing certificate's SHA-256 (`keytool -list -v -keystore …`),
   and Play App Signing's from the Play Console. List both.

### Tell K2 (from a terminal outside K2; Touch ID, your login or the vault password confirms it)

```sh
k2 app pairing setup <app> \
  --ios TEAMID.com.you.app \
  --android com.you.app:AB:CD:…:EF,12:34:…:56
k2 app pairing check <app>
```

- `--native-origin capacitor://localhost` (or `tauri://localhost`) if your
  phone app is a webview app that sends an `Origin` header. Without it, K2
  refuses a redeem that looks like it came from a web page
  (`pairing_not_from_browser`).
- While pairing is on, K2 itself answers
  `/.well-known/apple-app-site-association` and
  `/.well-known/assetlinks.json` on your App's hosts. If your App serves its
  own files there today, list its other universal-link paths with
  `--link-path '/jobs/*'` so they keep working.
- `--app-store` / `--play-store` add store links to the landing page a
  browser shows if the app isn't installed.
- Apple's CDN fetches a new host's file within about a day, and phones
  re-check about weekly; Android 15+ re-verifies within days, older Android
  at install. After you change the app ids, new QRs may open the old app
  for up to a week. `k2 app pairing check` shows what Apple's CDN holds.

Only the owner, in person, sets these ids: they decide which phone app
receives sign-in codes. The App's agent can read the status and turn
pairing off, never set it.

## 2. In the web App

The person's own session pass starts it (on the App's link or the main
port). A reference "Connect your phone" component:

```js
// pass: the signed-in person's session pass (k2skn_…).
async function connectPhone(k2, pass, el, askPassword) {
  let p
  try {
    p = await k2.asGuest(pass).pairing.start()
  } catch (e) {
    if (e.code !== 'reauth_required') throw e
    p = await k2.asGuest(pass).pairing.start({ password: await askPassword() })
  }
  el.innerHTML = qrSvg(p.qr.rows, { size: 240 }) // from '@k2/server', or draw p.qr.rows yourself
  const until = Date.now() + p.expiresInSecs * 1000
  for (;;) {
    await new Promise((r) => setTimeout(r, 2000))
    const s = await k2.asGuest(pass).pairing.status({ pairingId: p.pairingId })
    if (s.state === 'paired') {
      el.textContent = `Paired with ${s.device.name}. Not you? `
      // a button calling k2.asGuest(pass).guests.revokeDevice({ id: s.device.deviceLoginId })
      return s
    }
    if (s.state !== 'pending' || Date.now() > until) return connectPhone(k2, pass, el, askPassword)
  }
}
```

- `qr.rows` are strings of `1` (dark) and `0` (light), without the quiet
  zone. `qrSvg` (in `@k2/server`) draws them; in a browser, the same few
  lines draw each `1` as a 1×1 square inside a 4-module border.
- The start's answer holds the code: send it with `Cache-Control: no-store`
  (K2 does) and never log it.
- A device name proves nothing on its own: anyone can call their phone
  "Rosson's iPhone". The signal is that the person's own phone fails when
  someone else got there first. Always offer "Not you? End it".

## 3. In the phone app

Plain HTTPS to your box: the base URL your app already uses for
`/cli/skin/login` (`https://<you>.k2.dev`), not the App's label (that one
is your App).

1. Handle the universal link / App Link
   `https://<host>/.well-known/k2-pair#<CODE>` and read the code from the
   fragment. Also offer **Paste code** for the landing page's copy button.
2. `POST /cli/skin/pair/check {code, app}` and show **"Sign in to <appName>
   as <username>?"**. This is required: a QR someone else made would
   otherwise sign the phone in as them.
3. On yes: `POST /cli/skin/pair {code, app, device: {id, name}, push:
   {platform, pushToken, environment}}`. Store `token` and `refresh` in the
   Keychain / Keystore. Use the same `device.id` you use for push.
4. Refresh with `POST /cli/skin/refresh {refresh, app}` exactly as after a
   device login. Sign out with `POST /cli/skin/logout`.

A redeem through a BFF goes through the App's gateway link (its own
limits). On the main port every phone a BFF redeems for would share the
BFF's address limit.

### Swift

```swift
struct Check: Decodable { let appName: String; let username: String }
struct Paired: Decodable { let token: String; let refresh: String }

func code(from url: URL) -> String? {
    guard url.path == "/.well-known/k2-pair", let f = url.fragment, !f.isEmpty else { return nil }
    return f
}

func post<T: Decodable>(_ path: String, _ body: [String: Any]) async throws -> T {
    var req = URLRequest(url: URL(string: "https://you.k2.dev" + path)!) // your box (where /cli/skin/login goes), not the App label
    req.httpMethod = "POST"
    req.setValue("application/json", forHTTPHeaderField: "Content-Type")
    req.httpBody = try JSONSerialization.data(withJSONObject: body)
    let (data, resp) = try await URLSession.shared.data(for: req)
    guard (resp as? HTTPURLResponse)?.statusCode == 200 else { throw URLError(.userAuthenticationRequired) }
    return try JSONDecoder().decode(T.self, from: data)
}

// After the person said yes to "Sign in to \(check.appName) as \(check.username)?":
let check: Check = try await post("/cli/skin/pair/check", ["code": code, "app": appId])
let paired: Paired = try await post("/cli/skin/pair", [
    "code": code, "app": appId,
    "device": ["id": installId, "name": UIDevice.current.name],
    "push": ["platform": "apns", "pushToken": apnsHex, "environment": "production"],
])
// Keychain: paired.token, paired.refresh
```

### Kotlin

```kotlin
fun codeFrom(uri: Uri): String? =
    if (uri.path == "/.well-known/k2-pair") uri.fragment?.takeIf { it.isNotEmpty() } else null

suspend fun post(path: String, body: JSONObject): JSONObject = withContext(Dispatchers.IO) {
    val c = URL("https://you.k2.dev$path").openConnection() as HttpURLConnection // your box, not the App label
    c.requestMethod = "POST"
    c.setRequestProperty("Content-Type", "application/json")
    c.doOutput = true
    c.outputStream.use { it.write(body.toString().toByteArray()) }
    if (c.responseCode != 200) throw IOException("pairing refused: ${c.responseCode}")
    JSONObject(c.inputStream.bufferedReader().readText())
}

val check = post("/cli/skin/pair/check", JSONObject().put("code", code).put("app", appId))
// Ask: "Sign in to ${check.getString("appName")} as ${check.getString("username")}?"
val paired = post("/cli/skin/pair", JSONObject()
    .put("code", code).put("app", appId)
    .put("device", JSONObject().put("id", installId).put("name", Build.MODEL))
    .put("push", JSONObject().put("platform", "fcm").put("pushToken", fcmToken)))
// EncryptedSharedPreferences / Keystore: paired.getString("token"), paired.getString("refresh")
```

## Refusals

| Code | When |
|---|---|
| `pairing_off` | the App doesn't pair phones (the owner turns it on with `k2 app pairing setup`) |
| `device_signin_off` | the App's "Stay signed in" is 0 |
| `reauth_required` | the web sign-in is older than 15 minutes: send `password` with the start |
| `bad_login`, `locked` | a wrong password at start (it counts toward the person's lockout) |
| `pairing_from_device` | a phone's pass tried to start |
| `host_not_paired`, `no_public_host` | a `host` the App doesn't pair on, or no public host at all |
| `pairing_not_served` | K2 can't serve the App's link files (a `--cmd` or `--skin` service, or `host.clientIp: false`) |
| `pair_code_invalid` | unknown, used, expired, cancelled, another App's, or the web sign-in ended |
| `pairing_not_from_browser` | a web page tried to redeem (list your webview's Origin with `--native-origin`) |
| `rate_limited` | too many tries (`retryAfterMs`) |

`K2_APP_PAIRING=off` in the daemon's environment stops every App's pairing,
including the link files. K2 Companion's own phones pair with
`k2 users pair-phone`, not this.
