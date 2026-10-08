// zenLib.texts (prd-zen-user-widgets-v2 §15.3, UWB13, UWB15, TUWB5): order,
// all-or-nothing, per-session cache, size and hash checks, the daemon path
// for downloads and CDN files, and every error code. Fakes stand in for the
// app origin and the daemon; one test reads the real checked-in files.
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import {
  createZenLib,
  ZEN_LIB_FILE_SEPARATOR,
  ZEN_LIB_MANIFEST,
  ZenLibError,
  zenLibCdnProblem,
  type ZenLibDeps,
  type ZenLibEntry,
  type ZenLibManifest,
} from './zen-lib-loader'
import { zenLibInlineProblem } from './zen-lib-inline'

const LIB_DIR = resolve(__dirname, '../../zen-lib')
const hex = (b: Buffer | string): string => createHash('sha256').update(b).digest('hex')
const buf = (s: string): ArrayBuffer => {
  const b = Buffer.from(s, 'utf8')
  return b.buffer.slice(b.byteOffset, b.byteOffset + b.length) as ArrayBuffer
}

function file(name: string, text: string, url?: string) {
  return { name, bytes: Buffer.byteLength(text), sha256: hex(text), ...(url ? { url } : {}) }
}

function entry(id: string, version: string, files: ReturnType<typeof file>[], source: 'bundled' | 'download' = 'bundled'): ZenLibEntry {
  return { id, version, title: id.toUpperCase(), global: 'G', license: 'MIT', licenseFile: 'LICENSE.txt', copyright: 'c', source, files }
}

const A1 = 'var A=1'
const A2 = 'A+=1'
const B = 'var B=2'
const BIG = 'x'.repeat(64)
const DL = 'var D="</script>"'
const SRI = `sha384-${'A'.repeat(64)}`
const CDN_URL = 'https://cdn.jsdelivr.net/npm/x@1.2.3/dist/x.min.js'
const CDN_TEXT = 'var X=3'

function manifest(max = 1024): ZenLibManifest {
  return {
    schema: 1,
    maxBytesPerWidget: max,
    cdnHosts: ['cdnjs.cloudflare.com', 'cdn.jsdelivr.net', 'unpkg.com'],
    libs: [
      entry('alpha', '1.2.3', [file('a1.js', A1), file('a2.js', A2)]),
      entry('beta', '0.5.0', [file('b.js', B)]),
      entry('big', '1.0.0', [file('big.js', BIG)]),
      entry('dl', '2.0.0', [file('dl.js', DL, 'https://cdn.jsdelivr.net/npm/dl@2.0.0/dl.js')], 'download'),
    ],
  }
}

const DISK: Record<string, string> = {
  'alpha@1.2.3/a1.js': A1,
  'alpha@1.2.3/a2.js': A2,
  'beta@0.5.0/b.js': B,
  'big@1.0.0/big.js': BIG,
}

function deps(over: Partial<ZenLibDeps> = {}, max?: number) {
  const d = {
    manifest: manifest(max),
    readBundled: vi.fn(async (path: string) => {
      const t = DISK[path]
      if (t === undefined) throw new Error(`404 ${path}`)
      return buf(t)
    }),
    daemonFetch: vi.fn(async (body: unknown) => {
      if ('id' in (body as object)) return { ok: true, key: 'dl@2.0.0', files: [file('dl.js', DL)], cached: false }
      return { ok: true, key: `cdn:${SRI}`, files: [file('x.min.js', CDN_TEXT)], cached: false }
    }),
    daemonFile: vi.fn(async (q: Record<string, string>) => ('integrity' in q ? CDN_TEXT : DL)),
    sha256: vi.fn(async (b: ArrayBuffer) => hex(Buffer.from(b))),
    ...over,
  }
  return d
}

async function codeOf(p: Promise<unknown>): Promise<string> {
  const e = await p.then(
    () => {
      throw new Error('expected a rejection')
    },
    (err: unknown) => err,
  )
  expect(e).toBeInstanceOf(ZenLibError)
  return (e as ZenLibError).code
}

