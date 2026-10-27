// The widget standard library loader (prd-zen-user-widgets-v2 §13.9,
// UWB12–UWB15; Rosson R2, R8 2026-10-08).
//
// Day-0 interface (Zen v2, 2026-10-08). Owner: B3. B4's frame host calls
// `zenLib.texts(refs)` and inlines each text as a nonced classic <script>
// after `k2.js` and before the widget's own scripts (UWB13). The frame
// itself never touches the network.
//
// Sources, by manifest row (`src/shared/zen-lib.json`, Rust mirror
// `crates/k2-core/src/zen/stdlib.rs`):
//   - `bundled`: files in `src/renderer/zen-lib/<id>@<version>/`, copied
//     into the desktop renderer build only (never `public/`); read once per
//     app session from the app's own origin, size- and sha256-checked, and
//     cached in memory.
//   - `download` (Babylon.js, Plotly; R8 in this release): the daemon
//     fetches the pinned URL once, checks sha256, caches it in
//     `~/.k2/cache/zen-lib/`; the renderer reads it through
//     `GET /cli/zen/lib/file?id=&version=&file=` after `POST /cli/zen/lib/fetch`.
//   - CDN `{url, integrity}` (R2): the same daemon path, cached by hash.
// Air-gapped computers refuse downloads; bundled libraries always work.
//
// Every text comes back safe to put inside an HTML <script> element
// (`zen-lib-inline.ts`): bundled files are checked in that way, downloaded
// and CDN text is rewritten here after the daemon checked its hash.

import manifestJson from '@shared/zen-lib.json'
import { zenLibInlineSafe } from './zen-lib-inline'

/** One file of a library. */
export interface ZenLibFile {
  name: string
  bytes: number
  /** Lower-case hex sha256. */
  sha256: string
  /** Pinned download URL; set when the library is `download`. */
  url?: string
}

/** One row of `zen-lib.json`. */
export interface ZenLibEntry {
  id: string
  version: string
  /** Schema 2 (0.46.5): its line (`1`, `0.186`), what a widget asks for. */
  line: string
  /** Schema 2: line requests K2 loads first, once (`marked@18`). */
  needs?: string[]
  title: string
  /** The classic global it defines: THREE, PIXI, d3, … */
  global: string
  license: string
  licenseFile: string
  copyright: string
  source: 'bundled' | 'download'
  /** In inline order. */
  files: ZenLibFile[]
  prebundled?: string
  notes?: string
}

/** A retired line (0.46.5 WL6): its row stays, as a download. */
export interface ZenLibRetired {
  id: string
  line: string
  release: string
  use: string
}

export interface ZenLibManifest {
  schema: 2
  /** UWB13: at most 8 MB of library text per widget. */
  maxBytesPerWidget: number
  cdnHosts: string[]
  libs: ZenLibEntry[]
  retired?: ZenLibRetired[]
}

/**
 * One `requires.libs` item: `"three@0.170"` (exact version, or a prefix of
 * it ending at a dot) or a CDN file `{url, integrity}`.
 */
export type ZenLibRef = string | { url: string; integrity: string }

export type ZenLibErrorCode =
  | 'unknown_lib' // no manifest row matches
  | 'airgapped' // a download on an air-gapped computer
  | 'fetch_failed' // the daemon couldn't fetch it
  | 'hash_mismatch' // bytes don't match the pinned hash or SRI
  | 'too_large' // over maxBytesPerWidget in total
  | 'older_daemon' // the daemon has no `zen-lib-v1` routes

export class ZenLibError extends Error {
  readonly code: ZenLibErrorCode
  readonly ref: ZenLibRef
  constructor(code: ZenLibErrorCode, ref: ZenLibRef, message: string) {
    super(message)
    this.name = 'ZenLibError'
    this.code = code
    this.ref = ref
  }
}

export const ZEN_LIB_MANIFEST = manifestJson as ZenLibManifest

/** Dot-prefix version match: `0.170` and `0` name `0.170.0`; `0.17` doesn't. */
export function zenLibVersionMatches(entry: ZenLibEntry, want: string): boolean {
  return entry.version === want || (entry.version.startsWith(want) && entry.version[want.length] === '.')
}

