// Garden data — the renderer half of a widget's own data
// (prd-gardens-local-services-v1 1A; contract-garden-data-v1 §7). One per
// placement's frame, built beside the custom layer.
//
// - Calls: every `garden-local` verb (`store.*`, `save.*`, `viewer.get`)
//   goes to THIS computer's daemon as `POST /cli/zen/call {garden,
//   placement, hash, verb, args}` with the owner token. The daemon checks
//   the placement, the live hash, the caps (the manifest's, no taint since 0.46.1) and the
//   widget maker's rights; this file only carries the call and turns the
//   daemon's error envelope into the shared error list.
// - `by`: the daemon answers `you`, `you via <widget>` or `agent:<handle>`;
//   K2 names "you" with Settings' name for the person (`viewer.get`,
//   BG9), so a Board labels your notes "Baden" and a rename shows
//   everywhere.
// - Push: `k2.store.subscribe(prefix, cb)` arrives as a subscription on
//   `store.changes`. Each subscription keeps its own cursor in this widget's
//   `data:` space. On `garden_data_changed {space, rev}` (local scope only)
//   it pages `store.changes` until `next` is null, buffering while
//   `partial`, so a subscriber never sees half a rev; then it hands the
//   widget `[{key, rev, value | null, by}]`. The first delivery is the whole
//   current state (a replay from rev 0). `resync` (the cursor is older than
//   kept history) lists everything again and hands `{resync: true, items}`.
// - `stale_widget` (the frame runs an old version) reloads the widget once.

import type { CatalogErrorCode, CatalogWireError } from '../contract/catalog-types'
import { CATALOG_ERROR_CODES } from '../contract/catalog-types'

/** Every verb this file answers (the catalog's `garden-local` rows). */
export const ZEN_STORE_VERBS: ReadonlySet<string> = new Set([
  'store.get',
  'store.put',
  // 0.46.4: a field-level merge that keeps fields it doesn't name.
  'store.patch',
  'store.delete',
  'store.list',
  'store.changes',
  'store.batch',
  'store.txBegin',
  'store.txAdd',
  'store.txCommit',
  'store.txAbort',
  'save.get',
  'save.set',
  'save.remove',
  'save.keys',
  'viewer.get',
  // Room-scope surfaces (0.46.1, one reveal = App gateway + Garden bridge):
  // the same daemon door answers them, as the owner.
  'room.members',
  'app.self',
  'app.usage',
  'widget.self',
  // Box usage's Garden card (0.46.4, disk guard Slice B): read only.
  'app.resources',
  // garden-person-rows-v1 (0.46.3): the App rows a Garden shares, answered
  // by the same daemon door as the person on this computer (no project
  // chat on the bridge, Rosson 2026-10-20).
  'thread.catchup',
  'thread.unread',
  'thread.mute',
  'files.list',
  'files.read',
  'files.readBinary',
  'tickets.list',
  'tickets.show',
  'tickets.create',
  'tickets.comment',
  'tickets.resolve',
  'activity.snapshot',
  // 0.46.5 (prd-garden-board-needs-v1 GB13): a file's details and the list.
  'blobs.info',
  'blobs.list',
])

/** How long the person's name is reused before `viewer.get` again. */
export const ZEN_VIEWER_NAME_MS = 30_000

/** A `rate_limited` page fetch for a subscription retries after this. */
export const ZEN_STORE_RETRY_MS = 1_000

const KNOWN: ReadonlySet<string> = new Set(CATALOG_ERROR_CODES)

/** A refusal in the shared error list (the bridge passes it to the frame). */
export class ZenStoreError extends Error {
  readonly wire: CatalogWireError
  /** The daemon's own code (`bad_request`, `widget_not_found`, …), even
   *  when the frame sees `failed`. */
  readonly daemonCode: string
  readonly reapedRev: number | null
  constructor(daemonCode: string, message: string, extra: { last?: unknown; reapedRev?: number } = {}) {
    super(message)
    this.name = 'ZenStoreError'
    this.daemonCode = daemonCode
    const code = (KNOWN.has(daemonCode) ? daemonCode : 'failed') as CatalogErrorCode
    this.wire = { code, message, ...(extra.last !== undefined ? { last: extra.last } : {}) }
    this.reapedRev = extra.reapedRev ?? null
  }
}

/** The daemon's `{"error": {code, hint, …}}` envelope (thrown as the
 *  message text by the daemon client) as a `ZenStoreError`. */
