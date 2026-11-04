// A stand-in for `/cli/board/file/*`, following board-tab-b3-interface.md, for the client's tests (vitest and the
// Playwright specs in tests/board-file). It holds one file's objects as JSON, and compares every rev on every save, as
// the note says the daemon does. Real saves and real files are the daemon's own tests.

import { DaemonHttpError } from '@/lib/daemon-http-error'

type Json = Record<string, unknown>
export type FakeObject = Json & { id: string; type: string }

export interface FakeFileOpts {
  objects?: FakeObject[]
  meta?: Json
  people?: Json
  me?: string
  canWrite?: boolean
  missing?: string[]
  blobs?: Record<string, { mime: string; bytes: Uint8Array; display?: boolean }>
}

export interface FakeBoardDaemon {
  get<T = unknown>(route: string, params: Record<string, string | number | undefined>): Promise<T>
  post<T = unknown>(route: string, body: unknown): Promise<T>
  /** Every write route called, in order. */
  writes: { route: string; body: Json }[]
  /** Every route called. */
  calls: string[]
  /** An agent or outside writer changes an object. */
  outsideSet(id: string, f: Json): void
  outsideAdd(o: FakeObject): void
  outsideDelete(id: string): void
  hash(): string
  objects(): FakeObject[]
  /** The next calls of these routes answer this. */
  failNext(route: string, err: Error | 'network', times?: number): void
  /** Every call answers this until cleared. */
  failAll(err: Error | 'network' | null): void
  setEmpty(on: boolean): void
  setLarge(bytes: number | null): void
  setMissing(hashes: string[]): void
  /** Hold save answers until released. */
  holdSaves(): () => void
  copies: { to: string; body: Json; kind: 'copy' | 'rescue' }[]
  staged: Set<string>
  /** Answer a save with `blob_missing`, once per call, while on. */
  setSaveRefusal(r: { code: string; hint?: string } | null): void
  /** The daemon stores a person's held edits (BT-24), as the store on disk would, across tabs and restarts. */
  storeHeld(on: boolean): void
  stored(): { heldId: string; groupId: string; ops: Json[]; objects: unknown[]; mine?: Json }[]
  /** A 404 for failAll, from a page that can't import the error class. */
  notFound(): Error
  setCanWrite(v: boolean): void
}

export const httpErr = (status: number, code: string, extra: Json = {}, hint = code): DaemonHttpError =>
  new DaemonHttpError(hint, status, code, { code, hint, ...extra }, { error: { code, hint, ...extra }, ...extra })

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T