// ── The resolver (0.46.5, prd-widget-size-and-shared-libs-v1 WL4–WL7) ────
//
// The Rust one (`stdlib::resolve_one` / `resolve`) is the truth: the
// daemon sends each placement's `libKeys` and the frame loads exactly
// those. This mirror serves the shot tool's renderer and an older daemon's
// payload (no `libKeys`); the shared fixture
// `src/shared/zen-lib-resolve-cases.json` keeps the two in step.

/** A version's line: the major from 1.0.0 up, `0.minor` below 1. */
export function zenLibLineOf(version: string): string {
  const parts = version.split('.')
  if (parts[0] === '0') return parts.length > 1 ? `0.${parts[1]}` : '0'
  return parts[0]
}

function vparts(v: string): number[] {
  return v.split('.').map((p) => {
    const d = /^\d+/.exec(p)
    return d ? Number(d[0]) : 0
  })
}

/** Versions compared numerically (a missing part is 0). */
export function zenLibVersionCmp(a: string, b: string): number {
  const x = vparts(a)
  const y = vparts(b)
  for (let i = 0; i < Math.max(x.length, y.length); i++) {
    const d = (x[i] ?? 0) - (y[i] ?? 0)
    if (d !== 0) return d < 0 ? -1 : 1
  }
  return 0
}

function newest(rows: readonly ZenLibEntry[]): ZenLibEntry | null {
  let best: ZenLibEntry | null = null
  for (const r of rows) if (!best || zenLibVersionCmp(r.version, best.version) > 0) best = r
  return best
}

export type ZenLibResolvedOne = { entry: ZenLibEntry; warning: string | null } | { error: string }

/** What `<id>@<want>` loads (Rust `resolve_one`, rule for rule). */
export function zenLibResolveOne(id: string, want: string, manifest: ZenLibManifest = ZEN_LIB_MANIFEST): ZenLibResolvedOne {
  const rows = manifest.libs.filter((e) => e.id === id)
  if (rows.length === 0) return { error: `K2's widget library has no ${id}@${want}; see k2 zen guide libs for what's available` }
  let warning: string | null = null
  const matched = rows.filter((e) => zenLibVersionMatches(e, want))
  let pick = newest(matched)
  if (pick) {
    if (new Set(matched.map((e) => e.line)).size > 1) {
      warning = `${id}@${want} spans versions that break each other; write ${id}@${pick.line}`
    }
  } else {
    const line = zenLibLineOf(want)
    pick = newest(rows.filter((e) => e.line === line && zenLibVersionCmp(e.version, want) >= 0))
    if (pick) {
      warning = `K2 ships ${id} ${pick.version} and updates it inside line ${pick.line}; write "${id}@${pick.line}"`
    } else {
      const sameLine = newest(rows.filter((e) => e.line === line))
      if (sameLine) {
        return {
          error: `K2 ships ${id} ${sameLine.version} in line ${sameLine.line}; ${id}@${want} is newer than K2's. Write "${id}@${sameLine.line}", or update K2`,
        }
      }
      const list = [...rows].sort((a, b) => zenLibVersionCmp(a.version, b.version)).map((e) => `${e.line} (${e.version})`)
      return { error: `K2 ships ${id} line${list.length === 1 ? '' : 's'} ${list.join(', ')}; this widget asks for ${id}@${want} (k2 zen guide libs)` }
    }
  }
  const chosen: ZenLibEntry = pick
  const retired = manifest.retired?.find((r) => r.id === id && r.line === chosen.line)
  if (retired) {
    const w = `K2 retired ${id}@${chosen.line} in ${retired.release}: it downloads on first use now (not under air-gap); move to ${retired.use}`
    warning = warning ? `${warning}; ${w}` : w
  }
  return { entry: chosen, warning }
}

/** One finding of `zenLibResolve` (`index` null: the whole list). */
export interface ZenLibFinding {
  index: number | null
  error: boolean
  message: string
}

export interface ZenLibResolved {
  /** The load order: exact keys (needs first, each once), CDN refs as written. */
  keys: ZenLibRef[]
  entries: Array<ZenLibEntry | null>
  findings: ZenLibFinding[]
}

