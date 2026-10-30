// stash-sync-v1 (0.46.7): the desktop half of prompt stash sync. The merge
// rule, the runner against a fake server that follows the contract
// (`.k2/prds/contract-stash-sync-v1.md`: LWW on updated_at / updated_by /
// tombstone, the 50 live cap, the Homes device registry, 410 once
// removed), and the restore notes. Fail loud: no skips.
import { describe, expect, it, vi } from 'vitest'
import { coerceStashRecord, STASH_CAP, stashBeats, type StashAttachment, type StashRecord } from './record'
import type { StashWrite } from './idb'
import {
  ackPush,
  emptyStashSyncState,
  markDirty,
  mergePull,
  parsePushAnswer,
  startSync,
  STASH_SYNC_STATE_KEY,
  type PullAnswer,
  type StashSyncState,
} from './sync-engine'
import { StashSyncRunner, loadStashSyncState, noteLocalStashChange, type StashSyncTransport } from './sync-runner'
import { deviceLabel, planStashRestore, type RestoreDeps } from './restore'

const T = 1_761_840_000_000

function uid(n: number): string {
  return `00000000-0000-4000-8000-${n.toString(16).padStart(12, '0')}`
}

function rec(n: number, text: string, at: number, by: string | null = 'desk', extra: Partial<StashRecord> = {}): StashRecord {
  return {
    v: 1,
    id: uid(n),
    text,
    attachments: [],
    quotes: [],
    source: { server_id: 'rosson.k2.dev', server_label: null, user: 'rosson', agent: null, agent_name: null },
    created_at: at,
    updated_at: at,
    ...(by ? { updated_by: by } : {}),
    deleted_at: null,
    ...extra,
  }
}

function tomb(r: StashRecord, at: number, by: string): StashRecord {
  return { ...r, text: '', attachments: [], quotes: [], updated_at: at, updated_by: by, deleted_at: at }
}

// ── The merge rule ────────────────────────────────────────────────────

describe('the merge rule', () => {
  it('newest updated_at wins; a tie goes to the larger device id; a full tie to the tombstone', () => {
    const a = rec(1, 'a', T + 10, 'phone')
    const b = rec(1, 'b', T + 5, 'zzz')
    expect(stashBeats(a, b)).toBe(true)
    expect(stashBeats(b, a)).toBe(false)
    const lo = rec(1, 'lo', T, 'a-dev')
    const hi = rec(1, 'hi', T, 'b-dev')
    expect(stashBeats(hi, lo)).toBe(true)
    expect(stashBeats(lo, hi)).toBe(false)
    const live = rec(1, 'x', T, 'desk')
    const dead = tomb(live, T, 'desk')
    expect(stashBeats(dead, live)).toBe(true)
    expect(stashBeats(live, dead)).toBe(false)
    expect(stashBeats(live, { ...live })).toBe(false)
  })

  it('a pull lands only where it wins, and drops the bytes of files the winner no longer has', () => {
    const att: StashAttachment = { id: 'a1', name: 'shot.png', type: 'image/png', size: 3, device_id: 'desk', path: '/Users/r/shot.png' }
    const local = [rec(1, 'mine, newer', T + 50, 'desk', { attachments: [att] }), rec(2, 'mine', T, 'desk', { attachments: [{ ...att, id: 'a2' }] })]
    const pull: PullAnswer = {
      cursor: 7,
      full: false,
      changes: [rec(1, 'older remote', T + 10, 'phone'), tomb(local[1], T + 60, 'phone'), rec(3, 'new from phone', T + 70, 'phone')],
    }
    const w = mergePull(local, pull, emptyStashSyncState())
    expect((w.put ?? []).map((r) => r.id)).toEqual([uid(2), uid(3)])
    expect(w.deleteBlobs).toEqual(['a2'])
    expect(w.deleteRecords).toEqual([])
  })

  it('a full pull forgets what the server no longer has, unless it waits to push or is held', () => {
    const local = [rec(1, 'collected elsewhere', T), rec(2, 'saved offline', T), rec(3, 'held at the cap', T), rec(4, 'on server', T)]
    let st: StashSyncState = { ...emptyStashSyncState(), cursor: 5, started: true }
    st = markDirty(st, [local[1]])
    st = { ...st, held: { [uid(3)]: 'cap_reached' } }
    const w = mergePull(local, { cursor: 9, full: true, changes: [local[3]] }, st)
    expect(w.deleteRecords).toEqual([uid(1)])
  })

  it('a full answer behind our cursor is a server reset: nothing local is forgotten', () => {
    const local = [rec(1, 'keep me', T)]
    const st = { ...emptyStashSyncState(), cursor: 40, started: true }
    const w = mergePull(local, { cursor: 0, full: true, changes: [] }, st)
    expect(w.deleteRecords).toEqual([])
  })

  it('a push ack clears what landed, holds what was refused, and keeps edits made meanwhile', () => {
    const a = rec(1, 'a', T)
    const b = rec(2, 'b', T)
    const c = rec(3, 'c', T)
    let st = markDirty(emptyStashSyncState(), [a, b, c])
    st = markDirty(st, [{ id: c.id, updated_at: T + 9 }]) // edited during the push
    st = ackPush(st, [a, b, c], parsePushAnswer({ rejected: [{ item: { id: b.id }, reason: 'cap_reached' }] }))
    expect(Object.keys(st.dirty)).toEqual([c.id])
    expect(st.held).toEqual({ [b.id]: 'cap_reached' })
    // The next local change frees what the cap held.
    st = markDirty(st, [{ id: uid(9), updated_at: T + 20 }])
    expect(Object.keys(st.dirty).sort()).toEqual([c.id, b.id, uid(9)].sort())
    expect(st.held).toEqual({})
  })

  it('the first sync queues every local record once', () => {
    const st = startSync(emptyStashSyncState(), [rec(1, 'a', T), rec(2, 'b', T)])
    expect(st.started).toBe(true)
    expect(Object.keys(st.dirty).sort()).toEqual([uid(1), uid(2)])
    expect(startSync(st, [rec(3, 'c', T)])).toBe(st)
  })
})

