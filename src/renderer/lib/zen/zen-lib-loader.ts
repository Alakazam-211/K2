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
//     app session from the app's own origin and cached in memory.
//   - `download` (Babylon.js, Plotly; R8 in this release): the daemon
//     fetches the pinned URL once, checks sha256, caches it in
//     `~/.k2/cache/zen-lib/`; the renderer reads it through
//     `GET /cli/zen/lib/file?id=&file=` after `POST /cli/zen/lib/fetch`.
//   - CDN `{url, integrity}` (R2): the same daemon path, cached by hash.
// Air-gapped computers refuse downloads; bundled libraries always work.

import manifestJson from '@shared/zen-lib.json'

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

export const zenLib: ZenLib = {
  async texts(refs) {
    for (const ref of refs) {
      if (typeof ref === 'string' && !zenLibFind(ref)) {
        throw new ZenLibError('unknown_lib', ref, `K2 has no library '${ref}'. See k2 zen guide libs.`)
      }
    }
    if (refs.length === 0) return []
    // B3 (UWB13, UWB15): read bundled files from the app origin, downloads
    // and CDN files through the daemon, check sizes, cache per session.
    throw new Error('zenLib.texts: loading lands with B3 (day-0 stub)')
  },
}