function parseRef(ref: unknown): { id: string; want: string } | { cdn: { url: string; integrity: string } } | { bad: string } {
  if (typeof ref === 'string') {
    const at = ref.lastIndexOf('@')
    if (at <= 0 || at === ref.length - 1) return { bad: `'${ref}': name a library as <id>@<line>, like uplot@1 (k2 zen guide libs)` }
    return { id: ref.slice(0, at), want: ref.slice(at + 1) }
  }
  if (ref && typeof ref === 'object' && !Array.isArray(ref)) {
    const o = ref as Record<string, unknown>
    const extra = Object.keys(o).find((k) => k !== 'url' && k !== 'integrity')
    if (extra) return { bad: `unknown key '${extra}' in a requires.libs entry; use url and integrity` }
    if (typeof o.url === 'string' && typeof o.integrity === 'string') return { cdn: { url: o.url, integrity: o.integrity } }
    return { bad: 'a CDN entry needs both url and integrity' }
  }
  return { bad: 'a requires.libs entry is "<id>@<version>" or {url, integrity}' }
}

/** A whole `requires.libs` list (Rust `resolve`): needs first, every key
 *  once, two lines of one library an error, the 8 MB budget. */
export function zenLibResolve(refs: readonly unknown[], manifest: ZenLibManifest = ZEN_LIB_MANIFEST): ZenLibResolved {
  const out: ZenLibResolved = { keys: [], entries: [], findings: [] }
  const loaded: Array<{ key: string; id: string; line: string; explicit: boolean }> = []
  let bytes = 0
  const add = (index: number, e: ZenLibEntry, explicit: boolean, depth: number): void => {
    const key = `${e.id}@${e.version}`
    if (depth > 8) {
      out.findings.push({ index, error: true, message: `${key}: its needs go round in a circle` })
      return
    }
    for (const need of e.needs ?? []) {
      const at = need.lastIndexOf('@')
      if (at <= 0) {
        out.findings.push({ index, error: true, message: `${key} needs '${need}', which isn't <id>@<line>` })
        continue
      }
      const r = zenLibResolveOne(need.slice(0, at), need.slice(at + 1), manifest)
      if ('error' in r) out.findings.push({ index, error: true, message: `${key} needs ${need}: ${r.error}` })
      else add(index, r.entry, false, depth + 1)
    }
    const prev = loaded.find((l) => l.key === key)
    if (prev) {
      if (explicit && prev.explicit) out.findings.push({ index, error: false, message: `'${key}' is listed twice; K2 loads it once` })
      prev.explicit ||= explicit
      return
    }
    const other = loaded.find((l) => l.id === e.id && l.line !== e.line)
    if (other) {
      out.findings.push({
        index,
        error: true,
        message: `two versions of ${e.id} in one widget (${other.key} and ${key}) define ${e.global} twice; ask for one line`,
      })
      return
    }
    bytes += e.files.reduce((n, f) => n + f.bytes, 0)
    out.keys.push(key)
    out.entries.push(e)
    loaded.push({ key, id: e.id, line: e.line, explicit })
  }
  refs.forEach((ref, i) => {
    const p = parseRef(ref)
    if ('bad' in p) {
      out.findings.push({ index: i, error: true, message: p.bad })
    } else if ('cdn' in p) {
      const problem = zenLibCdnProblem(manifest, p.cdn.url, p.cdn.integrity)
      if (problem) {
        out.findings.push({ index: i, error: true, message: problem })
      } else if (out.keys.some((k) => typeof k !== 'string' && k.url === p.cdn.url && k.integrity === p.cdn.integrity)) {
        out.findings.push({ index: i, error: false, message: `${p.cdn.url} is listed twice; K2 loads it once` })
      } else {
        out.keys.push(p.cdn)
        out.entries.push(null)
      }
    } else {
      const r = zenLibResolveOne(p.id, p.want, manifest)
      if ('error' in r) {
        out.findings.push({ index: i, error: true, message: r.error })
      } else {
        if (r.warning) out.findings.push({ index: i, error: false, message: r.warning })
        add(i, r.entry, true, 0)
      }
    }
  })
  if (bytes > manifest.maxBytesPerWidget) {
    out.findings.push({
      index: null,
      error: true,
      message: `these libraries are ${bytes} bytes (with what they need); a widget may load at most ${manifest.maxBytesPerWidget} (8 MB)`,
    })
  }
  return out
}

/** The manifest row a named reference resolves to, or null (CDN refs and
 *  requests K2 can't serve: null). The resolver's rule, warnings dropped. */
