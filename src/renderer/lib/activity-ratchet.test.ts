// prd-daemon-activity-and-thread-working-v1 T-S5b (RL9): the renderer no
// longer derives agent activity. The daemon decides it (`stores/activity.ts`
// holds its rows); none of the old client-side derivation may come back
// under `src/renderer/` — not the phrase scan, not the per-pane status
// maps, not the client/daemon merge rule.

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

  it('lib/agent-signals.ts (the phrase list) is gone', () => {
    expect(FILES).not.toContain('lib/agent-signals.ts')
  })

  it('the renderer never subscribes to the Tauri agent:lifecycle hook event', () => {
    const hits = FILES.filter((f) => readFileSync(join(RENDERER, f), 'utf8').includes("'agent:lifecycle'"))
    expect(hits).toEqual([])
  })
})
