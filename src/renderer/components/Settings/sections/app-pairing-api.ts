// Settings → Apps → <App> → Phones (prd-app-phone-pairing-v1 §8.6, 0.46.5):
// typed bindings for `/cli/app/pairing*`. Thin client: the daemon owns the
// config, which hosts pair, the link files and every rule; this file parses
// its answers and sends the owner's gestures. Setting the phone apps takes a
// fresh vault owner confirm (action `app-pairing-apps`, bound to the App's
// service id) through `vaultApi.confirmStart/confirmFinish`, like approvals.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

/** The vault confirm action the daemon checks on set. */
export const APP_PAIRING_CONFIRM_ACTION = 'app-pairing-apps'

export type PairingHost = { host: string; pairs: boolean }
export type AndroidApp = { package: string; sha256: string[] }
export type HostCheck = { host: string; aasa: string; assetlinks: string; appleCdn: string }

export type PairingStatus = {
  app: string
  name: string
  on: boolean
  /** Why it is off (`owner_off`, `not_set_up`, `box_off`), else null. */
  off: string | null
  /** K2 can serve the App's link files (a runtime App with K2's front). */
  served: boolean
  notServed: string | null
  staySignedInDays: number
  hosts: PairingHost[]
  ios: string[]
  android: AndroidApp[]
  nativeOrigins: string[]
  linkPaths: string[]
  storeIos: string | null
  storeAndroid: string | null
  pairedPhones: number
  pairingsLastDay: number
  lastCheck: HostCheck[] | null
  warnings: string[]
}

/** What the Setup sheet sends (the daemon checks every field). */
export type PairingSetup = {
  ios: string[]
  android: AndroidApp[]
  nativeOrigins: string[]
  linkPaths: string[]
  storeIos: string
  storeAndroid: string
}

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}
function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}
function strs(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
}
function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}

function parseChecks(v: unknown): HostCheck[] {
  return (Array.isArray(v) ? v : []).map((h) => {
    const x = rec(h)
    return { host: str(x.host), aasa: str(x.aasa), assetlinks: str(x.assetlinks), appleCdn: str(x.appleCdn) }
  })
}

export function parsePairing(raw: unknown): PairingStatus {
  const r = rec(raw)
  const store = rec(r.store)
  const last = r.lastCheck == null ? null : parseChecks(rec(r.lastCheck).hosts)
  return {
    app: str(r.app),
    name: str(r.name) || str(r.app),
    on: r.state === 'on',
    off: str(r.off) || null,
    served: r.served === true,
    notServed: str(r.notServed) || null,
    staySignedInDays: num(r.staySignedInDays),
    hosts: (Array.isArray(r.hosts) ? r.hosts : []).map((h) => {
      const x = rec(h)
      return { host: str(x.host), pairs: x.pairs === true }
    }),
    ios: strs(r.ios),
    android: (Array.isArray(r.android) ? r.android : []).map((a) => {
      const x = rec(a)
      return { package: str(x.package), sha256: strs(x.sha256) }
    }),
    nativeOrigins: strs(r.nativeOrigins),
    linkPaths: strs(r.linkPaths),
    storeIos: str(store.ios) || null,
    storeAndroid: str(store.android) || null,
    pairedPhones: num(r.pairedPhones),
    pairingsLastDay: num(r.pairingsLastDay),
    lastCheck: last,
    warnings: strs(r.warnings),
  }
}

/** Lines of a textarea: trimmed, empty ones dropped. */
export function lines(text: string): string[] {
  return text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s !== '')
}

/**
 * Android apps from the sheet, one per line: `com.you.app AB:CD:…[ AB:CD:…]`
 * (fingerprints separated by spaces or commas). The daemon checks shapes;
 * this only splits.
 */
export function parseAndroidLines(text: string): AndroidApp[] {
  return text
    .split('\n')
    .map((l) => l.trim())
    .filter((l) => l !== '')
    .map((l) => {
      const [pkg, ...rest] = l.split(/[\s,]+/)
      return { package: pkg ?? '', sha256: rest.filter((s) => s !== '') }
    })
}

/** The set body for `setup` (fields left empty are left out). */
export function setupBody(app: string, s: PairingSetup, confirmToken: string): Record<string, unknown> {
  const body: Record<string, unknown> = { app, confirmToken }
  if (s.ios.length) body.ios = s.ios
  if (s.android.length) body.android = s.android
  if (s.nativeOrigins.length) body.nativeOrigins = s.nativeOrigins
  if (s.linkPaths.length) body.linkPaths = s.linkPaths
  const store: Record<string, string> = {}
  if (s.storeIos.trim()) store.ios = s.storeIos.trim()
  if (s.storeAndroid.trim()) store.android = s.storeAndroid.trim()
  if (Object.keys(store).length) body.store = store
  return body
}

export async function fetchPairing(app: string): Promise<PairingStatus> {
  return parsePairing(await daemonCliGet<unknown>(primaryScope(), 'app/pairing', { app }))
}

export async function setupPairing(app: string, s: PairingSetup, confirmToken: string): Promise<PairingStatus> {
  return parsePairing(await daemonCliPost<unknown>(primaryScope(), 'app/pairing/set', setupBody(app, s, confirmToken)))
}

export async function turnPairingOff(app: string): Promise<PairingStatus> {
  return parsePairing(await daemonCliPost<unknown>(primaryScope(), 'app/pairing/off', { app }))
}

export async function checkPairing(app: string): Promise<HostCheck[]> {
  return parseChecks(rec(await daemonCliPost<unknown>(primaryScope(), 'app/pairing/check', { app })).hosts)
}

/** One link-file verdict in words. */
export function checkWord(s: string): string {
  switch (s) {
    case 'ok':
      return 'ok'
    case 'none':
      return 'not used'
    case 'stale':
      return 'another file is served there'
    case 'not_served':
      return 'not served'
    case 'tls_error':
      return 'TLS error'
    default:
      return s
  }
}

/** Why pairing is off, in words. */
export function offWords(p: PairingStatus): string {
  if (p.on) return ''
  if (!p.served && p.notServed) return p.notServed
  switch (p.off) {
    case 'owner_off':
      return 'turned off'
    case 'box_off':
      return 'off on this server (K2_APP_PAIRING=off)'
    case 'not_set_up':
      return 'not set up yet'
    default:
      return p.off ?? 'off'
  }
}