describe('zenLib.texts: bundled', () => {
  it('answers in the order asked, files joined in manifest order', async () => {
    const lib = createZenLib(deps())
    expect(await lib.texts(['beta@0.5', 'alpha@1'])).toEqual([B, `${A1}${ZEN_LIB_FILE_SEPARATOR}${A2}`])
    expect(await lib.texts([])).toEqual([])
  })

  it('reads each library once per session', async () => {
    const readBundled = vi.fn(async (path: string) => buf(DISK[path]))
    const lib = createZenLib(deps({ readBundled }))
    await Promise.all([lib.texts(['alpha@1.2.3']), lib.texts(['alpha@1.2'])])
    await lib.texts(['alpha@1', 'beta@0'])
    expect(readBundled.mock.calls.map((c) => c[0])).toEqual(['alpha@1.2.3/a1.js', 'alpha@1.2.3/a2.js', 'beta@0.5.0/b.js'])
  })

  it('a wrong size or sha256 is hash_mismatch; a read failure is fetch_failed and is retried next time', async () => {
    expect(await codeOf(createZenLib(deps({ readBundled: async () => buf('var B=3') })).texts(['beta@0.5.0']))).toBe('hash_mismatch')
    expect(await codeOf(createZenLib(deps({ readBundled: async () => buf('var B=22') })).texts(['beta@0.5.0']))).toBe('hash_mismatch')
    let fail = true
    const d = deps({
      readBundled: vi.fn(async () => {
        if (fail) throw new Error('offline')
        return buf(B)
      }),
    })
    const lib = createZenLib(d)
    expect(await codeOf(lib.texts(['beta@0.5.0']))).toBe('fetch_failed')
    fail = false
    expect(await lib.texts(['beta@0.5.0'])).toEqual([B])
  })

  it('without SubtleCrypto the size check still runs', async () => {
    const lib = createZenLib(deps({ sha256: async () => null, readBundled: async () => buf('var B=3') }))
    expect(await lib.texts(['beta@0.5.0'])).toEqual(['var B=3'])
    const short = createZenLib(deps({ sha256: async () => null, readBundled: async () => buf('var B=') }))
    expect(await codeOf(short.texts(['beta@0.5.0']))).toBe('hash_mismatch')
  })
})

describe('zenLib.texts: all or nothing', () => {
  it('an unknown name rejects before anything loads', async () => {
    const d = deps()
    const lib = createZenLib(d)
    expect(await codeOf(lib.texts(['alpha@1', 'nope@1']))).toBe('unknown_lib')
    expect(await codeOf(lib.texts(['alpha@1.3']))).toBe('unknown_lib')
    expect(await codeOf(lib.texts(['alpha']))).toBe('unknown_lib')
    expect(d.readBundled).not.toHaveBeenCalled()
  })

  it('over the per-widget limit is too_large before anything loads', async () => {
    const d = deps({}, 70)
    const lib = createZenLib(d)
    expect(await codeOf(lib.texts(['big@1', 'beta@0']))).toBe('too_large')
    expect(d.readBundled).not.toHaveBeenCalled()
    expect(await lib.texts(['big@1'])).toEqual([BIG])
  })

  it('one failing library fails the whole call', async () => {
    const d = deps({ daemonFetch: vi.fn(async () => Promise.reject(new Error('airgapped'))) })
    expect(await codeOf(createZenLib(d).texts(['alpha@1', 'dl@2']))).toBe('airgapped')
  })
})

