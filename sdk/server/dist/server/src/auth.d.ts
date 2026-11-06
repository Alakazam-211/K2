/** The cookie `begin` puts the verifier in (HttpOnly, 5 minutes). */
export declare const SIGN_IN_COOKIE = "k2_signin";
/** A PKCE pair for one sign-in attempt. */
export interface Pkce {
    /** Keep this on your server (never send it to the page). */
    verifier: string;
    /** Give this to the page: it posts it with the password. S256, 43 characters. */
    challenge: string;
}
/** What `k2.auth.begin()` returns. */
export interface SignInStart extends Pkce {
    /** A `Set-Cookie` value holding the verifier: HttpOnly, SameSite=Lax, Secure, 5 minutes. */
    setCookie: string;
}
/** `begin`'s options. */
export interface BeginOptions {
    /** The cookie's name (default `k2_signin`). */
    cookie?: string;
    /** Mark it Secure (default true; false only for a plain-http local test). */
    secure?: boolean;
    /** Its lifetime in seconds (default 300). */
    maxAgeSecs?: number;
}
/** A fresh PKCE pair (32 random bytes; S256). */
export declare function pkce(): Pkce;
/** Start one sign-in attempt: a new pair and the cookie that keeps its verifier. */
export declare function beginSignIn(opts?: BeginOptions): SignInStart;
/** A `Set-Cookie` value that removes the verifier cookie (send it with your answer to the exchange). */
export declare function clearSignInCookie(cookie?: string, secure?: boolean): string;
/** Read the verifier from a request's `cookie` header value; null when absent or malformed. */
export declare function verifierFromCookieHeader(raw: string | null | undefined, cookie?: string): string | null;