describe('the record keeps the sync fields', () => {
  it('device_id, path, updated_by and unknown fields survive; "local" reads as unknown', () => {
    const raw = {
      ...rec(1, 'x', T, 'phone'),
      pinned: true,
      source: { server_id: 'local', server_label: '', user: 'rosson', agent: null, agent_name: null },
      attachments: [
        { id: 'a1', name: 'old.png', type: 'image/png', size: 1 },
        { id: 'a2', name: 'IMG.jpg', type: 'image/jpeg', size: 2, device_id: 'phone', path: null, later: 1 },
      ],
    }
    const r = coerceStashRecord(raw)
    if (!r) throw new Error('not a record')
    expect(r.updated_by).toBe('phone')
    expect((r as unknown as { pinned: boolean }).pinned).toBe(true)
    expect(r.source.server_id).toBeNull()
    expect(r.source.server_label).toBeNull()
    expect('device_id' in r.attachments[0]).toBe(false)
    expect(r.attachments[1]).toEqual({ id: 'a2', name: 'IMG.jpg', type: 'image/jpeg', size: 2, device_id: 'phone', path: null, later: 1 })
  })
})

// ── The runner against a fake server ──────────────────────────────────

/** A server that follows the contract (per person; one person here). */
class FakeServer {
  records = new Map<string, StashRecord>()
  seq = 0
  devices = new Map<string, { name: string; removed: boolean }>()
  pushes = 0
  onChange: ((cursor: number) => void) | null = null

  private state(id: string): 'active' | 'removed' | 'unknown' {
    const d = this.devices.get(id)
    return !d ? 'unknown' : d.removed ? 'removed' : 'active'
  }

