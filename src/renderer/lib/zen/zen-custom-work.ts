// 0.46.7 Workbench (catalog v40, `workbench-v1`): the renderer half of the
// Workbench's Garden rows. Built like `zen-custom-guests` and following the
// Gardens Tauri-compatibility rules (GTC-3/4/5): the daemon answers every
// data row (`POST /cli/zen/call`, `workbench_rows.rs`); this file imports no
// store, daemon client or Tauri API, only what its host hands it.
//
// - Reads (`work.list`, `git.snapshot`, `git.diffFile`, `git.log`,
//   `compute.jobs`) go straight to the daemon.
// - **Writes need the person's press** in this frame within 5 s
//   (`ZEN_PICK_GESTURE_MS`); without one they answer `failed` and nothing
//   reaches the daemon.
// - **K2's sheet.** `agents.start`, `git.land` and `agents.settle` first
//   answer `confirm_required` with the change; this file asks the host to
//   draw K2's sheet outside the frame, and repeats the call with the
//   daemon's one-time id only after the person's press there. The widget
//   never sees the id: it hears a closed answer (`{landed}`, `{settled}`,
//   `{cancelled: true}` on Cancel; `declined` for a cancelled Start).
// - One sheet at a time per frame (a widget can't stack sheets).

import type { CatalogErrorCode, CatalogWireError } from '../contract/catalog-types'
import { CATALOG_ERROR_CODES } from '../contract/catalog-types'

/** The daemon rows this file carries. */
export const ZEN_WORK_VERBS: ReadonlySet<string> = new Set([
  'work.list',
  'git.snapshot',
  'git.diffFile',
  'git.log',
  'compute.jobs',
  'agents.start',
  'agents.stop',
  'git.land',
  'agents.settle',
  'git.note',
])

/** Rows that change something: a press in the frame first. */
export const ZEN_WORK_WRITES: ReadonlySet<string> = new Set(['agents.start', 'agents.stop', 'git.land', 'agents.settle', 'git.note'])

/** What K2's sheet shows (the daemon's `change`; `kind` picks the words). */
export interface ZenWorkChange {
  kind: 'land' | 'settle' | 'start'
  [k: string]: unknown
}

export interface ZenCustomWorkDeps {
  /** `POST /cli/zen/call` for this placement (with the sheet's id when given). */
  call(verb: string, args: unknown[], confirmToken?: string): Promise<unknown>
  /** Did the person press in this frame within the last 5 s (as K2's page saw it)? */
  pressed(): boolean
  /** Draw K2's sheet outside the frame; true when the person said yes. */
  confirm(change: ZenWorkChange): Promise<boolean>
}

export interface ZenCustomWork {
  call(verb: string, args: unknown[]): Promise<unknown>
}

const KNOWN: ReadonlySet<string> = new Set(CATALOG_ERROR_CODES)

export class ZenWorkError extends Error {
  readonly wire: CatalogWireError
  readonly daemonCode: string
  /** `confirm_required`'s change and id (never passed to the frame). */
  readonly ask: { change: ZenWorkChange; secretId: string } | null
  constructor(daemonCode: string, message: string, ask: { change: ZenWorkChange; secretId: string } | null = null) {
    super(message)
    this.name = 'ZenWorkError'
    this.daemonCode = daemonCode
    this.wire = { code: (KNOWN.has(daemonCode) ? daemonCode : 'failed') as CatalogErrorCode, message }
    this.ask = ask
  }
}

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

/** The daemon's `{"error": {...}}` envelope (thrown as text by the client). */
export function zenWorkErrorFrom(err: unknown): ZenWorkError {
  if (err instanceof ZenWorkError) return err
  const text = err instanceof Error ? err.message : String(err)
  let env: unknown = null
  try {
    env = JSON.parse(text)
  } catch {
    env = null
  }
  const e = isObj(env) ? env.error : undefined
  if (isObj(e)) {
    const code = typeof e.code === 'string' ? e.code : 'failed'
    const message = typeof e.hint === 'string' && e.hint ? e.hint : code
    const change = e.change
    const kind = isObj(change) ? change.kind : undefined
    const ask =
      code === 'confirm_required' && isObj(change) && (kind === 'land' || kind === 'settle' || kind === 'start') && typeof e.secretId === 'string'
        ? { change: change as ZenWorkChange, secretId: e.secretId }
        : null
    return new ZenWorkError(code, message, ask)
  }
  return new ZenWorkError('failed', text || 'the Workbench call failed')
}

/** What a sheet-opener answers when the person pressed Cancel (GTC-5). */
function cancelled(verb: string): unknown {
  if (verb === 'git.land') return { landed: false, commits: 0, head: null, conflict: [], cancelled: true }
  if (verb === 'agents.settle') return { settled: false, discarded: false, cancelled: true }
  throw new ZenWorkError('declined', 'The person didn’t start it.')
}

export function createZenCustomWork(deps: ZenCustomWorkDeps): ZenCustomWork {
  let sheetOpen = false
  return {
    async call(verb, args) {
      if (!ZEN_WORK_VERBS.has(verb)) throw new ZenWorkError('unknown_verb', `${verb} isn’t a Workbench verb`)
      if (ZEN_WORK_WRITES.has(verb) && !deps.pressed()) {
        throw new ZenWorkError('failed', `${verb} needs a click: it works only just after the person presses in the widget.`)
      }
      try {
        return await deps.call(verb, args)
      } catch (raw) {
        const err = zenWorkErrorFrom(raw)
        if (err.daemonCode !== 'confirm_required' || !err.ask) throw err
        if (sheetOpen) throw new ZenWorkError('rate_limited', 'K2 is already asking the person about another change.')
        sheetOpen = true
        let yes: boolean
        try {
          yes = await deps.confirm(err.ask.change)
        } finally {
          sheetOpen = false
        }
        if (!yes) return cancelled(verb)
        try {
          return await deps.call(verb, args, err.ask.secretId)
        } catch (again) {
          throw zenWorkErrorFrom(again)
        }
      }
    },
  }
}
