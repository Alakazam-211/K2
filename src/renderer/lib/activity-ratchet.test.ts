// prd-daemon-activity-and-thread-working-v1 T-S5b (RL9): the renderer no
// longer derives agent activity. The daemon decides it (`stores/activity.ts`
// holds its rows); none of the old client-side derivation may come back
// under `src/renderer/` — not the phrase scan, not the per-pane status
// maps, not the client/daemon merge rule.
//
// RL13: the one exception is a server WITHOUT `daemon-activity` (0.44.x),
// which has no rows to give. Its dots come from the legacy adapter
// (`stores/activity-legacy.ts`, under new names), fed by `stores/activity.ts`
// and, for a mounted pane's busy footer, by `TerminalPane`. Those names, the
// footer phrases and the old servers' event subscriptions stay inside those
// files; the deleted names stay deleted everywhere, the adapter included.

import { describe, expect, it } from 'vitest'
import { readFileSync, readdirSync } from 'node:fs'
import { dirname, join, relative, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const RENDERER = join(dirname(fileURLToPath(import.meta.url)), '..')
const SELF = 'lib/activity-ratchet.test.ts'

function walk(dir: string, out: string[]): string[] {
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, e.name)
    if (e.isDirectory()) walk(p, out)
    else if (/\.tsx?$/.test(e.name)) out.push(p)
  }
  return out
}

const FILES = walk(RENDERER, [])
  .map((f) => relative(RENDERER, f).split(sep).join('/'))
  .filter((f) => f !== SELF)

/** RL9's deleted symbols (and their stores' derivation helpers). */
const GONE = [
  'detectWorkingSignal',
  'WORKING_SIGNALS',
  'mergePaneStatus',
  'paneStatuses',
  'daemonPaneStatuses',
  'recordTitleActivity',
  'recordTitlePermission',
  'handleLifecycleEvent',
  'applyDaemonActivity',
  'applyHookStatus',
  'HOOK_TRUST_GRACE_MS',
  'ensureActivityStaleSweep',
  'createRoomActivity',
  'bindPaneAgentName',
  'bindPaneProject',
]

/** RL13 — the legacy adapter's names, and the only files that may say them. */
const LEGACY_ONLY: Record<string, readonly string[]> = {
  LEGACY_BUSY_PHRASES: ['stores/activity-legacy.ts'],
  legacyScreenShowsBusy: ['stores/activity-legacy.ts', 'stores/activity.ts'],
  legacyRows: ['stores/activity-legacy.ts', 'stores/activity.ts'],
  legacyRollups: ['stores/activity-legacy.ts', 'stores/activity.ts'],
  noteLegacyScreen: ['stores/activity.ts', 'kessel-term/TerminalPane.tsx', 'stores/activity.test.ts'],
  forgetLegacyScreen: ['stores/activity.ts', 'kessel-term/TerminalPane.tsx', 'stores/activity.test.ts'],
  isLegacyActivityServer: ['stores/activity.ts', 'kessel-term/TerminalPane.tsx', 'stores/activity.test.ts'],
}

/** An old server's activity events: defined by the bus, read only by the
 *  feed (test files that fake the bus may name them too). */
const LEGACY_EVENTS: Record<string, readonly string[]> = {
  onAgentStatusChanged: ['stores/session-events.ts', 'stores/activity.ts'],
  onSessionActivityChanged: ['stores/session-events.ts', 'stores/activity.ts'],
}

function isTestFile(f: string): boolean {
  return /\.(test|mstest)\.tsx?$/.test(f)
}

/** Who may import the adapter module. */
const LEGACY_IMPORTERS = ['stores/activity.ts', 'kessel-term/TerminalPane.tsx', 'stores/activity.test.ts']

/** Busy-footer phrases distinctive enough to spot a copy of the scan. */
const FOOTER_PHRASES = ['esc to interrupt', 'esc:cancel', 'msg=interrupt', 'ctrl+c to stop', 'planning next moves']

describe('T-S5b: no client-side activity derivation', () => {
  it('walks the renderer', () => {
    expect(FILES.length).toBeGreaterThan(300)
    expect(FILES).toContain('stores/activity.ts')
  })

  it('none of the deleted derivation symbols is named anywhere under src/renderer', () => {
    const hits: string[] = []
    for (const f of FILES) {
      const src = readFileSync(join(RENDERER, f), 'utf8')
      for (const sym of GONE) if (new RegExp(`\\b${sym}\\b`).test(src)) hits.push(`${f}: ${sym}`)
    }
    expect(hits).toEqual([])
  })

  it('RL13: the legacy adapter names live only in the legacy-scoped files', () => {
    const hits: string[] = []
    for (const f of FILES) {
      const src = readFileSync(join(RENDERER, f), 'utf8')
      for (const [sym, allowed] of Object.entries(LEGACY_ONLY)) {
        if (allowed.includes(f) || f === 'stores/activity-legacy.ts') continue
        if (new RegExp(`\\b${sym}\\b`).test(src)) hits.push(`${f}: ${sym}`)
      }
      for (const [sym, allowed] of Object.entries(LEGACY_EVENTS)) {
        if (allowed.includes(f) || isTestFile(f)) continue
        if (new RegExp(`\\b${sym}\\b`).test(src)) hits.push(`${f}: ${sym}`)
      }
      if (f !== 'stores/activity-legacy.ts' && !LEGACY_IMPORTERS.includes(f) && /activity-legacy['"]/.test(src)) {
        hits.push(`${f}: imports stores/activity-legacy`)
      }
    }
    expect(hits).toEqual([])
    // The checks above are live: every named symbol still exists in its home.
    const adapter = readFileSync(join(RENDERER, 'stores/activity-legacy.ts'), 'utf8')
    for (const sym of ['LEGACY_BUSY_PHRASES', 'legacyScreenShowsBusy', 'legacyRows', 'legacyRollups']) {
      expect(adapter).toMatch(new RegExp(`export (const|function) ${sym}\\b`))
    }
  })

  it('RL13: the busy-footer phrases appear only in the legacy adapter (and its tests)', () => {
    const hits: string[] = []
    for (const f of FILES) {
      if (f === 'stores/activity-legacy.ts' || f === 'stores/activity.test.ts') continue
      const src = readFileSync(join(RENDERER, f), 'utf8').toLowerCase()
      for (const phrase of FOOTER_PHRASES) if (src.includes(phrase)) hits.push(`${f}: ${phrase}`)
    }
    expect(hits).toEqual([])
    const adapter = readFileSync(join(RENDERER, 'stores/activity-legacy.ts'), 'utf8')
    for (const phrase of FOOTER_PHRASES) expect(adapter).toContain(`'${phrase}'`)
  })

  it('lib/agent-signals.ts (the phrase list) is gone', () => {
    expect(FILES).not.toContain('lib/agent-signals.ts')
  })

  it('the renderer never subscribes to the Tauri agent:lifecycle hook event', () => {
    const hits = FILES.filter((f) => readFileSync(join(RENDERER, f), 'utf8').includes("'agent:lifecycle'"))
    expect(hits).toEqual([])
  })
})