export function createFakeBoardDaemon(opts: FakeFileOpts = {}): FakeBoardDaemon {
  const objects = new Map<string, FakeObject>()
  for (const o of opts.objects ?? []) objects.set(o.id, clone(o))
  let order = [...objects.keys()]
  const revs = new Map<string, number>()
  let n = 1
  for (const id of order) revs.set(id, n++)
  let settings = 1
  const meta: Json = clone(opts.meta ?? {})
  const people: Json = clone(opts.people ?? {})
  const me = opts.me ?? 'p-me'
  let canWrite = opts.canWrite !== false
  let missing = [...(opts.missing ?? [])]
  let version = 1
  let empty = false
  let large: number | null = null
  const writes: FakeBoardDaemon['writes'] = []
  const calls: string[] = []
  const nextFail = new Map<string, { err: Error | 'network'; times: number }>()
  let allFail: Error | 'network' | null = null
  const answered = new Map<string, unknown>()
  let gate: Promise<void> | null = null
  const copies: FakeBoardDaemon['copies'] = []
  const staged = new Set<string>()
  const blobs = opts.blobs ?? {}
  let saveRefusal: { code: string; hint?: string } | null = null
  let held: { heldId: string; groupId: string; byAll: boolean; ops: Json[]; objects: unknown[]; mine?: Json }[] | null = null
  let heldSeq = 0

  const hash = (): string => `sha256-v${version}`
  const revOf = (id: string): string => `r${revs.get(id)}`
  const bump = (id: string): void => {
    revs.set(id, n++)
    version += 1
  }
  const readAnswer = (): Json => {
    const list = order.map((id, i) => ({ ...clone(objects.get(id)!), z: i + 1 }))
    return {
      shape: 1,
      hash: hash(),
      version: 1,
      minReader: 1,
      canWrite,
      objects: list,
      revs: Object.fromEntries(order.map((id) => [id, revOf(id)])),
      settingsRev: `s${settings}`,
      meta: clone(meta),
      people: clone(people),
      me,
      missing,
      ...(held !== null ? { held: clone(held) } : {}),
    }
  }
  const check = (route: string): void => {
    calls.push(route)
    if (allFail) {
      if (allFail === 'network') throw new TypeError('fetch failed')
      throw allFail
    }
    const f = nextFail.get(route)
    if (f && f.times > 0) {
      f.times -= 1
      if (f.err === 'network') throw new TypeError('fetch failed')
      throw f.err
    }
  }
  const place = (id: string, to: unknown): void => {
    order = order.filter((x) => x !== id)
    if (to === 'back') order.unshift(id)
    else if (to && typeof to === 'object' && Array.isArray((to as { after?: unknown }).after)) {
      const near = ((to as { after: string[] }).after).find((x) => order.includes(x))
      if (near) order.splice(order.indexOf(near) + 1, 0, id)
      else order.unshift(id)
    } else order.push(id)
  }

  const save = (body: Json): Json & { held: unknown[] } => {
    const out = {
      hash: '',
      wrote: false,
      stale: body.baseHash !== hash(),
      revs: {} as Record<string, string>,
      settingsRev: '',
      me,
      removed: {} as Record<string, unknown>,
      moved: [] as unknown[],
      held: [] as { groupId: string; heldId?: string; objects: unknown[] }[],
      refused: [] as unknown[],
    }
    for (const g of body.groups as { id: string; noHold?: boolean; byAll?: boolean; heldId?: string; ops: Json[] }[]) {
      if (answered.has(g.id)) continue
      const conflicts: { id: string; why: string; theirs: unknown }[] = []
      for (const op of g.ops) {
        const id = String(op.id ?? (op.obj as Json | undefined)?.id ?? (op.object as Json | undefined)?.id ?? '')
        if (op.op === 'set' || op.op === 'del') {
          if (!objects.has(id)) {
            if (op.op === 'set') conflicts.push({ id, why: 'deleted', theirs: null })
          } else if (op.rev !== revOf(id)) conflicts.push({ id, why: 'changed', theirs: clone(objects.get(id)) })
        } else if (op.op === 'add' && objects.has(id)) conflicts.push({ id, why: 'exists', theirs: clone(objects.get(id)) })
        else if (op.op === 'meta' && op.rev !== `s${settings}`) conflicts.push({ id: '', why: 'settings', theirs: clone(meta) })
      }
      if (conflicts.length) {
        if (g.noHold) out.refused.push({ groupId: g.id, code: 'board_conflict', at: conflicts[0].id })
        else if (held !== null) {
          const again = g.heldId ? held.find((h) => h.heldId === g.heldId) : undefined
          const heldId = again?.heldId ?? `h-${++heldSeq}`
          if (again) again.objects = conflicts
          else held.push({ heldId, groupId: g.id, byAll: g.byAll === true, ops: clone(g.ops), objects: conflicts })
          out.held.push({ groupId: g.id, heldId, objects: conflicts })
        } else out.held.push({ groupId: g.id, objects: conflicts })
        continue
      }
      if (saveRefusal) {
        out.refused.push({ groupId: g.id, code: saveRefusal.code, hint: saveRefusal.hint })
        continue
      }
      for (const op of g.ops) {
        const id = String(op.id ?? (op.obj as Json | undefined)?.id ?? (op.object as Json | undefined)?.id ?? '')
        if (op.op === 'add') {
          objects.set(id, clone(op.obj) as FakeObject)
          place(id, op.at)
          bump(id)
          out.revs[id] = revOf(id)
        } else if (op.op === 'putback') {
          objects.set(id, clone(op.object) as FakeObject)
          place(id, op.at)
          bump(id)
          out.revs[id] = revOf(id)
        } else if (op.op === 'set') {
          const cur = objects.get(id)!
          for (const [k, v] of Object.entries(op.f as Json)) {
            if (v === null) delete cur[k]
            else cur[k] = clone(v)
          }
          bump(id)
          out.revs[id] = revOf(id)
        } else if (op.op === 'del') {
          if (objects.has(id)) {
            out.removed[id] = clone(objects.get(id))
            objects.delete(id)
            order = order.filter((x) => x !== id)
            version += 1
          }
        } else if (op.op === 'meta') {
          Object.assign(meta, clone(op.f))
          settings += 1
          version += 1
        } else if (op.op === 'z') {
          place(id, op.to)
          version += 1
        } else if (op.op === 'clearBlob') {
          missing = missing.filter((h) => h !== op.hash)
          version += 1
        }
      }
      out.wrote = true
      answered.set(g.id, true)
      if (g.heldId && held) held = held.filter((h) => h.heldId !== g.heldId)
    }
    out.hash = hash()
    out.settingsRev = `s${settings}`
    return out
  }

  const d: FakeBoardDaemon = {
    writes,
    calls,
    copies,
    staged,
    async get<T>(route: string, params: Record<string, string | number | undefined>): Promise<T> {
      check(route)
      if (route === 'board/file/info') {
        if (empty) return { empty: true, bytes: 0, mtimeMs: 1, canWrite } as T
        return { hash: hash(), bytes: 100, mtimeMs: 1, canWrite } as T
      }
      if (route === 'board/file/read') {
        if (empty) return { empty: true } as T
        if (large !== null && !params.allowLarge) return { large: true, bytes: large } as T
        if (params.ifHash === hash()) return { same: true, hash: hash() } as T
        return readAnswer() as T
      }
      if (route === 'board/file/blob') {
        const h = String(params.hash)
        const b = blobs[h]
        if (!b) throw httpErr(404, 'not_found')
        const offset = Number(params.offset ?? 0)
        const len = Number(params.len ?? b.bytes.length)
        const slice = b.bytes.subarray(offset, offset + len)
        let s = ''
        for (const c of slice) s += String.fromCharCode(c)
        return { hash: h, mime: b.mime, total: b.bytes.length, offset, data: btoa(s), display: b.display !== false } as T
      }
      throw new Error(`fake daemon: no GET ${route}`)
    },
    async post<T>(route: string, body: unknown): Promise<T> {
      check(route)
      const b = body as Json
      writes.push({ route, body: clone(b) })
      if (route === 'board/file/save') {
        const release = gate
        if (release) await release
        const a = save(b)
        const groups = (b.groups as unknown[]).length
        if (groups > 0 && a.held.length === groups) {
          throw new DaemonHttpError('conflict', 409, 'board_conflict', { code: 'board_conflict' }, { error: { code: 'board_conflict' }, ...a })
        }
        return a as T
      }
      if (route === 'board/file/held/resolve') {
        if (held) held = held.filter((h) => h.heldId !== b.heldId)
        return { ok: true } as T
      }
      if (route === 'board/file/held/mine') {
        const h = held?.find((x) => x.heldId === b.heldId)
        if (h) h.mine = clone(b.mine) as Json
        return { ok: true } as T
      }
      if (route === 'board/file/start') {
        if (!empty) throw httpErr(409, 'not_empty')
        empty = false
        return { hash: hash() } as T
      }
      if (route === 'board/file/blob/put') {
        const { hash: h, offset, total } = b as { hash: string; offset: number; total: number }
        const data = atob(String(b.data))
        if (offset + data.length >= total) staged.add(h)
        return { received: offset + data.length, done: offset + data.length >= total } as T
      }
      if (route === 'board/file/copy') {
        const to = String(b.to)
        copies.push({ to, body: clone(b), kind: 'copy' })
        return { path: to, name: to.split('/').pop(), hash: 'sha256-copy', missing: [] } as T
      }
      if (route === 'board/file/copy/rescue') {
        const to = String(b.to)
        const refs = (b.blobs as string[]) ?? []
        const gone = refs.filter((h) => !staged.has(h))
        if (gone.length && !b.dropMissing) {
          // As the daemon answers (missing_answer in board_file/routes.rs): the list beside the error.
          throw new DaemonHttpError('missing', 409, 'missing', { code: 'missing' }, { ok: false, error: { code: 'missing' }, missing: gone })
        }
        copies.push({ to, body: clone(b), kind: 'rescue' })
        return { path: to, name: to.split('/').pop(), hash: 'sha256-copy', missing: gone } as T
      }
      throw new Error(`fake daemon: no POST ${route}`)
    },
    outsideSet(id, f) {
      Object.assign(objects.get(id)!, clone(f))
      bump(id)
    },
    outsideAdd(o) {
      objects.set(o.id, clone(o))
      order.push(o.id)
      bump(o.id)
    },
    outsideDelete(id) {
      objects.delete(id)
      order = order.filter((x) => x !== id)
      version += 1
    },
    hash,
    objects: () => order.map((id) => clone(objects.get(id)!)),
    failNext(route, err, times = 1) {
      nextFail.set(route, { err, times })
    },
    failAll(err) {
      allFail = err
    },
    setEmpty(on) {
      empty = on
    },
    setLarge(bytes) {
      large = bytes
    },
    setMissing(h) {
      missing = h
    },
    holdSaves() {
      let release = (): void => undefined
      gate = new Promise((r) => {
        release = r
      })
      return () => {
        gate = null
        release()
      }
    },
    setSaveRefusal(r) {
      saveRefusal = r
    },
    storeHeld(on) {
      held = on ? (held ?? []) : null
    },
    stored: () => clone(held ?? []),
    notFound: () => httpErr(404, 'not_found'),
    setCanWrite(v) {
      canWrite = v
    },
  }
  return d
}