  transport(): StashSyncTransport {
    const err = (status: number, code: string): Error => Object.assign(new Error(code), { status })
    return {
      supported: () => true,
      register: async (deviceId, name) => {
        this.devices.set(deviceId, { name, removed: false })
        return { ok: true }
      },
      pull: async (deviceId, since) => {
        if (this.state(deviceId) === 'removed') throw err(410, 'device_removed')
        const full = since === null || since > this.seq
        const all = [...this.records.values()]
        const changes = full ? all.filter((r) => r.deleted_at === null) : all.filter((r) => (r as unknown as { _seq: number })._seq > since)
        return { ok: true, cursor: this.seq, full, changes: changes.map(({ ...r }) => (delete (r as Partial<{ _seq: number }>)._seq, r)) }
      },
      push: async (deviceId, changes) => {
        const s = this.state(deviceId)
        if (s === 'removed') throw err(410, 'device_removed')
        if (s === 'unknown') throw err(409, 'device_not_registered')
        this.pushes++
        const next = this.seq + 1
        let applied = 0
        let superseded = 0
        const rejected: Array<{ item: unknown; reason: string }> = []
        const live = (): number => [...this.records.values()].filter((r) => r.deleted_at === null).length
        const sorted = [...changes].sort((a, b) => Number(a.deleted_at === null) - Number(b.deleted_at === null))
        for (const c of sorted) {
          const r: StashRecord = JSON.parse(JSON.stringify({ ...c, updated_by: deviceId }))
          if (r.deleted_at !== null) Object.assign(r, { text: '', attachments: [], quotes: [] })
          const ex = this.records.get(r.id)
          if (ex && !stashBeats(r, ex)) {
            superseded++
            continue
          }
          if (r.deleted_at === null && (!ex || ex.deleted_at !== null) && live() >= STASH_CAP) {
            rejected.push({ item: c, reason: 'cap_reached' })
            continue
          }
          this.records.set(r.id, Object.assign(r, { _seq: next }))
          applied++
        }
        if (applied) {
          this.seq = next
          this.onChange?.(this.seq)
        }
        return { ok: true, cursor: this.seq, applied, superseded, rejected }
      },
    }
  }

  liveTexts(): string[] {
    return [...this.records.values()].filter((r) => r.deleted_at === null).map((r) => r.text).sort()
  }
}

/** One device: an in-memory stash (the IndexedDB stand-in), its own
 *  localStorage, its runner. */
class Device {
  records = new Map<string, StashRecord>()
  blobs = new Set<string>()
  store = new Map<string, string>()
  kv = { getItem: (k: string) => this.store.get(k) ?? null, setItem: (k: string, v: string) => void this.store.set(k, v) }
  runner: StashSyncRunner

  constructor(
    readonly id: string,
    server: FakeServer,
  ) {
    this.runner = new StashSyncRunner({
      transport: server.transport(),
      kv: this.kv,
      deviceId: () => this.id,
      device: { name: `${id}'s device`, platform: 'macos' },
      local: {
        readAll: async () => [...this.records.values()].map((r) => ({ ...r })),
        apply: async (fn) => this.write(fn([...this.records.values()].map((r) => ({ ...r })))),
      },
      debounceMs: 0,
      setTimer: () => null,
      clearTimer: () => {},
    })
    this.runner.start()
  }

  write(w: StashWrite | null): void {
    if (!w) return
    for (const r of w.put ?? []) this.records.set(r.id, r)
    for (const id of w.deleteRecords ?? []) this.records.delete(id)
    for (const id of w.deleteBlobs ?? []) this.blobs.delete(id)
  }

  /** What the store does on a save or delete. */
  local(r: StashRecord): void {
    this.records.set(r.id, r)
    for (const a of r.attachments) this.blobs.add(a.id)
    noteLocalStashChange(this.kv, [r])
  }

  live(): string[] {
    return [...this.records.values()].filter((r) => r.deleted_at === null).map((r) => r.text).sort()
  }

  sync(): Promise<void> {
    return this.runner.syncNow()
  }
}

