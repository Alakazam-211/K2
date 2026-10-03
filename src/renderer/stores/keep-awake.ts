import { create } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { parseKeepAwakeBody, type KeepAwakeMode, type KeepAwakeStatus } from '@/lib/keep-awake'
import { primaryScope } from '@/kessel/server-scope'

// Heartbeat S6 — Keep awake is a setting of the window's server (the
// machine that should stay awake), like Settings. The daemon owns the
// mode and the truth; this store only reads and sends gestures.

interface KeepAwakeStore {
  /** `null` until the server answers (an older server never does). */
  status: KeepAwakeStatus | null
  error: string | null
  /** A POST is in flight. On a Mac without the helper it can wait up to
   *  2 minutes for the admin dialog. */
  busy: boolean
  load: () => Promise<void>
  setMode: (mode: KeepAwakeMode) => Promise<void>
  /** Show the one admin dialog again after a decline. */
  approveLid: () => Promise<void>
  setOnBattery: (on: boolean) => Promise<void>
}

export function resetKeepAwakeForTests(): void {
  useKeepAwakeStore.setState({ status: null, error: null, busy: false })
}

export const useKeepAwakeStore = create<KeepAwakeStore>((set, get) => {
  async function post(body: Record<string, unknown>): Promise<void> {
    set({ busy: true })
    try {
      const status = parseKeepAwakeBody(
        await daemonCliPost<unknown>(primaryScope(), 'power/keep-awake', body),
      )
      if (!status) throw new Error('power/keep-awake: response has no keepAwake')
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

    setMode: (mode) => post({ mode }),
    approveLid: () => post({ approveLid: true }),
    setOnBattery: (on) => post({ onBattery: on }),
  }
})
