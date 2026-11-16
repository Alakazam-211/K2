// The server's default model per harness (0.46.13, settings key
// `defaultModels`): every new chat K2 starts on this server begins on it
// (the daemon passes it as the CLI's model flag). Its own small store so
// Settings → Providers / Models and the Thread's `/model` picker share one
// read, and the big settings store stays as it is.

import { create } from 'zustand'
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'

interface DefaultModelsState {
  models: Record<string, string>
  loaded: boolean
  /** The save that failed last, in words (cleared by the next save). */
  error: string | null
  load: () => Promise<void>
  /** Set (or with '' clear) one harness's default. Resolves either way;
   *  a refusal puts the old value back and sets `error`. */
  setDefault: (harness: string, model: string) => Promise<void>
}

export const useDefaultModelsStore = create<DefaultModelsState>((set, get) => ({
  models: {},
  loaded: false,
  error: null,
  load: async () => {
    try {
      const s = await settingsGet()
      set({ models: { ...(s.defaultModels ?? {}) }, loaded: true })
    } catch {
      set({ loaded: true })
    }
  },
  setDefault: async (harness, model) => {
    const prev = get().models
    const next = { ...prev }
    if (model.trim()) next[harness] = model.trim()
    else delete next[harness]
    set({ models: next, error: null })
    try {
      const s = await settingsUpdate({ defaultModels: { [harness]: model.trim() } })
      set({ models: { ...(s.defaultModels ?? {}) } })
    } catch (err) {
      set({ models: prev, error: err instanceof Error ? err.message : String(err) })
    }
  },
}))
