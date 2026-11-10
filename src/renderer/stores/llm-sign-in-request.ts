// A one-shot "open the LLM sign-in sheet" request from outside Settings
// (the setup checklist). The sheet itself stays local state in
// `AgentAccountsColumn`; this store only carries the ask across: the
// opener sets `pending` and opens Settings on LLM tokens, and the column,
// once its list has loaded, takes the request and opens the sheet for that
// tool (or just shows the page when that tool can't sign in there).

import { create } from 'zustand'
import { useSettingsStore } from '@/stores/settings'

export interface LlmSignInRequest {
  tool: string
  toolLabel: string
}

/** The tools the checklist's sign-in row means, first is the default. */
export const SIGN_IN_TOOLS: readonly LlmSignInRequest[] = [
  { tool: 'claude', toolLabel: 'Claude' },
  { tool: 'codex', toolLabel: 'Codex' },
  { tool: 'grok', toolLabel: 'Grok' },
]

interface LlmSignInRequestState {
  pending: LlmSignInRequest | null
  request: (req?: LlmSignInRequest) => void
  /** Take the request (the column calls it once it acts). */
  take: () => LlmSignInRequest | null
}

export const useLlmSignInRequestStore = create<LlmSignInRequestState>((set, get) => ({
  pending: null,
  request: (req = SIGN_IN_TOOLS[0]) => set({ pending: req }),
  take: () => {
    const p = get().pending
    if (p) set({ pending: null })
    return p
  },
}))

/** Open Settings on LLM tokens with the sign-in sheet open (Claude unless
 *  the caller names another tool). */
export function requestLlmSignIn(req?: LlmSignInRequest): void {
  useLlmSignInRequestStore.getState().request(req)
  useSettingsStore.getState().openSettings('llm-tokens')
}