describe('zenLib.texts: downloads through the daemon (R8)', () => {
  it('fetches, checks the reply against the pin, reads the text and makes it safe to inline', async () => {
    const d = deps()
    const [text] = await createZenLib(d).texts(['dl@2'])
    expect(d.daemonFetch).toHaveBeenCalledWith({ id: 'dl', version: '2.0.0' })
    expect(d.daemonFile).toHaveBeenCalledWith({ id: 'dl', version: '2.0.0', file: 'dl.js' })
    expect(text).toBe('var D="<\\/script>"')
    expect(zenLibInlineProblem(text)).toBeNull()
  })

  it('maps the daemon error codes', async () => {
    const reject = (msg: string) => deps({ daemonFetch: vi.fn(async () => Promise.reject(new Error(msg))) })
    expect(await codeOf(createZenLib(reject('airgapped')).texts(['dl@2']))).toBe('airgapped')
    expect(await codeOf(createZenLib(reject('hash_mismatch')).texts(['dl@2']))).toBe('hash_mismatch')
    expect(await codeOf(createZenLib(reject('fetch_failed')).texts(['dl@2']))).toBe('fetch_failed')
    expect(await codeOf(createZenLib(reject('too_large')).texts(['dl@2']))).toBe('too_large')
    expect(await codeOf(createZenLib(reject('unknown zen route')).texts(['dl@2']))).toBe('older_daemon')
    expect(await codeOf(createZenLib(reject('connection refused')).texts(['dl@2']))).toBe('fetch_failed')
    const notCached = deps({ daemonFile: vi.fn(async () => Promise.reject(new Error('not_cached'))) })
    expect(await codeOf(createZenLib(notCached).texts(['dl@2']))).toBe('fetch_failed')
  })

  it('the air-gap message says why, in plain words', async () => {
    const d = deps({ daemonFetch: vi.fn(async () => Promise.reject(new Error('airgapped'))) })
    await expect(createZenLib(d).texts(['dl@2'])).rejects.toThrow(/downloads on first use\. This computer is air-gapped/)
  })

  it('a reply whose hash or size differs from the pin, or a text of another size, is refused', async () => {
    const wrong = deps({ daemonFetch: vi.fn(async () => ({ ok: true, key: 'k', files: [file('dl.js', 'other')], cached: true })) })
    expect(await codeOf(createZenLib(wrong).texts(['dl@2']))).toBe('hash_mismatch')
    const odd = deps({ daemonFetch: vi.fn(async () => ({ nope: 1 })) })
    expect(await codeOf(createZenLib(odd).texts(['dl@2']))).toBe('fetch_failed')
    const swapped = deps({ daemonFile: vi.fn(async () => 'var D=1') })
    expect(await codeOf(createZenLib(swapped).texts(['dl@2']))).toBe('hash_mismatch')
  })
})

describe('zenLib.texts: CDN files (R2)', () => {
  it('goes through the daemon by url + integrity and reads back by integrity', async () => {
    const d = deps()
    expect(await createZenLib(d).texts([{ url: CDN_URL, integrity: SRI }, 'beta@0'])).toEqual([CDN_TEXT, B])
    expect(d.daemonFetch).toHaveBeenCalledWith({ url: CDN_URL, integrity: SRI })
    expect(d.daemonFile).toHaveBeenCalledWith({ integrity: SRI })
  })

  it('refuses a bad CDN reference without calling the daemon', async () => {
    const d = deps()
    const lib = createZenLib(d)
    for (const ref of [
      { url: 'http://cdn.jsdelivr.net/npm/x@1.2.3/x.js', integrity: SRI },
      { url: 'https://evil.example.test/x@1.2.3/x.js', integrity: SRI },
      { url: 'https://unpkg.com/x@latest/x.js', integrity: SRI },
      { url: 'https://unpkg.com/x@^1/x.js', integrity: SRI },
      { url: CDN_URL, integrity: 'md5-abc' },
    ]) {
      expect(await codeOf(lib.texts([ref])), ref.url).toBe('unknown_lib')
    }
    expect(d.daemonFetch).not.toHaveBeenCalled()
    expect(zenLibCdnProblem(d.manifest, CDN_URL, SRI)).toBeNull()
  })
})

describe('zenLib.texts over the real checked-in library', () => {
  it('every bundled library loads with its recorded size and sha256', async () => {
    const lib = createZenLib({
      manifest: ZEN_LIB_MANIFEST,
      readBundled: async (path) => {
        const b = readFileSync(resolve(LIB_DIR, path))
        return b.buffer.slice(b.byteOffset, b.byteOffset + b.length) as ArrayBuffer
      },
      daemonFetch: async () => Promise.reject(new Error('no daemon in this test')),
      daemonFile: async () => Promise.reject(new Error('no daemon in this test')),
      sha256: async (b) => hex(Buffer.from(b)),
    })
    const bundled = ZEN_LIB_MANIFEST.libs.filter((l) => l.source === 'bundled')
    for (const l of bundled) {
      const [text] = await lib.texts([`${l.id}@${l.version}`])
      expect(text.length, l.id).toBeGreaterThan(100)
      expect(zenLibInlineProblem(text), l.id).toBeNull()
    }
    // A download library can't come from the app bundle.
    expect(await codeOf(lib.texts(['babylonjs@8']))).toBe('fetch_failed')
  })
})
