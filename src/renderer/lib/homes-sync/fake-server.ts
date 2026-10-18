// Test helper (homes-sync-v1): an in-memory home server with the same merge
// rules as `k2_core::homes_sync` (per-item last writer wins on
// (updatedAt, deviceId), tombstones, live-before-tombstones, the last-Home
// rule, a per-person cursor, devices and 410 after removal). Lets the
// renderer tests run two devices against one "server" without a daemon.
// The daemon's own tests (`homes_sync_integration.rs`, `homes_sync.rs`) are
// the truth for the real server.

import type { HomesSyncTransport } from './runner'
import { itemKey, beats, type WireItem } from './engine'

export class HttpLikeError extends Error {
  readonly status: number
  readonly code: string
  constructor(status: number, code: string) {
    super(`${status} ${code}`)
    this.status = status
    this.code = code
  }
}

interface Stored {
  item: WireItem
  seq: number
}

export class FakeHomeServer {
  readonly serverId: string
  seq = 0
  items = new Map<string, Stored>()
  devices = new Map<string, { name: string; platform: string; removed: boolean; lastSyncAt: number | null }>()
  online = true
  supported = true
  pushes = 0

  constructor(serverId = 'aaaaaaaaaaaaaaaa') {
    this.serverId = serverId
  }

  private check(): void {
    if (!this.online) throw new TypeError('Failed to fetch')
  }

  register(deviceId: string, name: string, platform: string): unknown {
    this.check()
    const d = this.devices.get(deviceId)
    this.devices.set(deviceId, { name, platform, removed: false, lastSyncAt: d?.lastSyncAt ?? null })
    return { ok: true, device: { deviceId, name, platform, current: true } }
  }

  remove(deviceId: string): unknown {
    this.check()
    const d = this.devices.get(deviceId)
    if (!d) throw new HttpLikeError(404, 'unknown_device')
    d.removed = true
    return { ok: true, removed: true, deviceId }
  }

  list(current: string): unknown {
    this.check()
    return {
      ok: true,
      devices: [...this.devices.entries()]
        .filter(([, d]) => !d.removed)
        .map(([id, d]) => ({ deviceId: id, name: d.name, platform: d.platform, lastSyncAt: d.lastSyncAt, current: id === current })),
    }
  }

  pull(deviceId: string, since: number | null): unknown {
    this.check()
    const d = this.devices.get(deviceId)
    if (d?.removed) throw new HttpLikeError(410, 'device_removed')
    if (d) d.lastSyncAt = Date.now()
    const full = since === null || since > this.seq || since < 0
    const all = [...this.items.values()]
    const changes = full ? all.filter((s) => !s.item.deleted).map((s) => s.item) : all.filter((s) => s.seq > since).map((s) => s.item)
    const redirects: Record<string, string> = {}
    for (const s of all) if (s.item.kind === 'home' && s.item.deleted && s.item.mergedInto) redirects[s.item.id] = s.item.mergedInto
    return {
      ok: true,
      person: 'owner',
      serverId: this.serverId,
      cursor: this.seq,
      full,
      changes: changes.map((c) => ({ ...c })),
      redirects,
      device: { deviceId, syncing: !!d && !d.removed },
      servers: {},
    }
  }

  push(deviceId: string, changes: WireItem[]): unknown {
    this.check()
    this.pushes++
    const d = this.devices.get(deviceId)
    if (!d) throw new HttpLikeError(409, 'device_not_registered')
    if (d.removed) throw new HttpLikeError(410, 'device_removed')
    const ordered = [...changes].sort((a, b) => Number(!!a.deleted) - Number(!!b.deleted))
    const next = this.seq + 1
    let applied = 0
    let superseded = 0
    const rejected: Array<{ item: WireItem; reason: string }> = []
    for (const raw of ordered) {
      const item = { ...raw, deviceId } as WireItem
      const k = itemKey(item)
      const ex = this.items.get(k)
      if (ex && !beats(item, ex.item)) {
        superseded++
        continue
      }
      if (item.kind === 'home' && item.deleted && ex && !ex.item.deleted) {
        const others = [...this.items.values()].filter((s) => s.item.kind === 'home' && !s.item.deleted && itemKey(s.item) !== k)
        if (others.length === 0) {
          rejected.push({ item, reason: 'last_home' })
          continue
        }
      }
      this.items.set(k, { item, seq: next })
      applied++
    }
    if (applied) this.seq = next
    return { ok: true, cursor: this.seq, applied, superseded, rejected }
  }

  /** Live Homes as `name: [row addresses]`, by order (for assertions). */
  view(): Array<{ id: string; name: string | null; rows: string[] }> {
    const live = [...this.items.values()].map((s) => s.item).filter((i) => !i.deleted)
    const homes = live.filter((i): i is Extract<WireItem, { kind: 'home' }> => i.kind === 'home').sort((a, b) => (a.order < b.order ? -1 : a.order > b.order ? 1 : 0))
    return homes.map((h) => ({
      id: h.id,
      name: h.name,
      rows: live
        .filter((i): i is Extract<WireItem, { kind: 'row' }> => i.kind === 'row' && i.homeId === h.id)
        .sort((a, b) => (a.order < b.order ? -1 : a.order > b.order ? 1 : 0))
        .map((r) => r.address),
    }))
  }

  transport(): HomesSyncTransport {
    return {
      supported: () => this.supported,
      pull: async (deviceId, since) => this.pull(deviceId, since),
      push: async (deviceId, changes) => this.push(deviceId, changes),
      register: async (deviceId, name, platform) => this.register(deviceId, name, platform),
      remove: async (deviceId) => this.remove(deviceId),
      listDevices: async (deviceId) => this.list(deviceId),
    }
  }
}
