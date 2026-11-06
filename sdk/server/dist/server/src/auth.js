// The sign-in door's server half (K2 0.46.9, prd-app-hosted-sign-in-v1).
//
// Your page posts the password straight to K2 at /.k2/auth/* on your own
// host; K2 answers with a one-time code. Your server only ever sees that
// code, and swaps it (with the PKCE verifier it kept) for the person's pass:
//
//   // 1. Before the form: a fresh pair per attempt. The verifier stays
//   //    with your server (an HttpOnly cookie here), the page gets only the
//   //    challenge.
//   const s = k2.auth.begin()
//   res.setHeader('set-cookie', s.setCookie)
//   render(page, { challenge: s.challenge })
//
//   // 2. The page posted to /.k2/auth/sign-in and sent you the code:
//   const verifier = k2.auth.verifierFrom(req)
//   const session = await k2.auth.exchange({ code, verifier })
//   res.setHeader('set-cookie', [k2.auth.clearCookie(), yourOwnSessionCookie(session.pass)])
//
// A phone app has no cookie: keep the verifier against a one-time id your
// server hands the app with the challenge. Never share one verifier
// between attempts or people: a fixed challenge removes the protection.
import { createHash, randomBytes } from 'node:crypto';
/** The cookie `begin` puts the verifier in (HttpOnly, 5 minutes). */
export const SIGN_IN_COOKIE = 'k2_signin';
function b64url(b) {
    return b.toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
/** A fresh PKCE pair (32 random bytes; S256). */
export function pkce() {
    const verifier = b64url(randomBytes(32));
    const challenge = b64url(createHash('sha256').update(verifier).digest());
    return { verifier, challenge };
}
function cookieValue(name, value, secure, maxAge) {
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(name))
        throw new Error('cookie names are letters, digits, _ and -');
    return `${name}=${value}; Path=/; HttpOnly; SameSite=Lax${secure ? '; Secure' : ''}; Max-Age=${maxAge}`;
}
/** Start one sign-in attempt: a new pair and the cookie that keeps its verifier. */
export function beginSignIn(opts = {}) {
    const p = pkce();
    const maxAge = opts.maxAgeSecs ?? 300;
    if (!Number.isInteger(maxAge) || maxAge < 1 || maxAge > 3600)
        throw new Error('maxAgeSecs is a whole number from 1 to 3600');
    return { ...p, setCookie: cookieValue(opts.cookie ?? SIGN_IN_COOKIE, p.verifier, opts.secure ?? true, maxAge) };
}
/** A `Set-Cookie` value that removes the verifier cookie (send it with your answer to the exchange). */
export function clearSignInCookie(cookie = SIGN_IN_COOKIE, secure = true) {
    return cookieValue(cookie, '', secure, 0);
}
/** Read the verifier from a request's `cookie` header value; null when absent or malformed. */
export function verifierFromCookieHeader(raw, cookie = SIGN_IN_COOKIE) {
    if (!raw)
        return null;
    for (const part of raw.split(';')) {
        const [k, ...rest] = part.trim().split('=');
        const v = rest.join('=');
        if (k === cookie && /^[A-Za-z0-9._~-]{43,128}$/.test(v))
            return v;
    }
    return null;
}
