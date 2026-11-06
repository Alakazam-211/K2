# Your own sign-in page, passwords straight to K2 (K2 Apps)

This guide is for the developer of a custom K2 App that wants its own,
white-labeled sign-in page. Since K2 0.46.9 the page can send the password
straight to K2 instead of to your server. The short version is
`k2 study auth-pages`.

1. Your server makes a PKCE pair for this attempt (`k2.auth.begin()`
   returns `{challenge, verifier, setCookie}`). It keeps the **verifier**
   and gives the page only the **challenge**.
2. The page posts `{username, password, challenge}` to
   `/.k2/auth/sign-in` on **your App's own host**. K2 answers that path
   before your App's server sees a byte of it, and returns a one-time code.
3. The page hands the code to your server. Your server swaps it, with the
   verifier, for the person's pass (`k2.auth.exchange`) and sets its own
   cookie.

Your server never sees the password. The browser never holds a K2 pass or
a K2 cookie, and the code is useless without the verifier your server kept.
K2 serves no pages: the page is yours.

## Turn it on

The door starts off. The owner turns it on for the server:

```sh
k2 app auth door on
k2 app auth <app>          # is it on for this App? which hosts?
```

`k2 app auth door off <app>` hands `/.k2/auth/*` back to one App that
serves that path itself.

## Where the door is

- **Yes:** the App's K2 label (`<app>.<box>.k2.dev`, with the default
  client-IP front), every custom domain on the App, and its local front port.
- **No:** a label whose manifest says `host.clientIp: false`, a `--cmd`
  service, a name served by your own web server, or a CDN in front that
  doesn't pass the path on. There `GET /.k2/auth/ping` gets your own App's
  answer, not `{"k2": true}`: send no password.

## A minimal page

Your server renders this with the challenge from `k2.auth.begin()` (and sets
the cookie `begin` returns, which keeps the verifier):

```html
<form id="f">
  <input name="username" autocomplete="username" required>
  <input name="password" type="password" autocomplete="current-password" required>
  <button>Sign in</button>
  <p id="msg" role="alert"></p>
</form>
<script>
  const CHALLENGE = '{{challenge}}'  // from k2.auth.begin(), new for every attempt
  const f = document.getElementById('f')
  const msg = document.getElementById('msg')
  f.addEventListener('submit', async (e) => {
    e.preventDefault()
    // No door on this host: never send the password anywhere.
    const ping = await fetch('/.k2/auth/ping').then((r) => r.json()).catch(() => null)
    if (!ping || ping.k2 !== true) { msg.textContent = 'Sign-in is not available here.'; return }
    const r = await fetch('/.k2/auth/sign-in', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ username: f.username.value, password: f.password.value, challenge: CHALLENGE }),
    })
    const a = await r.json()
    if (a.ok) {
      // Only the code goes to your own server.
      await fetch('/session', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code: a.code }) })
      location.href = '/'
      return
    }
    // The words are yours. a.error.code: bad_login, locked, rate_limited,
    // password_change_required (a.changeToken, at the top level), bad_origin, ...
    // A refused attempt uses nothing: retry with the same CHALLENGE.
    msg.textContent = a.error.code === 'locked' ? 'Too many tries. Wait a few minutes.' : 'That did not work.'
  })
</script>
```

And the two server routes (Node, `@k2/server`):

```js
import { fromEnv } from '@k2/server'
const k2 = fromEnv()

// GET /sign-in
const s = k2.auth.begin()
res.setHeader('set-cookie', s.setCookie)
render('sign-in.html', { challenge: s.challenge })

// POST /session {code}
const verifier = k2.auth.verifierFrom(req)
const session = await k2.auth.exchange({ code: body.code, verifier })
res.setHeader('set-cookie', [k2.auth.clearCookie(), yourOwnSessionCookie(session.pass)])
```

Without the SDK: `POST /cli/skin/auth/exchange {code, verifier}` on your
gateway link with the App pass, or `POST /v1/apps/<service id>/auth/exchange`
with an App key (scope `signin`, or `users`). A wrong, used, expired or
other App's code is `401 bad_code`.

## First passwords, invites and resets

- `sign-in` answers `403 password_change_required` with `changeToken` (at
  the top level of the answer) when someone else set the password. Show
  your new-password form and post `{token: changeToken, password,
  challenge}` to `/.k2/auth/set-password`, with the same challenge. The
  answer has a code: no second typing.
- An invite or reset link brings `#k2inv_…` to your page (set it with
  `k2 app login settings <app> --set-password-url`). Send the token without
  the `#`, the same way. With a challenge the answer has a code (unless the
  person can't use this App: then the password is set and no code comes);
  without one, no code, and the person signs in next. The token must be
  this App's, and the owner's own reset never works at an App's door.
- `/.k2/auth/change-password` takes `{username, oldPassword, password,
  challenge}` and returns a code. The person's other sessions end.

## Phones and desktop shells

A phone app or a Tauri window can post with native HTTP to
`https://<your App's host>/.k2/auth/*` (no `Origin`, no CORS needed). Your
server keeps `s.verifier` from `k2.auth.begin()` against a one-time id it
hands the app with the challenge. A Tauri window whose bundled pages call
`fetch` sends `Origin: tauri://localhost`: list it with
`k2 app auth origins <app> --origin tauri://localhost` (each call replaces
the list). A listed origin gets CORS on every verb and on ping.

## Handling the password on the page

- Use a real `<form>` with `autocomplete` set, so password managers work.
- Post straight to the door. Don't log the password, keep it, or send it to
  your server.
- No analytics, session replay or third-party scripts on the sign-in page.
  Keep your service worker off `/.k2/`.
- The door keeps the password from your server, not from scripts on your
  own page.
- Your own form that posts to your own server (which calls `k2.login`)
  stays allowed. Then your server sees passwords and must never log them.

Enterprise single sign-on (OIDC, SAML) is not part of the door yet.
