// Host keys — the one spelling of "which server" shared by Home rows
// (`handle::host`, lib/home-address.ts) and ServerScope (kessel/server-scope.ts).
//
// Canonical form (Home M1, MS8 + MS60):
//   - `local` for the daemon on the computer running this window;
//   - `https` on 443: the bare lowercase hostname (`dtl.k2.dev`);
//   - anything else carries its port, plain `http` on 80 included
//     (`192.168.1.20:38471`, `box.lan:80`), so `http://x` and `https://x`
//     never share a key;
//   - an IPv6 literal is always `[addr]:port`.
// `canonicalHostKey(string)` parses any spelling (`https://DTL.k2.dev/`,
// `dtl.k2.dev:443`, `dtl.k2.dev`) and re-emits the canonical key, or throws.
//
// Kept free of app imports so the request layer can use it without an
// import cycle.

import type { ConnectHost } from '@/stores/connect-host'

export const LOCAL_HOME_HOST = 'local'

function stripBrackets(hostname: string): string {
  const h = hostname.trim().toLowerCase()
  return h.startsWith('[') && h.endsWith(']') ? h.slice(1, -1) : h
}

function isIpv6(hostname: string): boolean {
  return hostname.includes(':')
}

/** Host key for `local` or a saved server. */
export function homeHostKey(
  h: 'local' | Pick<ConnectHost, 'hostname' | 'port' | 'secure'>,
): string {
  if (h === 'local') return LOCAL_HOME_HOST
  const hostname = stripBrackets(h.hostname)
  if (isIpv6(hostname)) return `[${hostname}]:${h.port}`
  if (h.secure && h.port === 443) return hostname
  return `${hostname}:${h.port}`
}

const HOSTNAME_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)*$/
const IPV6_RE = /^[0-9a-f:.]+$/

function parsePort(raw: string, input: string): number {
  if (!/^\d{1,5}$/.test(raw)) throw new Error(`not a host key: ${JSON.stringify(input)}`)
  const port = Number(raw)
  if (port < 1 || port > 65535) throw new Error(`not a host key: ${JSON.stringify(input)}`)
  return port
}

/** Parse any spelling of a server address into its canonical host key.
 *  Throws on anything that is not `local`, a hostname, an IPv4/IPv6
 *  literal, optionally with a scheme, a port and a trailing `/`. */
export function canonicalHostKey(input: string): string {
  let s = input.trim().toLowerCase()
  if (s === LOCAL_HOME_HOST) return LOCAL_HOME_HOST
  let secure: boolean | null = null
  const scheme = /^(https|wss|http|ws):\/\//.exec(s)
  if (scheme) {
    secure = scheme[1] === 'https' || scheme[1] === 'wss'
    s = s.slice(scheme[0].length)
  }
  if (s.endsWith('/')) s = s.slice(0, -1)
  if (s.length === 0 || s.includes('/') || s.includes('@') || s.includes('?') || s.includes('#')) {
    throw new Error(`not a host key: ${JSON.stringify(input)}`)
  }
  let hostname: string
  let port: number | null = null
  if (s.startsWith('[')) {
    const close = s.indexOf(']')
    if (close < 0) throw new Error(`not a host key: ${JSON.stringify(input)}`)
    hostname = s.slice(1, close)
    const rest = s.slice(close + 1)
    if (rest.length > 0) {
      if (!rest.startsWith(':')) throw new Error(`not a host key: ${JSON.stringify(input)}`)
      port = parsePort(rest.slice(1), input)
    }
    if (!IPV6_RE.test(hostname) || !hostname.includes(':')) {
      throw new Error(`not a host key: ${JSON.stringify(input)}`)
    }
  } else {
    const colon = s.lastIndexOf(':')
    if (colon >= 0) {
      if (s.indexOf(':') !== colon) {
        // Unbracketed IPv6 literal — only valid without a port.
        if (!IPV6_RE.test(s)) throw new Error(`not a host key: ${JSON.stringify(input)}`)
        hostname = s
      } else {
        hostname = s.slice(0, colon)
        port = parsePort(s.slice(colon + 1), input)
      }
    } else {
      hostname = s
    }
    if (!isIpv6(hostname) && !HOSTNAME_RE.test(hostname)) {
      throw new Error(`not a host key: ${JSON.stringify(input)}`)
    }
  }
  // No scheme: a missing port or 443 means the default https server (the
  // spelling Home rows and the switcher use for `<sub>.k2.dev`).
  const isSecure = secure === null ? port === null || port === 443 : secure
  const effectivePort = port === null ? (isSecure ? 443 : 80) : port
  return homeHostKey({ hostname, port: effectivePort, secure: isSecure })
}

/** The saved server a host key points at. Several saved entries can share
 *  an address; prefer the one holding a login. */
export function savedHostForKey(hosts: ConnectHost[], hostKey: string): ConnectHost | null {
  const matches = hosts.filter((h) => homeHostKey(h) === hostKey)
  if (matches.length === 0) return null
  return matches.find((h) => h.token.length > 0) ?? matches[0]
}
