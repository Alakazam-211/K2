// The Zen widget standard library as checked in (prd-zen-user-widgets-v2
// UWB12, UWB16, TUWB5): every manifest row has its files with the recorded
// size and sha256, a licence file and a NOTICE line; NOTICE.md's generated
// section is exactly what the manifest renders; nothing ships to the web
// build; every bundled text is safe to inline into a <script>.
import { createHash } from 'node:crypto'
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ZEN_LIB_MANIFEST } from './zen-lib-loader'
import { currentZenLibNotice, renderZenLibNotice } from './zen-lib-notice'
import { zenLibInlineProblem } from './zen-lib-inline'

const ROOT = resolve(__dirname, '../../../..')
const LIB_DIR = resolve(ROOT, 'src/renderer/zen-lib')
const m = ZEN_LIB_MANIFEST

const sha256 = (b: Buffer): string => createHash('sha256').update(b).digest('hex')

describe('zen-lib manifest', () => {
  it('has libraries, unique ids, and none that R4 / §13.9 forbid', () => {
    expect(m.libs.length).toBeGreaterThan(20)
    const keys = m.libs.map((l) => `${l.id}@${l.version}`)
    expect(new Set(keys).size).toBe(keys.length)
    expect(new Set(m.libs.map((l) => l.id)).size).toBe(m.libs.length)
    for (const banned of ['p5', 'gsap']) expect(m.libs.map((l) => l.id)).not.toContain(banned)
    expect(m.libs.filter((l) => l.source === 'download').map((l) => l.id)).toEqual(['babylonjs', 'plotly.js-dist-min'])
  })

  it('every row is complete: files, sizes, sha256, a licence, a global', () => {
    for (const l of m.libs) {
      const key = `${l.id}@${l.version}`
      expect(l.files.length, key).toBeGreaterThan(0)
      expect(l.global, key).toMatch(/^[A-Za-z_$][\w$]*$/)
      expect(l.licenseFile, key).toBe('LICENSE.txt')
      expect(l.copyright.length, key).toBeGreaterThan(5)
      expect(/^\d+\.\d+\.\d+$/.test(l.version), key).toBe(true)
      for (const f of l.files) {
        expect(f.name, key).toMatch(/^[\w.@-]+$/)
        expect(f.sha256, `${key}/${f.name}`).toMatch(/^[0-9a-f]{64}$/)
        expect(f.bytes, `${key}/${f.name}`).toBeGreaterThan(0)
      }
      const total = l.files.reduce((n, f) => n + f.bytes, 0)
      expect(total, `${key} fits one widget`).toBeLessThanOrEqual(m.maxBytesPerWidget)
    }
  })

  it('bundled files on disk match the recorded bytes and sha256 and are safe to inline', () => {
    for (const l of m.libs.filter((x) => x.source === 'bundled')) {
      for (const f of l.files) {
        const p = resolve(LIB_DIR, `${l.id}@${l.version}`, f.name)
        const buf = readFileSync(p)
        expect(buf.length, p).toBe(f.bytes)
        expect(sha256(buf), p).toBe(f.sha256)
        expect(f.url, p).toBeUndefined()
        expect(zenLibInlineProblem(buf.toString('utf8')), p).toBeNull()
      }
    }
  })

  it('download rows name pinned https URLs on an allowed CDN, and ship no code', () => {
    for (const l of m.libs.filter((x) => x.source === 'download')) {
      for (const f of l.files) {
        const u = new URL(f.url ?? 'missing:')
        expect(u.protocol, l.id).toBe('https:')
        expect(m.cdnHosts, l.id).toContain(u.host)
        expect(u.pathname, l.id).toContain(`@${l.version}/`)
        expect(existsSync(resolve(LIB_DIR, `${l.id}@${l.version}`, f.name)), `${l.id} code is not checked in`).toBe(false)
      }
    }
  })

  it('every library folder has its licence file, and the folders are exactly the manifest', () => {
    const dirs = readdirSync(LIB_DIR).filter((d) => statSync(resolve(LIB_DIR, d)).isDirectory()).sort()
    expect(dirs).toEqual(m.libs.map((l) => `${l.id}@${l.version}`).sort())
    for (const l of m.libs) {
      const lic = readFileSync(resolve(LIB_DIR, `${l.id}@${l.version}`, l.licenseFile), 'utf8')
      const flat = lic.replace(/\s+/g, ' ')
      for (const part of l.copyright.split('; ').map((p) => p.replace(/^[\w.-]+: /, ''))) {
        expect(flat.includes(part.replace(/\s+/g, ' ')), `${l.id}: "${part}" in LICENSE.txt`).toBe(true)
      }
    }
  })
})

describe('NOTICE.md', () => {
  const notice = readFileSync(resolve(ROOT, 'NOTICE.md'), 'utf8')

  it('the zen-lib section is exactly what the manifest renders (run scripts/vendor-zen-lib.sh)', () => {
    expect(currentZenLibNotice(notice)).toBe(renderZenLibNotice(m))
  })

  it('has one line per library with its version, licence and copyright', () => {
    const section = currentZenLibNotice(notice) ?? ''
    for (const l of m.libs) {
      const line = section.split('\n').find((x) => x.includes(`(\`${l.id}\`)`))
      expect(line, l.id).toBeDefined()
      expect(line).toContain(` ${l.version} `)
      expect(line).toContain(l.license)
      expect(line).toContain(l.copyright.replace(/\|/g, '\\|'))
    }
  })

  it('credits the vendored Mermaid build, with its licence file next to it', () => {
    expect(notice).toContain('## Third-party: Mermaid (MIT)')
    expect(notice).toContain('src/renderer/public/vendor/mermaid.LICENSE.txt')
    expect(readFileSync(resolve(ROOT, 'src/renderer/public/vendor/mermaid.LICENSE.txt'), 'utf8')).toContain(
      'Copyright (c) 2014 - 2022 Knut Sveidqvist',
    )
  })
})

describe('desktop only (UWB12)', () => {
  it('the library is not under public/ (the hosted web build copies public/)', () => {
    expect(existsSync(resolve(ROOT, 'src/renderer/public/zen-lib'))).toBe(false)
  })

  it('only the desktop vite config copies zen-lib; the web config never names it', () => {
    expect(readFileSync(resolve(ROOT, 'vite.config.ts'), 'utf8')).toContain('zenLibDesktopCopy')
    expect(readFileSync(resolve(ROOT, 'vite.config.web.ts'), 'utf8')).not.toMatch(/zen-?lib/i)
  })
})
