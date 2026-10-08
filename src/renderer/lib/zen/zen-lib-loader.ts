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

export interface ZenLibManifest {
  schema: 1
  /** UWB13: at most 8 MB of library text per widget. */
  maxBytesPerWidget: number
  cdnHosts: string[]
  libs: ZenLibEntry[]
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

/** The manifest row a named reference resolves to, or null (CDN refs: null). */
export function zenLibFind(ref: ZenLibRef, manifest: ZenLibManifest = ZEN_LIB_MANIFEST): ZenLibEntry | null {
  if (typeof ref !== 'string') return null
  const at = ref.lastIndexOf('@')
  if (at <= 0 || at === ref.length - 1) return null
  const id = ref.slice(0, at)
  const want = ref.slice(at + 1)
  return manifest.libs.find((e) => e.id === id && zenLibVersionMatches(e, want)) ?? null
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
      if (sum !== null && sum !== f.sha256) throw errorFor('hash_mismatch', ref, entry)
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
      // Resolve every ref before loading anything (all or nothing).
      const plan = refs.map((ref) => {
        if (typeof ref === 'string') {
          const entry = zenLibFind(ref, deps.manifest)
          if (!entry) throw errorFor('unknown_lib', ref, null)
          return { ref, entry }
        }
        const problem = zenLibCdnProblem(deps.manifest, ref.url, ref.integrity)
        if (problem) throw errorFor('unknown_lib', ref, null, problem)
        return { ref, entry: null }
      })
      if (plan.length === 0) return []
      const max = deps.manifest.maxBytesPerWidget
      const known = plan.reduce((n, p) => n + (p.entry ? p.entry.files.reduce((s, f) => s + f.bytes, 0) : 0), 0)
      if (known > max) throw errorFor('too_large', refs[0], null, `${mb(known)} asked, ${mb(max)} allowed`)

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
