import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseKeepAwakeBody, type KeepAwakeMode, type KeepAwakeStatus } from '@/lib/keep-awake'
import { primaryScope } from '@/kessel/server-scope'

// Heartbeat S6 — Keep awake is a setting of the window's server (the
// machine that should stay awake), like Settings. The daemon owns the
// mode and the truth; this store only reads and sends gestures.
//
// Power-helper S1: the mode and "Also with the lid closed" never show the
// admin dialog. Only Set up does, and the daemon allows it only for an
// Admin or Owner at the host itself.

interface KeepAwakeStore {
  /** `null` until the server answers (an older server never does). */
  status: KeepAwakeStatus | null
  error: string | null
  /** A POST is in flight. */
  busy: boolean
  /** Set up is in flight: the admin dialog is open on the host (up to 2
   *  minutes). Nothing else opens it. */
  settingUp: boolean
  load: () => Promise<void>
  /** Never shows a dialog. */
  setMode: (mode: KeepAwakeMode) => Promise<void>
  /** "Also with the lid closed". Never shows a dialog. */
  setLidClosed: (on: boolean) => Promise<void>
  /** Set up lid closed on the host: the one admin dialog. */
  setUp: () => Promise<void>
  setOnBattery: (on: boolean) => Promise<void>
}

export function resetKeepAwakeForTests(): void {
  useKeepAwakeStore.setState({ status: null, error: null, busy: false, settingUp: false })
}

export const useKeepAwakeStore = create<KeepAwakeStore>((set, get) => {
  async function send(route: string, body: Record<string, unknown>): Promise<void> {
    set({ busy: true })
    try {
      const status = parseKeepAwakeBody(await daemonCliPost<unknown>(primaryScope(), route, body))
      if (!status) throw new Error(`${route}: response has no keepAwake`)
      set({ status, error: null })
    } catch (e) {
      set({ error: String(e) })
    } finally {
      set({ busy: false })
    }
  }

  return {
    status: null,
    error: null,
    busy: false,
    settingUp: false,

    load: async () => {
      if (get().busy) return
      try {
        const status = parseKeepAwakeBody(await daemonCliGet<unknown>(primaryScope(), 'power/status'))
        if (!status) throw new Error('power/status: response has no keepAwake')
        set({ status, error: null })
      } catch (e) {
        set({ error: String(e) })
      }
    },

    setMode: (mode) => send('power/keep-awake', { mode }),
    setLidClosed: (on) => send('power/keep-awake', { lidClosed: on }),
    setUp: async () => {
      set({ settingUp: true })
      try {
        await send('power/helper', { action: 'setup' })
      } finally {
        set({ settingUp: false })
      }
    },
    setOnBattery: (on) => send('power/keep-awake', { onBattery: on }),
  }
})