describe('the sync runner', () => {
  it('two devices of one person converge: saves, a conflict and a delete', async () => {
    const server = new FakeServer()
    const desk = new Device('d-desk', server)
    const phone = new Device('d-phone', server)
    desk.local(rec(1, 'from desk', T, 'd-desk'))
    phone.local(rec(2, 'from phone', T + 1, 'd-phone'))
    await desk.sync()
    await phone.sync()
    await desk.sync()
    expect(desk.live()).toEqual(['from desk', 'from phone'])
    expect(phone.live()).toEqual(['from desk', 'from phone'])
    // A conflict: both edit record 1; the later edit wins on both.
    desk.local({ ...rec(1, 'desk edit', T, 'd-desk'), updated_at: T + 10 })
    phone.local({ ...rec(1, 'phone edit', T, 'd-phone'), updated_at: T + 20 })
    await desk.sync()
    await phone.sync()
    await desk.sync()
    expect(desk.live()).toEqual(['from phone', 'phone edit'])
    expect(phone.live()).toEqual(['from phone', 'phone edit'])
    expect(server.liveTexts()).toEqual(['from phone', 'phone edit'])
    // A delete on the phone reaches the desk as a tombstone.
    phone.local(tomb(rec(2, '', T + 1), T + 30, 'd-phone'))
    await phone.sync()
    await desk.sync()
    expect(desk.live()).toEqual(['phone edit'])
    expect(desk.records.get(uid(2))?.deleted_at).toBe(T + 30)
    // Nothing left to push: a sync round sends nothing.
    const before = server.pushes
    await desk.sync()
    expect(server.pushes).toBe(before)
    expect(loadStashSyncState(desk.kv).dirty).toEqual({})
  })

  it('a stash_changed frame with a new cursor pulls; the same cursor does not', async () => {
    const server = new FakeServer()
    const desk = new Device('d-desk', server)
    const phone = new Device('d-phone', server)
    await desk.sync()
    phone.local(rec(1, 'hello', T, 'd-phone'))
    await phone.sync()
    const pull = vi.spyOn(desk.runner, 'syncNow')
    desk.runner.onRemoteCursor(loadStashSyncState(desk.kv).cursor ?? -1)
    expect(pull).not.toHaveBeenCalled()
    desk.runner.onRemoteCursor(server.seq)
    expect(pull).toHaveBeenCalledTimes(1)
    await desk.runner.syncNow()
    expect(desk.live()).toEqual(['hello'])
  })

  it('the server cap holds a save here (nothing dropped) until something is removed', async () => {
    const server = new FakeServer()
    const desk = new Device('d-desk', server)
    const phone = new Device('d-phone', server)
    for (let n = 1; n <= STASH_CAP; n++) phone.local(rec(n, `p${n}`, T + n, 'd-phone'))
    await phone.sync()
    expect(server.liveTexts()).toHaveLength(STASH_CAP)
    // The desk saved one offline: the server is full.
    desk.local(rec(100, 'desk extra', T + 500, 'd-desk'))
    await desk.sync()
    expect(server.liveTexts()).not.toContain('desk extra')
    expect(desk.live()).toContain('desk extra')
    expect(loadStashSyncState(desk.kv).held).toEqual({ [uid(100)]: 'cap_reached' })
    expect(desk.runner.getStatus().heldAtCap).toBe(1)
    // A full pull later doesn't forget it.
    desk.store.set(STASH_SYNC_STATE_KEY, JSON.stringify({ ...loadStashSyncState(desk.kv), cursor: null }))
    await desk.sync()
    expect(desk.live()).toContain('desk extra')
    // The person removes the oldest: the held one goes up with it.
    desk.local(tomb(rec(1, '', T + 1), T + 600, 'd-desk'))
    await desk.sync()
    expect(server.liveTexts()).toContain('desk extra')
    expect(server.liveTexts()).toHaveLength(STASH_CAP)
    expect(loadStashSyncState(desk.kv).held).toEqual({})
  })

  it('registers in the Homes registry on 409, and stops on 410 until Homes syncs again', async () => {
    const server = new FakeServer()
    const desk = new Device('d-desk', server)
    desk.local(rec(1, 'x', T, 'd-desk'))
    await desk.sync()
    expect(server.devices.get('d-desk')?.name).toBe("d-desk's device")
    expect(server.liveTexts()).toEqual(['x'])
    // Removed in Server settings.
    server.devices.set('d-desk', { name: 'Desk', removed: true })
    desk.local(rec(2, 'after removal', T + 1, 'd-desk'))
    await desk.sync()
    expect(desk.runner.getStatus().phase).toBe('removed')
    expect(server.liveTexts()).toEqual(['x'])
    await desk.sync()
    expect(desk.runner.getStatus().phase).toBe('removed')
    // Homes "Sync again" bumps the control epoch and re-registers.
    desk.kv.setItem('k2.homes.syncControl.v1', JSON.stringify({ enabled: true, epoch: 1 }))
    server.devices.set('d-desk', { name: 'Desk', removed: false })
    await desk.sync()
    expect(server.liveTexts()).toEqual(['after removal', 'x'])
  })
})

