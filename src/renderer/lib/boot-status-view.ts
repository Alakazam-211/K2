// `/boot-status` views (PRD boot-status-ghost, 0.46.3).
//
// One route, three bodies. A 0.46.3+ daemon stamps `view`:
//   - `full`   — loopback, or a valid credential (owner token, Connect-login
//                session, agent passport, `k2sk_` owner/status key): the old
//                body plus `view` + `protocolMin`;
//   - `compat` — a stranger while the box is in compat mode (the 0.46.3
//                default): phase, protocol range, instanceId + version,
//                features, serverId, publicHost, airgap.enabled;
//   - `public` — a stranger in ghost mode: phase, protocol range, instanceId.
// A ≤0.46.2 daemon has no `view` and always answers its full body.
//
// Clients send their token for a REMOTE host (`?token=`, like every remote
// `/cli` call) so they get the full body, and a partial body must never
// clear what an earlier full body taught (B3): it only updates `phase` and
// `instanceId`, plus any field it does carry.

/** The client↔daemon contract this client speaks (`boot_status::PROTOCOL`). */
export const CLIENT_PROTOCOL = 1

export type BootView = 'public' | 'compat' | 'full'

/** The fields every view may carry; all optional (old daemons, partial views). */
export interface BootStatusLike {
  view?: unknown
  version?: unknown
  protocol?: unknown
  protocolMin?: unknown
  features?: unknown
  airgap?: { enabled?: unknown } | null
}

/** The body's `view`, or `undefined` for a ≤0.46.2 daemon (always full). */
export function bootViewOf(body: BootStatusLike | null | undefined): BootView | undefined {
  const v = body?.view
  return v === 'public' || v === 'compat' || v === 'full' ? v : undefined
}

/** A stranger's body (`public` / `compat`): must never clear known
 *  version / features / serverId / airgap (absent ≠ unsupported). */
export function isPartialBootView(body: BootStatusLike | null | undefined): boolean {
  const v = bootViewOf(body)
  return v === 'public' || v === 'compat'
}

/** We sent a token and the daemon still answered a stranger's view: the
 *  token was not accepted (a stale session after a restart). Never "down";
 *  start the session-revive check (B4). An old daemon (no `view`) is not
 *  this signal. */
export function tokenNotAccepted(tokenSent: boolean, body: BootStatusLike | null | undefined): boolean {
  return tokenSent && isPartialBootView(body)
}

/** `<base>/boot-status`, with `?token=` when a token is given. */
export function bootStatusUrl(base: string, token?: string | null): string {
  const url = `${base}/boot-status`
  return token ? `${url}?token=${encodeURIComponent(token)}` : url
}

/** A ≤0.46.2 daemon 403s a must-change-password session on every path,
 *  `/boot-status` included: retry once without the token. */
export function shouldRetryWithoutToken(tokenSent: boolean, httpStatus: number): boolean {
  return tokenSent && (httpStatus === 401 || httpStatus === 403)
}

/** Is this client inside the daemon's `[protocolMin, protocol]` range?
 *  A daemon without `protocolMin` (≤0.46.2) serves protocol 1 and up. */
export function protocolInRange(body: BootStatusLike): boolean {
  const max = typeof body.protocol === 'number' ? body.protocol : CLIENT_PROTOCOL
  const min = typeof body.protocolMin === 'number' ? body.protocolMin : 1
  return CLIENT_PROTOCOL >= min && CLIENT_PROTOCOL <= max
}

/**
 * The version to cache for a remote server from one accepted body (no
 * clobber): the body's own `version` when it has one; else, for a partial
 * view, the version already known; else (an old daemon that omits it)
 * `null`, as before.
 */
export function nextKnownVersion(
  body: BootStatusLike | null | undefined,
  known: string | null,
): string | null {
  if (typeof body?.version === 'string' && body.version) return body.version
  if (isPartialBootView(body)) return known
  return null
}

/**
 * Hosted web (§3.1): a server in ghost mode hides its version from a
 * signed-out browser, so the loader started the newest published bundle
 * (`app/current.json`) as the sign-in shell. Once signed in (a `full` body
 * naming the server's own version), reload ONCE onto that version through
 * the loader's `?v=` override. Returns the URL, or null to stay.
 */
export function webBundleReloadUrl(args: {
  isWeb: boolean
  body: BootStatusLike | null | undefined
  bundleVersion: string | undefined
  pathname: string
  alreadyTried: boolean
}): string | null {
  const { isWeb, body, bundleVersion, pathname, alreadyTried } = args
  if (!isWeb || alreadyTried || !bundleVersion) return null
  // Only a loader-started bundle (`/app/<ver>/…`), never the dev server.
  if (!pathname.startsWith('/app/')) return null
  if (bootViewOf(body) !== 'full') return null
  const v = body?.version
  if (typeof v !== 'string' || !/^\d+\.\d+\.\d+[a-zA-Z0-9.+-]*$/.test(v)) return null
  if (v === bundleVersion) return null
  return `/?v=${encodeURIComponent(v)}`
}

/** The airgap flag to apply, or `undefined` to keep the current one (a
 *  partial view without `airgap`). */
export function nextAirgap(body: BootStatusLike | null | undefined): boolean | undefined {
  if (body?.airgap && typeof body.airgap === 'object') return body.airgap.enabled === true
  if (isPartialBootView(body)) return undefined
  return false
}
