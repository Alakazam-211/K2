// Day-0 interface checks (prd-zen-user-widgets-v2 §15): the hand-written TS
// mirrors agree with the Rust-side data until B1's generator owns them.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import { CATALOG_ERROR_CODES, type Catalog } from '../contract/catalog-types'
import { K2_CAPS, USER_WIDGET_CAPS } from '../k2-caps.generated'
import { ZEN_VERBS as BRIDGE_ZEN_VERBS } from './zen-bridge'
import { ZEN_CUSTOM_VERBS, ZEN_VERBS } from './zen-verbs.generated'
import { ZEN_TEMPLATES_FALLBACK, zenWidgetMayRun } from './zen-custom-types'
import { ZEN_STORE_VERBS } from './zen-custom-store'
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
    // Widget caps = exposed to `widget` or `garden-local` (Garden data, contract-garden-data-v1 §7.2).
    expect(
      catalog.caps.filter((c) => c.exposure?.includes('widget') || c.exposure?.includes('garden-local')).map((c) => c.name),
    ).toEqual([...USER_WIDGET_CAPS])
  })
})

describe('day-0: verb tables', () => {
  it('placeholder ZEN_VERBS equals the bridge table key for key', () => {
    expect(ZEN_VERBS).toEqual(BRIDGE_ZEN_VERBS)
  })

  it('custom verbs are a subset of the bridge verbs (plus the frame-only rows), with widget caps only', () => {
    for (const [verb, row] of Object.entries(ZEN_CUSTOM_VERBS)) {
      if (verb === 'theme.changed') {
        expect(row.kind).toBe('event')
        continue
      }
      // 0.45.3: the whole canvas, handled by the frame host only (no cap).
      if (verb === 'canvas.setHitRegions' || verb === 'window.startDrag') {
        expect(row).toEqual({ cap: null, reach: 'local', kind: 'call' })
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // Catalog v20 (0.46.3 r10): collapsible columns, frame-only (no cap).
      if (verb.startsWith('layout.')) {
        expect(row).toEqual({ cap: null, reach: 'local', kind: verb === 'layout.subscribe' ? 'subscribe' : 'call' })
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // Catalog v21 (0.46.4): k2.open(url), answered by the custom layer only.
      if (verb === 'links.open') {
        expect(row).toEqual({ cap: 'links:open', reach: 'local', kind: 'call' })
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // Garden v3: the data sources verbs, answered by the custom layer only.
      if (verb.startsWith('sources.')) {
        expect(row.cap).toBe('sources:read')
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // Room-scope surfaces (0.46.1): garden-local through this computer's
      // daemon; room.members / app.* are also App rows (portable).
      if (verb === 'room.members' || verb === 'app.self' || verb === 'app.usage' || verb === 'widget.self' || verb === 'app.resources') {
        expect(row.reach).toBe(verb === 'widget.self' || verb === 'app.resources' ? 'local' : 'portable')
        expect([null, 'room:members']).toContain(row.cap)
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // 0.46.5 (prd-garden-board-needs-v1): blobs.put/get and downloads.*
      // are frame rows the custom layer answers (bytes over the widget door
      // and K2's download path); blobs.info/list are garden-local.
      if (verb === 'blobs.put' || verb === 'blobs.get' || verb.startsWith('downloads.')) {
        expect(row.reach).toBe('local')
        expect(['store:read', 'store:write', 'downloads:save']).toContain(row.cap)
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // Garden data: garden-local rows, answered by this computer's daemon.
      if (verb.startsWith('store.') || verb.startsWith('save.') || verb.startsWith('blobs.') || verb === 'viewer.get') {
        expect(row.reach).toBe('local')
        expect([null, 'store:read', 'store:write']).toContain(row.cap)
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // RS-A15: guests.* rows ride both the App route and the Garden door
      // (portable), answered by this computer's daemon in a Garden.
      if (verb === 'guests.rooms' || verb === 'guests.setRooms' || verb === 'guests.list' || verb === 'app.rooms') {
        expect(row.reach).toBe('portable')
        // Catalog v22: app.rooms has no cap row (the App reads its own
        // ceiling); in a Garden the daemon still asks for guests:rooms.
        expect(row.cap).toBe(verb === 'app.rooms' ? null : 'guests:rooms')
        expect(verb in ZEN_VERBS, verb).toBe(false)
        continue
      }
      // garden-person-rows-v1 (0.46.3): App rows the Garden shares, answered
      // by this computer's daemon as the person (portable, Garden-door caps).
      if (ZEN_STORE_VERBS.has(verb)) {
        expect(row.reach).toBe('portable')
        expect(['thread:read', 'files:read', 'tickets:read', 'tickets:post', 'activity:read']).toContain(row.cap)
        expect(USER_WIDGET_CAPS as readonly string[]).toContain(row.cap)
        continue
      }
      expect(verb in ZEN_VERBS, verb).toBe(true)
      expect(ZEN_VERBS[verb as keyof typeof ZEN_VERBS], verb).toBe(row.cap)
      if (row.cap) expect(USER_WIDGET_CAPS as readonly string[]).toContain(row.cap)
    }
    // thread.markRead left this list with Thread parity (catalog v16, 0.46.3,
    // Rosson 2026-10-20): a widget marks the person's own read cursor.
    expect('thread.markRead' in ZEN_CUSTOM_VERBS).toBe(true)
    for (const never of ['zen.exit', 'controls.bind', 'homes.list', 'agents.local', 'agents.add', 'thread.void']) {
      expect(never in ZEN_CUSTOM_VERBS, never).toBe(false)
    }
  })
})

describe('day-0: widget origin and templates', () => {
  it('no review: a widget from your own Garden runs; anything else waits for v4', () => {
    expect(zenWidgetMayRun({ origin: 'local' })).toBe(true)
    expect(zenWidgetMayRun({ origin: 'other' })).toBe(false)
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
    expect(ZEN_LIB_MANIFEST.schema).toBe(2)
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
