// The checked-in browser scripts are exactly what sdk/activity/src builds
// to (scripts/zen-lib/build-k2-activity.mjs), and the Garden library's
// zen-lib.json row names their bytes. Stale: run
// `node scripts/zen-lib/build-k2-activity.mjs`, then fix the row.
import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
// @ts-expect-error a plain .mjs build script (no types)
import { buildAll } from '../../../scripts/zen-lib/build-k2-activity.mjs'

const ROOT = resolve(__dirname, '../../..')

describe('@k2/activity browser builds', () => {
  it('are fresh', async () => {
    const outs = (await buildAll()) as Array<{ out: string; text: string }>
    expect(outs.map((o) => o.out)).toEqual(['src/renderer/zen-lib/k2-activity@1.0.0/k2-activity.js', 'sdk/activity/browser/k2-activity-bar.js'])
    for (const o of outs) {
      expect(readFileSync(resolve(ROOT, o.out), 'utf8'), `${o.out} is stale: node scripts/zen-lib/build-k2-activity.mjs`).toBe(o.text)
    }
  })

  it('the library row names the checked-in file; it stays small', () => {
    const manifest = JSON.parse(readFileSync(resolve(ROOT, 'src/shared/zen-lib.json'), 'utf8')) as {
      libs: Array<{ id: string; version: string; global: string; files: Array<{ name: string; bytes: number; sha256: string }> }>
    }
    const row = manifest.libs.find((l) => l.id === 'k2-activity')
    expect(row).toMatchObject({ version: '1.0.0', global: 'K2Activity' })
    const bytes = readFileSync(resolve(ROOT, 'src/renderer/zen-lib/k2-activity@1.0.0/k2-activity.js'))
    expect(row!.files).toEqual([{ name: 'k2-activity.js', bytes: bytes.length, sha256: createHash('sha256').update(bytes).digest('hex') }])
    expect(bytes.length, 'the bar stays a small library (16 KB)').toBeLessThanOrEqual(16 * 1024)
    expect(bytes.toString('utf8').startsWith('/*! k2-activity 1.0.0 ')).toBe(true)
  })
})
