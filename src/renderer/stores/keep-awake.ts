import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseKeepAwakeBody, type KeepAwakeMode, type KeepAwakeStatus } from '@/lib/keep-awake'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onActiveHostChange } from '@/stores/connect-host'

// Heartbeat S6 — Keep awake is a setting of a server (the machine that
// should stay awake). The daemon owns the mode and the truth; this store
// only reads and sends gestures.
//
// 0.43.2 Z17: one entry per server. `primaryScope().id` is the window's own
// server (reset on every top-switcher change, Z11); a focused Home room's
// server is keyed by its Home host key and read through the room's scope.
// Only an Admin or Owner may change it (Q3, route floor Admin; the status
// says `canChange` per login). The caller says so with `mayChange`, and
// a refused change never leaves this computer.
//
// Power-helper S1: the mode and "Also with the lid closed" never show the
// admin dialog. Only Set up does, and the daemon allows it only for an
// Admin or Owner at the host itself.

export interface KeepAwakeEntry {
  /** `null` until the server answers (an older server never does). */
  status: KeepAwakeStatus | null
  error: string | null
  /** A POST is in flight. */
  busy: boolean
  /** Set up is in flight: the admin dialog is open on the host (up to 2
   *  minutes). Nothing else opens it. */
  settingUp: boolean
  /** Z35: the server has no `power/*` routes (before 0.43.0). Never poll it. */
  unavailable: boolean
}

/** Which server, the scope its requests go to, and whether this caller may
 *  change it. */
export interface KeepAwakeTarget {
  readonly key: string
  readonly scope: ServerScope
  readonly mayChange: boolean
}

interface KeepAwakeStore {
  entries: Record<string, KeepAwakeEntry>
  load: (target: KeepAwakeTarget) => Promise<void>
  /** Never shows a dialog. */
  setMode: (target: KeepAwakeTarget, mode: KeepAwakeMode) => Promise<void>
  /** "Also with the lid closed". Never shows a dialog. */
  setLidClosed: (target: KeepAwakeTarget, on: boolean) => Promise<void>
  /** Set up lid closed on the host: the one admin dialog. */
  setUp: (target: KeepAwakeTarget) => Promise<void>
  setOnBattery: (target: KeepAwakeTarget, on: boolean) => Promise<void>
}

const BLANK: KeepAwakeEntry = Object.freeze({
  status: null,
  error: null,
  busy: false,
  settingUp: false,
  unavailable: false,
})

/** A change refused on this computer (a Member, a view-only room). */
export class KeepAwakeChangeRefusedError extends Error {
  constructor(hostKey: string) {
    super(`Keep awake on ${hostKey} is read-only here: only an Admin or Owner there may change it from a room`)
    this.name = 'KeepAwakeChangeRefusedError'
  }
}

const epochs = new Map<string, number>()
function epochOf(key: string): number {
  return epochs.get(key) ?? 0
}

export function resetKeepAwakeForTests(): void {
  for (const key of epochs.keys()) epochs.set(key, epochOf(key) + 1)
  useKeepAwakeStore.setState({ entries: {} })
}

/** One server's entry (a blank one before its first answer). */
export function keepAwakeEntryFor(key: string): KeepAwakeEntry {
  return useKeepAwakeStore.getState().entries[key] ?? BLANK
}

export const useKeepAwakeStore = create<KeepAwakeStore>((set, get) => {
  const patch = (key: string, epoch: number, next: Partial<KeepAwakeEntry>): void => {
    if (epoch !== epochOf(key)) return
    set((s) => ({ entries: { ...s.entries, [key]: { ...(s.entries[key] ?? BLANK), ...next } } }))
  }

  async function send(target: KeepAwakeTarget, route: string, body: Record<string, unknown>): Promise<void> {
    const epoch = epochOf(target.key)
    if (!target.mayChange) {
      patch(target.key, epoch, { error: new KeepAwakeChangeRefusedError(target.scope.hostKey).message })
      return
    }
    patch(target.key, epoch, { busy: true })
    try {
      const status = parseKeepAwakeBody(await daemonCliPost<unknown>(target.scope, route, body))
      if (!status) throw new Error(`${route}: response has no keepAwake`)
      patch(target.key, epoch, { status, error: null })
    } catch (e) {
      patch(target.key, epoch, { error: String(e) })
    } finally {
      patch(target.key, epoch, { busy: false })
    }
  }

  return {
    entries: {},

    load: async (target) => {
      const current = get().entries[target.key] ?? BLANK
      if (current.busy || current.unavailable) return
      const epoch = epochOf(target.key)
      try {
        const status = parseKeepAwakeBody(await daemonCliGet<unknown>(target.scope, 'power/status'))
        if (!status) throw new Error('power/status: response has no keepAwake')
        patch(target.key, epoch, { status, error: null })
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        // Z35: a server before 0.43.0 has no power routes. Say so and stop
        // asking; never show another server's mode instead.
        if (msg.includes('route not found')) patch(target.key, epoch, { status: null, unavailable: true, error: null })
        else patch(target.key, epoch, { error: String(e) })
      }
    },

    setMode: (target, mode) => send(target, 'power/keep-awake', { mode }),
    setLidClosed: (target, on) => send(target, 'power/keep-awake', { lidClosed: on }),
    setUp: async (target) => {
      const epoch = epochOf(target.key)
      patch(target.key, epoch, { settingUp: true })
      try {
        await send(target, 'power/helper', { action: 'setup' })
      } finally {
        patch(target.key, epoch, { settingUp: false })
      }
    },
    setOnBattery: (target, on) => send(target, 'power/keep-awake', { onBattery: on }),
  }
})

/** Z11: a top-switcher change drops the window's entry, so the old server's
 *  mode never sits under the new server. Rooms' entries stay. */
export function resetWindowKeepAwake(): void {
  const key = primaryScope().id
  epochs.set(key, epochOf(key) + 1)
  useKeepAwakeStore.setState((s) => {
    if (!(key in s.entries)) return s
    const entries = { ...s.entries }
    delete entries[key]
    return { entries }
  })
}

onActiveHostChange(() => resetWindowKeepAwake())
