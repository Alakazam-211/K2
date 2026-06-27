// federation-target store — holds the currently-selected CROSS-SERVER target
// (`<peer>::<workspace>::<agent>`) chosen in the Workspace Connections picker.
//
// `prd-cross-server-agent-comms.md` Phase 5. The daemon is authoritative for
// addressing + delivery (`/cli/federation/send`); this store is purely the
// renderer's "what did the user pick" state, consumed by whatever composes the
// outbound send. Tiny on purpose — no persistence, no I/O.

import { create } from 'zustand'

export interface FederationTargetState {
  /** The selected cross-server target, or null when none is chosen. */
  target: string | null
  /** Human label for the selection (e.g. "rosson@laptop · scout"). */
  label: string | null
  /** Pick a cross-server target. */
  setTarget: (target: string, label: string) => void
  /** Clear the selection (e.g. on host switch or after send). */
  clearTarget: () => void
}

export const useFederationTargetStore = create<FederationTargetState>((set) => ({
  target: null,
  label: null,
  setTarget: (target, label) => set({ target, label }),
  clearTarget: () => set({ target: null, label: null }),
}))

/** Test-only reset. */
export function __resetFederationTargetForTests(): void {
  useFederationTargetStore.setState({ target: null, label: null })
}