export function zenLibFind(ref: ZenLibRef, manifest: ZenLibManifest = ZEN_LIB_MANIFEST): ZenLibEntry | null {
  if (typeof ref !== 'string') return null
  const p = parseRef(ref)
  if (!('id' in p)) return null
  const r = zenLibResolveOne(p.id, p.want, manifest)
  return 'entry' in r ? r.entry : null
}

export interface ZenLib {
  /**
   * The full text of each library, in the order asked, each library's files
   * joined in manifest order. Rejects with a `ZenLibError` on the first ref
   * that can't be served; never resolves partially. Texts are cached per
   * app session, so six widgets on a page share one copy.
   */
  texts(refs: readonly ZenLibRef[]): Promise<string[]>
}

// ── Daemon wire (UWB15; crates/k2-daemon/src/zen_lib_routes.rs) ─────────
//
//   POST /cli/zen/lib/fetch  {id, version} | {url, integrity}
//        → {ok, key, files: [{name, bytes, sha256}], cached}
//   GET  /cli/zen/lib/file?id=&version=&file=   a download row's file
//   GET  /cli/zen/lib/file?integrity=<sri>       a CDN file
//        → the checked text (text/javascript)
// Errors carry the code in `error`: unknown_lib, airgapped, fetch_failed,
// hash_mismatch, too_large, not_cached, bad_request. A daemon older than
// these routes answers `unknown zen route` (→ older_daemon).

/** One fetched download/CDN file as the daemon reports it. */
export interface ZenLibFetchedFile {
  name: string
  bytes: number
  sha256: string
}

export interface ZenLibFetchReply {
  ok: true
  key: string
  files: ZenLibFetchedFile[]
  cached: boolean
}

export type ZenLibFetchBody = { id: string; version: string } | { url: string; integrity: string }
export type ZenLibFileQuery = { id: string; version: string; file: string } | { integrity: string }

/** What the loader needs from the outside world (tests inject fakes). */
export interface ZenLibDeps {
  manifest: ZenLibManifest
  /** The bytes of a bundled file, `<id>@<version>/<name>`, from the app's
   *  own origin. Throws when it can't be read. */
  readBundled(path: string): Promise<ArrayBuffer>
  /** POST /cli/zen/lib/fetch on this computer's daemon. */
  daemonFetch(body: ZenLibFetchBody): Promise<unknown>
  /** GET /cli/zen/lib/file on this computer's daemon (raw text). */
  daemonFile(q: ZenLibFileQuery): Promise<string>
  /** Lower-case hex sha256, or null when the platform has no SubtleCrypto. */
  sha256(bytes: ArrayBuffer): Promise<string | null>
}

/** Between a library's files when they are joined into one script. */
export const ZEN_LIB_FILE_SEPARATOR = '\n;\n'

const DAEMON_CODES: readonly ZenLibErrorCode[] = ['unknown_lib', 'airgapped', 'fetch_failed', 'hash_mismatch', 'too_large']

function refLabel(ref: ZenLibRef): string {
  return typeof ref === 'string' ? ref : ref.url
}

function mb(n: number): string {
  return `${(n / 1048576).toFixed(1)} MB`
}

/** The daemon's error code from a thrown daemon-cli error (its message is
 *  the body's `error`), or null. */
function daemonCode(err: unknown): ZenLibErrorCode | 'not_cached' | null {
  const msg = err instanceof Error ? err.message : String(err)
  if (/unknown zen route/i.test(msg)) return 'older_daemon'
  if (msg === 'not_cached') return 'not_cached'
  return (DAEMON_CODES as readonly string[]).includes(msg) ? (msg as ZenLibErrorCode) : null
}

function errorFor(code: ZenLibErrorCode, ref: ZenLibRef, entry: ZenLibEntry | null, detail?: string): ZenLibError {
  const name = entry ? `${entry.title} ${entry.version}` : refLabel(ref)
  const tail = detail ? ` (${detail})` : ''
  const text: Record<ZenLibErrorCode, string> = {
    unknown_lib: `K2 has no library '${refLabel(ref)}'. See k2 zen guide libs.${tail}`,
    airgapped: `This widget needs ${name}, which K2 downloads on first use. This computer is air-gapped, so K2 won't download it.`,
    fetch_failed: `K2 couldn't load ${name}${tail}.`,
    hash_mismatch: `${name} didn't match its pinned fingerprint, so K2 refused it.`,
    too_large: `This widget asks for more library code than K2 allows in one widget${tail}.`,
    older_daemon: `K2 on this computer is older than this app. Update it to load ${name}.`,
  }
  return new ZenLibError(code, ref, text[code])
}

