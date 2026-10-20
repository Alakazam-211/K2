// 0.46.3: `k2 zen widget preview --shot`'s runner (cli/k2-widget-shot.mjs)
// renders a widget headless in WebKit and writes the PNG. The answer it
// gets is built here the way the daemon builds it (widget_shot.rs): the
// real frame (`frameSrcDoc` + `zenFramePrelude`, pinned to the daemon's by
// src/shared/zen-frame-shape.golden.html) and the daemon's own host page
// template (crates/k2-core/src/zen/widget_shot_host.html) with its slots
// filled. The card is K2's k2:fleet-grid@2 as a staged folder would bundle.
//
//   bun run test:webkit   (npx playwright install webkit first)

import { execFileSync } from 'node:child_process'
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { PNG } from 'pngjs'
import { afterAll, describe, expect, it } from 'vitest'
import { frameSrcDoc } from '../frame-csp'
import { zenFramePrelude } from './zen-custom-prelude'

const root = resolve(__dirname, '../../../..')
const widgets = resolve(root, 'crates/k2-core/src/zen/builtin_widgets')
const runner = resolve(root, 'cli/k2-widget-shot.mjs')
const tmp = mkdtempSync(join(tmpdir(), 'k2-widget-shot-'))
/** ZEN_SHOTS=<dir> keeps the PNGs (for the report). */
const keep = process.env.ZEN_SHOTS

afterAll(() => rmSync(tmp, { recursive: true, force: true }))

const NONCE = 'BBBBBBBBBBBBBBBBBBBBBB'
const GARDEN = {
  dark: { canvas: '#121316', rgb: [18, 19, 22], vars: { '--zen-canvas': '#121316', '--zen-text': '#e6e8ec', '--zen-text-muted': '#9aa0ab', '--zen-border': '#2e3036', '--zen-surface-raised': '#222328' } },
  light: { canvas: '#f7f8fa', rgb: [247, 248, 250], vars: { '--zen-canvas': '#f7f8fa', '--zen-text': '#1a1d23', '--zen-text-muted': '#5f6672', '--zen-border': '#e3e6ec', '--zen-surface-raised': '#ffffff' } },
} as const

/** The bundle as the daemon makes it: k2:fleet-grid@2's scripts and style inline, nonced. */
function fleetGridBundle(): string {
  const kit = resolve(widgets, 'sources-kit-2')
  const css = readFileSync(resolve(kit, 'kit.css'), 'utf8')
  const scripts = [readFileSync(resolve(kit, 'kit.js'), 'utf8'), readFileSync(resolve(widgets, 'fleet-grid-2', 'card.js'), 'utf8')]
  return `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Fleet grid</title><style>${css}</style></head><body><div id="card" aria-live="polite"></div>${scripts
    .map((s) => `<script nonce="${NONCE}">${s.replace(/<\/(script)/gi, '<\\/$1')}</script>`)
    .join('')}</body></html>`
}

/** `script_json` in widget_shot.rs. */
const scriptJson = (v: unknown): string =>
  JSON.stringify(v).replace(/</g, '\\u003c').replace(/_/g, '\\u005f').replace(/\u2028/g, '\\u2028').replace(/\u2029/g, '\\u2029')

function answer(scheme: 'dark' | 'light', width: number, height: number, data: Record<string, unknown> = {}): Record<string, unknown> {
  const g = GARDEN[scheme]
  const prelude = zenFramePrelude({ nonce: NONCE, theme: { vars: { ...g.vars }, scheme }, libs: [] })
  const frame = frameSrcDoc(fleetGridBundle(), 'widget', { nonce: NONCE, prelude })
  const hello = {
    k2: 'hello',
    v: 1,
    caps: ['sources:read'],
    features: ['zen-v1', 'zen-gardens-v1', 'zen-widgets-v1', 'sources-v1', 'garden-data-v1', 'guests-rooms-v1'],
    widget: { id: 'shot', name: 'Fleet grid', garden: 'shot' },
    config: { source: 'fleet' },
    motion: { reduced: true },
  }
  const theme = { theme: { scheme, vars: g.vars }, chrome: { corners: null, stoplights: null }, motion: { reduced: true } }
  const host = readFileSync(resolve(root, 'crates/k2-core/src/zen/widget_shot_host.html'), 'utf8')
    .split('__K2_SHOT_SCHEME__').join(scheme)
    .split('__K2_SHOT_CANVAS__').join(g.canvas)
    .split('__K2_SHOT_WIDTH__').join(String(width))
    .split('__K2_SHOT_HEIGHT__').join(String(height))
    .split('__K2_SHOT_HELLO__').join(scriptJson(hello))
    .split('__K2_SHOT_THEME__').join(scriptJson(theme))
    .split('__K2_SHOT_DATA__').join(scriptJson(data))
  return { ok: true, status: 'ok', widget: 'fleet-grid', nonce: NONCE, frame, host, libs: [], width, height, scheme }
}

