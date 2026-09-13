// Login URL rule for K2 Connect password sign-in (PRD connect-login-edge-only
// §4.3 D1/D2/D4).
//
// The daemon no longer answers `POST /cli/auth/login` on public tunnel
// ingress unless the request carries a K2 first-party edge attestation. The
// only first-party edge in front of a hosted tunnel is the `*.app.k2.dev`
// Worker, so a client that wants to password-sign-in to `https://<label>.k2.dev`
// must POST the login to `https://<label>.app.k2.dev/cli/auth/login` instead.
//
// Nothing else moves host: token-bearing traffic, WebSocket upgrades, the
// whoami probes, and the whole data plane stay on `https://<label>.k2.dev`.
// Every login call site (loginToHost — which the switcher's pickHost,
// Settings → Connections add-server, RemoteSignIn, and the silent revive all
// funnel through) MUST build its URL with {@link loginUrlFor}.

/**
 * Only an APEX label under `k2.dev` is a hosted tunnel host: exactly one
 * DNS label, `[a-z0-9-]`, then `.k2.dev`, no port, no path. Nested labels
 * (`rosson.app.k2.dev`, `a.b.k2.dev`), custom domains, LAN `ip:port`, plain
 * http and any explicit port are all "everything else".
 */
const K2_DEV_APEX_HTTPS = /^https:\/\/([a-z0-9-]+)\.k2\.dev$/i

/**
 * Labels that are reserved for the edge itself and never a tunnel
 * subdomain. `app.k2.dev` is the Worker's own apex; mapping it would yield
 * `app.app.k2.dev`. Kept as a set so ops can grow it without touching the
 * rule.
 */
const RESERVED_EDGE_LABELS: ReadonlySet<string> = new Set(['app', 'www'])

/**
 * The tunnel label when `hostBaseUrl` is an apex `https://<label>.k2.dev`
 * host, else null. Case-insensitive on the host; the returned label is
 * lowercased (DNS labels are case-insensitive, and the Worker signs the
 * lowercase `sub`).
 */
export function k2DevApexLabel(hostBaseUrl: string): string | null {
  const m = K2_DEV_APEX_HTTPS.exec(hostBaseUrl.trim().replace(/\/+$/, ''))
  if (!m) return null
  const label = m[1].toLowerCase()
  if (RESERVED_EDGE_LABELS.has(label)) return null
  return label
}

/**
 * Where a client must POST `/cli/auth/login` for `hostBaseUrl`
 * (`<scheme>://<host>[:<port>]`, as built by connect-host's `hostBaseUrl`).
 *
 *   https://rosson.k2.dev        → https://rosson.app.k2.dev/cli/auth/login
 *   https://ROSSON.K2.DEV        → https://rosson.app.k2.dev/cli/auth/login
 *   https://rosson.app.k2.dev    → https://rosson.app.k2.dev/cli/auth/login  (already the edge)
 *   https://a.b.k2.dev           → https://a.b.k2.dev/cli/auth/login          (nested: not a tunnel apex)
 *   http://192.168.1.50:60710    → http://192.168.1.50:60710/cli/auth/login   (LAN / self-host)
 *   http://rosson.k2.dev         → http://rosson.k2.dev/cli/auth/login        (plain http: unchanged)
 *   https://k2.example.com       → https://k2.example.com/cli/auth/login      (custom domain)
 *
 * Pure; no I/O. The first branch is the ONLY host change this rule makes.
 */
export function loginUrlFor(hostBaseUrl: string): string {
  const base = hostBaseUrl.trim().replace(/\/+$/, '')
  const label = k2DevApexLabel(base)
  if (label) return `https://${label}.app.k2.dev/cli/auth/login`
  return `${base}/cli/auth/login`
}

/**
 * D3 copy: a `404` from login on a hosted `.k2.dev` host means the daemon
 * refused password sign-in on this ingress (edge-only gate) and the client
 * did not reach it through the edge — or the edge is not yet signing. Either
 * way the fix is "update both sides", never "retry".
 */
export const LOGIN_404_K2DEV_MESSAGE =
  "Sign-in through this server's web address failed (404). Make sure both K2 and the server are up to date."