/** Check a CDN reference the way the daemon will (R2; `stdlib::check_cdn`):
 *  https, an allowed host, an exact version (no `@latest` or ranges), and an
 *  SRI hash. Returns why it isn't acceptable, or null. */
export function zenLibCdnProblem(manifest: ZenLibManifest, url: string, integrity: string): string | null {
  if (!url.startsWith('https://')) return 'CDN libraries load over https only'
  const host = url.slice('https://'.length).split('/')[0]
  if (!manifest.cdnHosts.includes(host)) return `'${host}' isn't an allowed CDN; use ${manifest.cdnHosts.join(', ')}`
  if (/@latest|[\^~*]/.test(url)) return 'pin an exact version in the URL (no @latest or ranges)'
  if (!/^sha(256|384|512)-[A-Za-z0-9+/]{43,}={0,2}$/.test(integrity)) {
    return 'integrity must be an SRI hash: sha256-…, sha384-… or sha512-…'
  }
  return null
}

/** A loader over `deps`. `zenLib` below is the app's one instance. */
export function createZenLib(deps: ZenLibDeps): ZenLib {
  // Per app session: one promise per library (bundled or downloaded) and
  // per CDN file. A failure is forgotten, so the next mount can retry.
  const cache = new Map<string, Promise<string>>()

  const once = (key: string, load: () => Promise<string>): Promise<string> => {
    let p = cache.get(key)
    if (!p) {
      p = load()
      cache.set(key, p)
      p.catch(() => cache.delete(key))
    }
    return p
  }

  const decode = (buf: ArrayBuffer): string => new TextDecoder('utf-8', { fatal: true }).decode(buf)

  async function loadBundled(entry: ZenLibEntry, ref: ZenLibRef): Promise<string> {
    const texts: string[] = []
    for (const f of entry.files) {
      let buf: ArrayBuffer
      try {
        buf = await deps.readBundled(`${entry.id}@${entry.version}/${f.name}`)
      } catch (e) {
        throw errorFor('fetch_failed', ref, entry, e instanceof Error ? e.message : String(e))
      }
      if (buf.byteLength !== f.bytes) throw errorFor('hash_mismatch', ref, entry)
      const sum = await deps.sha256(buf)
      // vs-live L2: fail closed. A platform that can't compute sha256 never
      // inlines a library it couldn't check (Tauri always can).
      if (sum === null) throw errorFor('hash_mismatch', ref, entry, "this app can't check fingerprints here")
      if (sum !== f.sha256) throw errorFor('hash_mismatch', ref, entry)
      texts.push(decode(buf))
    }
    return texts.join(ZEN_LIB_FILE_SEPARATOR)
  }

  async function viaDaemon<T>(ref: ZenLibRef, entry: ZenLibEntry | null, call: () => Promise<T>): Promise<T> {
    try {
      return await call()
    } catch (e) {
      if (e instanceof ZenLibError) throw e
      const code = daemonCode(e)
      if (code === 'not_cached') throw errorFor('fetch_failed', ref, entry, "not in K2's download cache")
      if (code) throw errorFor(code, ref, entry)
      throw errorFor('fetch_failed', ref, entry, e instanceof Error ? e.message : String(e))
    }
  }

  function filesOf(reply: unknown, ref: ZenLibRef, entry: ZenLibEntry | null): ZenLibFetchedFile[] {
    const r = reply as Partial<ZenLibFetchReply> | null
    if (!r || r.ok !== true || !Array.isArray(r.files)) {
      throw errorFor('fetch_failed', ref, entry, 'unexpected answer from K2')
    }
    return r.files
  }

  async function loadDownload(entry: ZenLibEntry, ref: ZenLibRef): Promise<string> {
    const reply = await viaDaemon(ref, entry, () => deps.daemonFetch({ id: entry.id, version: entry.version }))
    const fetched = filesOf(reply, ref, entry)
    const texts: string[] = []
    for (const f of entry.files) {
      const got = fetched.find((x) => x.name === f.name)
      if (!got || got.sha256 !== f.sha256 || got.bytes !== f.bytes) throw errorFor('hash_mismatch', ref, entry)
      const text = await viaDaemon(ref, entry, () =>
        deps.daemonFile({ id: entry.id, version: entry.version, file: f.name }),
      )
      if (new TextEncoder().encode(text).length !== f.bytes) throw errorFor('hash_mismatch', ref, entry)
      texts.push(zenLibInlineSafe(text))
    }
    return texts.join(ZEN_LIB_FILE_SEPARATOR)
  }

  async function loadCdn(ref: { url: string; integrity: string }): Promise<string> {
    const reply = await viaDaemon(ref, null, () => deps.daemonFetch({ url: ref.url, integrity: ref.integrity }))
    const [file] = filesOf(reply, ref, null)
    const text = await viaDaemon(ref, null, () => deps.daemonFile({ integrity: ref.integrity }))
    if (!file || new TextEncoder().encode(text).length !== file.bytes) throw errorFor('hash_mismatch', ref, null)
    return zenLibInlineSafe(text)
  }

  return {
    async texts(refs) {
      // Resolve every ref before loading anything (all or nothing), with the
      // one resolver: `libKeys` (exact keys) resolve to themselves; an older
      // daemon's `libs` get their lines and needs here.
      const r = zenLibResolve(refs, deps.manifest)
      const max = deps.manifest.maxBytesPerWidget
      for (const f of r.findings) {
        if (!f.error) continue
        if (f.index === null) throw errorFor('too_large', refs[0], null, f.message)
        throw errorFor('unknown_lib', refs[f.index], null, f.message)
      }
      const plan = r.keys.map((ref, i) => ({ ref, entry: r.entries[i] }))
      if (plan.length === 0) return []

      const texts = await Promise.all(
        plan.map(({ ref, entry }) => {
          if (!entry) {
            const cdn = ref as { url: string; integrity: string }
            return once(`cdn:${cdn.integrity}`, () => loadCdn(cdn))
          }
          const key = `${entry.id}@${entry.version}`
          return once(key, () => (entry.source === 'bundled' ? loadBundled(entry, ref) : loadDownload(entry, ref)))
        }),
      )
      const total = texts.reduce((n, t) => n + new TextEncoder().encode(t).length, 0)
      if (total > max) throw errorFor('too_large', refs[0], null, `${mb(total)} asked, ${mb(max)} allowed`)
      return texts
    },
  }
}

