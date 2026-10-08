// Day-0 interface checks (prd-zen-user-widgets-v2 §15): the hand-written TS
// mirrors agree with the Rust-side data until B1's generator owns them.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { CATALOG_ERROR_CODES, type Catalog } from '../contract/catalog-types'
import { K2_CAPS, USER_WIDGET_CAPS } from '../k2-caps.generated'
import { ZEN_VERBS as BRIDGE_ZEN_VERBS } from './zen-bridge'
import { ZEN_CUSTOM_VERBS, ZEN_VERBS } from './zen-verbs.generated'
import { ZEN_TEMPLATES_FALLBACK, zenGrantNeedsReview, type ZenGrantView } from './zen-custom-types'
import { ZEN_LIB_MANIFEST, ZenLibError, zenLib, zenLibFind, zenLibVersionMatches, type ZenLibEntry } from './zen-lib-loader'

const ROOT = resolve(__dirname, '../../../..')
const catalog = JSON.parse(
  readFileSync(resolve(ROOT, 'crates/k2-core/src/contract/catalog.json'), 'utf8'),
) as Catalog

describe('day-0: catalog mirror', () => {
  it('error codes match catalog.json in order', () => {
    expect(catalog.errors.map((e) => e.code)).toEqual([...CATALOG_ERROR_CODES])
  })

  it('cap table matches catalog.json', () => {
    const fromJson = Object.fromEntries(
      catalog.caps.map((c) => [c.name, { label: c.label, sentence: c.sentence, exposure: c.exposure ?? [] }]),
    )
    const fromTs = Object.fromEntries(
      Object.entries(K2_CAPS).map(([k, v]) => [
        k,
        { label: v.label, sentence: 'sentence' in v ? v.sentence : undefined, exposure: [...v.exposure] },
      ]),
    )
    expect(fromTs).toEqual(fromJson)
    expect(catalog.caps.filter((c) => c.exposure?.includes('widget')).map((c) => c.name)).toEqual([
      ...USER_WIDGET_CAPS,
    ])
  })
})

describe('day-0: verb tables', () => {
  it('placeholder ZEN_VERBS equals the bridge table key for key', () => {
    expect(ZEN_VERBS).toEqual(BRIDGE_ZEN_VERBS)
  })

  it('custom verbs are a subset of the bridge verbs (plus the frame-only theme.changed), with widget caps only', () => {
    for (const [verb, row] of Object.entries(ZEN_CUSTOM_VERBS)) {
      if (verb === 'theme.changed') {
        expect(row.kind).toBe('event')
        continue
      }
      expect(verb in ZEN_VERBS, verb).toBe(true)
      expect(ZEN_VERBS[verb as keyof typeof ZEN_VERBS], verb).toBe(row.cap)
      if (row.cap) expect(USER_WIDGET_CAPS as readonly string[]).toContain(row.cap)
    }
    for (const never of ['zen.exit', 'controls.bind', 'homes.list', 'agents.local', 'agents.add', 'thread.void', 'thread.markRead']) {
      expect(never in ZEN_CUSTOM_VERBS, never).toBe(false)
    }
  })
})

describe('day-0: grants and templates', () => {
  const view = (state: ZenGrantView['state']): ZenGrantView => ({
    state,
    caps: [],
    granted: [],
    scope: null,
    entries: [],
    sending: false,
    paused: null,
    grantedAt: null,
    widgetHash: null,
  })

  it('review card rule matches the Rust GrantView::needs_review', () => {
    expect(zenGrantNeedsReview(null, ['agents:read'])).toBe(true)
    expect(zenGrantNeedsReview(view('none'), ['agents:read'])).toBe(true)
    expect(zenGrantNeedsReview(view('invalid'), ['agents:read'])).toBe(true)
    expect(zenGrantNeedsReview(view('review'), ['agents:read'])).toBe(true)
    expect(zenGrantNeedsReview(view('partial'), ['agents:read'])).toBe(false)
    expect(zenGrantNeedsReview(view('granted'), ['agents:read'])).toBe(false)
    expect(zenGrantNeedsReview(null, [])).toBe(false)
  })

  it('template fallback is the two starts', () => {
    expect(ZEN_TEMPLATES_FALLBACK.map((t) => [t.id, t.short, t.section])).toEqual([
      ['k2.texting@1', 'texting', 'start'],
      ['k2.blank@1', 'blank', 'start'],
    ])
  })
})

describe('day-0: zenLib', () => {
  it('manifest loads; no p5 or gsap', () => {
    expect(ZEN_LIB_MANIFEST.schema).toBe(1)
    expect(ZEN_LIB_MANIFEST.maxBytesPerWidget).toBe(8 * 1024 * 1024)
    expect(ZEN_LIB_MANIFEST.libs.some((l) => l.id === 'p5' || l.id === 'gsap')).toBe(false)
  })

  it('version match is by dot prefix', () => {
    const e = { id: 'three', version: '0.170.0' } as ZenLibEntry
    expect(zenLibVersionMatches(e, '0.170.0')).toBe(true)
    expect(zenLibVersionMatches(e, '0.170')).toBe(true)
    expect(zenLibVersionMatches(e, '0')).toBe(true)
    expect(zenLibVersionMatches(e, '0.17')).toBe(false)
    expect(zenLibFind('three')).toBeNull()
  })

  it('texts rejects an unknown library with unknown_lib', async () => {
    await expect(zenLib.texts(['nope@1'])).rejects.toBeInstanceOf(ZenLibError)
    await expect(zenLib.texts(['nope@1'])).rejects.toMatchObject({ code: 'unknown_lib' })
    await expect(zenLib.texts([])).resolves.toEqual([])
  })
})