// ── Restore notes ─────────────────────────────────────────────────────

function deps(over: Partial<RestoreDeps> = {}): RestoreDeps {
  return {
    deviceId: 'd-desk',
    deviceNames: async () => new Map([['d-mac', "Rosson's MacBook"], ['d-desk', 'Desk']]),
    pathExists: async () => true,
    readBytes: async (a) => new File([new Uint8Array([1])], a.name),
    ...over,
  }
}

function att(name: string, device_id: string | null | undefined, path: string | null | undefined): StashAttachment {
  const a: StashAttachment = { id: `id-${name}`, name, type: '', size: 1 }
  if (device_id !== undefined) a.device_id = device_id
  if (path !== undefined) a.path = path
  return a
}

describe('restoring a stash', () => {
  it("a file saved on another device stays there, named from the registry", async () => {
    const names = vi.fn(async () => new Map([['d-mac', "Rosson's MacBook"]]))
    const plan = await planStashRestore(
      { attachments: [att('shot.png', 'd-mac', '/Users/r/shot.png'), att('IMG.jpg', 'd-gone', null)] },
      deps({ deviceNames: names }),
    )
    expect(plan.files).toEqual([])
    expect(plan.notes).toEqual(["shot.png wasn't restored: it's on Rosson's MacBook", "IMG.jpg wasn't restored: it's on another device"])
    expect(names).toHaveBeenCalledTimes(1)
  })

  it('a file of this device comes back only while its path still exists', async () => {
    const exists = vi.fn(async (p: string) => p !== '/Users/r/moved.png')
    const plan = await planStashRestore(
      { attachments: [att('here.png', 'd-desk', '/Users/r/here.png'), att('moved.png', 'd-desk', '/Users/r/moved.png')] },
      deps({ pathExists: exists }),
    )
    expect(plan.files.map((f) => f.name)).toEqual(['here.png'])
    expect(plan.notes).toEqual(["moved.png wasn't restored: it's no longer at /Users/r/moved.png"])
  })

  it('the path is used when this device lost its own copy; with no path and no copy, a note', async () => {
    const plan = await planStashRestore(
      { attachments: [att('a.png', 'd-desk', '/Users/r/a.png'), att('pasted.png', 'd-desk', null), att('legacy.png', undefined, undefined)] },
      deps({ readBytes: async (a) => (a.name === 'legacy.png' ? new File([new Uint8Array([1])], a.name) : null) }),
    )
    expect(plan.paths).toEqual(['/Users/r/a.png'])
    expect(plan.files.map((f) => f.name)).toEqual(['legacy.png'])
    expect(plan.notes).toEqual(["pasted.png wasn't restored: it's gone from this device"])
  })

  it("an unreachable registry still says another device; an unknown path check doesn't call the file gone", async () => {
    const plan = await planStashRestore(
      { attachments: [att('x.png', 'd-mac', null), att('y.png', 'd-desk', '/Users/r/y.png')] },
      deps({ deviceNames: async () => Promise.reject(new Error('offline')), pathExists: async () => null }),
    )
    expect(plan.notes).toEqual(["x.png wasn't restored: it's on another device"])
    expect(plan.files.map((f) => f.name)).toEqual(['y.png'])
    expect(deviceLabel(new Map([['a', '  ']]), 'a')).toBe('another device')
  })
})
