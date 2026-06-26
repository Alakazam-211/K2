// Agent Ops (Observability) pane — overlay open/close state.
//
// Phase D of `.k2/prds/prd-observability-agent-ops.md` ("prove the pane").
// A single top-level full-screen view that lists every live agent on the
// active daemon. This store only owns whether the overlay is shown; all the
// fleet data lives inside the <AgentOps /> component (one GET on mount +
// live deltas over the /cli/ops/stream WS — never a heavy refetch loop).

import { create } from 'zustand'

interface AgentOpsState {
  isOpen: boolean
  open: () => void
  close: () => void
  toggle: () => void
}

export const useAgentOpsStore = create<AgentOpsState>((set) => ({
  isOpen: false,
  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
}))