export function zenStoreErrorFrom(err: unknown): ZenStoreError {
  if (err instanceof ZenStoreError) return err
  const text = err instanceof Error ? err.message : String(err)
  let env: unknown = null
  try {
    env = JSON.parse(text)
  } catch {
    env = null
  }
  const e = env !== null && typeof env === 'object' ? (env as { error?: unknown }).error : undefined
  if (e !== null && typeof e === 'object') {
    const o = e as Record<string, unknown>
    const code = typeof o.code === 'string' ? o.code : 'failed'
    let message = typeof o.hint === 'string' && o.hint ? o.hint : code
    let last: unknown
    if (code === 'conflict' && typeof o.key === 'string' && typeof o.rev === 'number') {
      last = { key: o.key, rev: o.rev }
      message = `${message} (now rev ${o.rev})`
    }
    const reapedRev = typeof o.reapedRev === 'number' ? o.reapedRev : undefined
    return new ZenStoreError(code, message, { last, reapedRev })
  }
  return new ZenStoreError('failed', text || 'Garden data call failed')
}

/** Who wrote, in the person's words: `you` → their name. */
export function zenStoreBy(by: unknown, name: string): unknown {
  if (by === 'you') return name
  if (typeof by === 'string' && by.startsWith('you via ')) return `${name} via ${by.slice('you via '.length)}`
  return by
}

export interface ZenCustomStoreDeps {
  /** `POST /cli/zen/call` for this placement and the hash its frame runs. */
  call(verb: string, args: unknown[]): Promise<unknown>
  /** `garden_data_changed` on this computer's app bus. */
  onChanged(cb: (space: string, rev: number) => void): () => void
  /** This widget's shared space: `data:<widget>` (a built-in without `@n`). */
  space: string
  /** The daemon said `stale_widget`: reload the frame (once). */
  onStale(): void
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(t: unknown): void
  now(): number
}

export interface ZenCustomStore {
  /** One `garden-local` verb; `by` comes back in the person's words. */
  call(verb: string, args: unknown[]): Promise<unknown>
  /** `k2.store.subscribe`: `arg` is a prefix or `{prefix?, since?, from?}`
   *  (`from`: a neighbour widget named in the manifest's `reads`, 0.46.1). */
  subscribe(arg: unknown, push: (value: unknown) => void, fail: (error: CatalogWireError) => void): () => void
}

/** The space a widget's data lives in (`k2:diary@2` → `data:k2:diary`). */
export function zenDataSpace(widget: string): string {
  const w = widget.startsWith('k2:') ? widget.split('@')[0] : widget
  return `data:${w}`
}

type Row = { key: string; rev: number; value: unknown; by: unknown }

function isObj(v: unknown): v is Record<string, unknown> {
  return v !== null && typeof v === 'object' && !Array.isArray(v)
}