interface Run {
  code: number
  out: Record<string, any>
  png: PNG
}

function shoot(tag: string, a: Record<string, unknown>): Run {
  const page = join(tmp, `${tag}.json`)
  const png = join(keep ?? tmp, `fleet-grid-${tag}.png`)
  if (keep) mkdirSync(keep, { recursive: true })
  writeFileSync(page, JSON.stringify(a))
  let code = 0
  let stdout = ''
  try {
    stdout = execFileSync('node', [runner, '--page', page, '--out', png], { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] })
  } catch (e) {
    const err = e as { status: number; stdout: string; stderr: string }
    code = err.status
    stdout = err.stdout
    if (!stdout) throw new Error(`the runner failed with no answer (exit ${code}): ${err.stderr}`)
  }
  return { code, out: JSON.parse(stdout.trim()), png: PNG.sync.read(readFileSync(png)) }
}

function px(p: PNG, x: number, y: number): number[] {
  const i = (p.width * y + x) * 4
  return [p.data[i], p.data[i + 1], p.data[i + 2]]
}
const near = (a: number[], b: readonly number[]): boolean => a.every((v, i) => Math.abs(v - b[i]) <= 6)

describe('k2 zen widget preview --shot: the runner (WebKit)', () => {
  it('a dark Garden shot is dark, at 2x, with the sample data drawn', () => {
    const r = shoot('dark', answer('dark', 420, 360))
    expect(r.code).toBe(0)
    expect(r.out).toMatchObject({ ok: true, ready: true, errors: [], scheme: 'dark', width: 420, height: 360 })
    expect(r.out.calls).toEqual(expect.arrayContaining(['theme.changed', 'sources.subscribe']))
    expect([r.png.width, r.png.height]).toEqual([840, 720])
    const corner = px(r.png, r.png.width - 6, r.png.height - 6)
    expect(near(corner, GARDEN.dark.rgb), `corner rgb(${corner.join(',')})`).toBe(true)
    // Something was drawn on the card (the sample tiles), not just canvas.
    let drawn = 0
    for (let y = 0; y < r.png.height; y += 8) for (let x = 0; x < r.png.width; x += 8) if (!near(px(r.png, x, y), GARDEN.dark.rgb)) drawn++
    expect(drawn).toBeGreaterThan(200)
  })

  it('a light Garden shot is light', () => {
    const r = shoot('light', answer('light', 420, 360))
    expect(r.code).toBe(0)
    const corner = px(r.png, r.png.width - 6, r.png.height - 6)
    expect(near(corner, GARDEN.light.rgb), `corner rgb(${corner.join(',')})`).toBe(true)
  })

  it('--data fixtures replace the samples', () => {
    const now = Math.floor(Date.now() / 1000)
    const view = {
      name: 'fleet', label: 'Fleet', health: 'crit', phase: 'live', display: 'crit',
      items: [{ key: 'only-box', health: 'crit', reason: 'unreachable', fields: {} }],
      counts: { ok: 0, warn: 0, crit: 1, unknown: 0 }, soft: { count: 0, of: 3 }, fields: {}, serverNow: now, updatedAt: now,
    }
    const r = shoot('data', answer('dark', 300, 200, { 'sources.subscribe fleet': view }))
    expect(r.code).toBe(0)
    expect([r.png.width, r.png.height]).toEqual([600, 400])
  })

  it('a widget that never says ready exits 1 and still writes the PNG', () => {
    const a = answer('dark', 200, 150)
    a.frame = (a.frame as string).replace('C.mount', 'throw new Error("boom"); C.mount')
    const r = shoot('broken', a)
    expect(r.code).toBe(1)
    expect(r.out.ready).toBe(false)
    expect(r.out.errors.join(' ')).toContain('boom')
    expect(r.png.width).toBe(400)
  })

  it('usage problems exit 2 before any render', () => {
    let code = 0
    try {
      execFileSync('node', [runner, '--page', join(tmp, 'nope.json')], { cwd: root, stdio: 'pipe' })
    } catch (e) {
      code = (e as { status: number }).status
    }
    expect(code).toBe(2)
  })
})
