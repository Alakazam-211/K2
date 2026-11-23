// The agent-reports card's open state (plan-agent-reports-opt-in-app-v1).
// Opened by the setup list's `agent_reports` row and by Settings → General
// → Send feedback's "Choose now"; drawn once per window by
// `AgentReportsHost` (mounted in App), which also shows it on its own when
// the daemon offers it (`offers: ["agent_reports"]`). The window's server
// only, like the setup list.

import { create } from 'zustand'

/** Where the card was opened from; each has its own buttons. */
export type AgentReportsCardKind = 'row' | 'offer' | 'settings'

interface AgentReportsCardState {
  /** Opened by hand (the row or Settings), else null. */
  openAs: Exclude<AgentReportsCardKind, 'offer'> | null
  open: (kind: Exclude<AgentReportsCardKind, 'offer'>) => void
  close: () => void
}

export const useAgentReportsCardStore = create<AgentReportsCardState>((set) => ({
  openAs: null,
  open: (kind) => set({ openAs: kind }),
  close: () => set({ openAs: null }),
}))

export function openAgentReportsCard(kind: Exclude<AgentReportsCardKind, 'offer'>): void {
  useAgentReportsCardStore.getState().open(kind)
}