// ── The app's instance ─────────────────────────────────────────────────

async function readBundledFromApp(path: string): Promise<ArrayBuffer> {
  // The app's own origin (tauri://localhost in the desktop build; the Vite
  // dev server serves the same files at /zen-lib/). Not a daemon call.
  const res = await fetch(`/zen-lib/${path}`)
  if (!res.ok) throw new Error(`/zen-lib/${path}: ${res.status}`)
  return res.arrayBuffer()
}

async function sha256Hex(bytes: ArrayBuffer): Promise<string | null> {
  const subtle = globalThis.crypto?.subtle
  if (!subtle) return null
  const d = new Uint8Array(await subtle.digest('SHA-256', bytes))
  return Array.from(d, (b) => b.toString(16).padStart(2, '0')).join('')
}

/** Downloads and the CDN cache live in THIS computer's daemon (R2, R8),
 *  whichever server the window shows. Imported lazily so the loader stays
 *  light for its callers and tests. */
async function localCli() {
  const [{ scopeForHost }, cli] = await Promise.all([import('@/kessel/server-scope'), import('@/lib/daemon-cli')])
  return { scope: scopeForHost('local'), cli }
}

export const zenLib: ZenLib = createZenLib({
  manifest: ZEN_LIB_MANIFEST,
  readBundled: readBundledFromApp,
  async daemonFetch(body) {
    const { scope, cli } = await localCli()
    return cli.daemonCliPost<unknown>(scope, 'zen/lib/fetch', body)
  },
  async daemonFile(q) {
    const { scope, cli } = await localCli()
    return cli.daemonCliGetText(scope, 'zen/lib/file', q)
  },
  sha256: sha256Hex,
})
