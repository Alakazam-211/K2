// Agent templates in the app (prd-agent-templates-v1 §6.1, §7): whether the
// Create an agent sheet is open (drawn once per window by `CreateAgentSheet`,
// mounted in App; opened from the sidebar's add menu), and which servers
// answered that they can't save templates yet, so Settings says so instead
// of offering the button again. The window's server only.

import { create } from 'zustand'

interface AgentTemplatesState {
  createOpen: boolean
  /** The template the Create sheet starts on; null: its first card. */
  createStart: string | null
  openCreate: (startOn?: string) => void
  closeCreate: () => void
  /** Host keys whose server has no save route (an older version). */
  saveUnsupported: Record<string, true>
  markSaveUnsupported: (hostKey: string) => void
}

export const useAgentTemplatesStore = create<AgentTemplatesState>((set) => ({
  createOpen: false,
  createStart: null,
  openCreate: (startOn) => set({ createOpen: true, createStart: startOn ?? null }),
  closeCreate: () => set({ createOpen: false, createStart: null }),
  saveUnsupported: {},
  markSaveUnsupported: (hostKey) =>
    set((s) => (s.saveUnsupported[hostKey] ? s : { saveUnsupported: { ...s.saveUnsupported, [hostKey]: true } })),
}))

/** Open Create an agent, on `startOn`'s card when given. */
export function openCreateAgentSheet(startOn?: string): void {
  useAgentTemplatesStore.getState().openCreate(startOn)
}

export function resetAgentTemplatesForTests(): void {
  useAgentTemplatesStore.setState({ createOpen: false, createStart: null, saveUnsupported: {} })
}