export function createZenCustomStore(deps: ZenCustomStoreDeps): ZenCustomStore {
  let name: { value: string; at: number } | null = null
  let staleSent = false

  const raw = async (verb: string, args: unknown[]): Promise<unknown> => {
    try {
      return await deps.call(verb, args)
    } catch (err) {
      const e = zenStoreErrorFrom(err)
      if (e.daemonCode === 'stale_widget' && !staleSent) {
        staleSent = true
        deps.onStale()
      }
      throw e
    }
  }

  const viewerName = async (): Promise<string> => {
    const now = deps.now()
    if (name && now - name.at < ZEN_VIEWER_NAME_MS) return name.value
    const v = await raw('viewer.get', [])
    const n = isObj(v) && typeof v.name === 'string' && v.name ? v.name : 'You'
    name = { value: n, at: now }
    return n
  }

  /** `by` fields in an answer, in the person's words. */
  const named = async (verb: string, v: unknown): Promise<unknown> => {
    if (verb === 'store.get' && isObj(v)) return { ...v, by: zenStoreBy(v.by, await viewerName()) }
    if ((verb === 'store.list' || verb === 'store.changes') && isObj(v) && Array.isArray(v.items)) {
      const n = await viewerName()
      return { ...v, items: v.items.map((i: unknown) => (isObj(i) ? { ...i, by: zenStoreBy(i.by, n) } : i)) }
    }
    return v
  }

  const call = async (verb: string, args: unknown[]): Promise<unknown> => {
    if (!ZEN_STORE_VERBS.has(verb)) throw new ZenStoreError('unknown_verb', `No verb ${verb}.`)
    const v = await raw(verb, args)
    return named(verb, v)
  }

  const subscribe = (
    arg: unknown,
    push: (value: unknown) => void,
    fail: (error: CatalogWireError) => void,
  ): (() => void) => {
    let prefix = ''
    let since = 0
    let from: string | null = null
    if (typeof arg === 'string') prefix = arg
    else if (isObj(arg)) {
      if (arg.prefix !== undefined) {
        if (typeof arg.prefix !== 'string') throw new ZenStoreError('failed', 'store.subscribe: prefix is text')
        prefix = arg.prefix
      }
      if (arg.since !== undefined) {
        if (typeof arg.since !== 'number' || !Number.isInteger(arg.since) || arg.since < 0) {
          throw new ZenStoreError('failed', 'store.subscribe: since is a whole number, 0 or more')
        }
        since = arg.since
      }
      if (arg.from !== undefined && arg.from !== null) {
        if (typeof arg.from !== 'string' || !arg.from) throw new ZenStoreError('failed', 'store.subscribe: from is a widget name')
        from = arg.from
      }
    } else if (arg !== undefined && arg !== null) {
      throw new ZenStoreError('failed', 'store.subscribe takes a prefix or {prefix, since, from}')
    }
    // 0.46.1 (gap 5): a neighbour's data rides its own space's pushes; the
    // daemon checks `from` against the manifest's `reads` on every page.
    const space = from === null ? deps.space : zenDataSpace(from)
    const fromOpt = from === null ? {} : { from }
    let live = true
    let running = false
    let again = false
    let first = true
    let timer: unknown = null
    const cursor: { since: number; after?: string } = { since }

    const end = (e: ZenStoreError): void => {
      if (!live) return
      live = false
      off()
      fail(e.wire)
    }

    /** Every key under the prefix (a resync), by pages. */
    const listAll = async (): Promise<unknown[]> => {
      const items: unknown[] = []
      let after: string | undefined
      for (;;) {
        const page = (await call('store.list', [{ prefix, ...(after !== undefined ? { after } : {}), limit: 500, ...fromOpt }])) as {
          items: unknown[]
          next: string | null
        }
        items.push(...page.items)
        if (!page.next) return items
        after = page.next
      }
    }

    const pump = async (): Promise<void> => {
      if (!live) return
      if (running) {
        again = true
        return
      }
      running = true
      try {
        do {
          again = false
          const rows: Row[] = []
          let head = cursor.since
          let at: { since: number; after?: string } = { ...cursor }
          for (;;) {
            let page: { head: number; items: Row[]; next: { since: number; after?: string } | null; partial: boolean }
            try {
              page = (await call('store.changes', [
                { since: at.since, ...(at.after !== undefined ? { after: at.after } : {}), prefix, ...fromOpt },
              ])) as typeof page
            } catch (err) {
              const e = zenStoreErrorFrom(err)
              if (e.daemonCode !== 'resync') throw e
              const items = await listAll()
              if (!live) return
              push({ resync: true, items })
              first = false
              // Replays from the kept history are the same current state.
              cursor.since = e.reapedRev ?? 0
              delete cursor.after
              again = true
              rows.length = 0
              break
            }
            rows.push(...page.items.map((i) => ({ key: i.key, rev: i.rev, value: i.value, by: i.by })))
            head = page.head
            if (!page.next) {
              // Every row up to the head the last page saw is in.
              cursor.since = Math.max(head, cursor.since)
              delete cursor.after
              if (rows.length > 0 || first) {
                first = false
                if (!live) return
                push(rows)
              }
              break
            }
            at = { ...page.next }
          }
        } while (again && live)
      } catch (err) {
        const e = zenStoreErrorFrom(err)
        if (e.daemonCode === 'rate_limited' && live) {
          timer = deps.setTimer(() => {
            timer = null
            void pump()
          }, ZEN_STORE_RETRY_MS)
        } else {
          end(e)
        }
      } finally {
        running = false
      }
    }

    const offChanged = deps.onChanged((changedSpace, rev) => {
      if (changedSpace !== space || !live) return
      if (rev <= cursor.since && !running) return
      void pump()
    })
    const off = (): void => {
      live = false
      offChanged()
      if (timer !== null) deps.clearTimer(timer)
      timer = null
    }
    void pump()
    return off
  }

  return { call, subscribe }
}
