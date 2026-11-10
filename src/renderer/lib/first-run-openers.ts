// Setup checklist: what each row's `opens` value opens. Every opener opens
// the thing itself (the sign-in sheet, the folder pick, the pairing block,
// the account prompt), never a page to go looking on. A value this client
// doesn't know does nothing.

import { useSettingsStore } from '@/stores/settings'
import { requestLlmSignIn } from '@/stores/llm-sign-in-request'
import { usePhonePairingFocusStore } from '@/stores/phone-pairing-focus'
import { useOwnerAccountPromptReopenStore } from '@/stores/owner-account-prompt-reopen'
import { promptEligible } from '@/components/OwnerAccountPrompt/OwnerAccountPrompt'
import { startAddWorkspace } from '@/lib/start-add-workspace'
import type { FirstRunOpens } from '@/stores/first-run'

const OPENERS: Record<FirstRunOpens, () => void> = {
  llm_sign_in: () => requestLlmSignIn(),
  add_workspace: () => void startAddWorkspace(),
  tunnel: () => useSettingsStore.getState().openSettings('k2-connect'),
  phone: () => {
    usePhonePairingFocusStore.getState().requestPhonePairingFocus()
    useSettingsStore.getState().openSettings('companion')
  },
  owner_account_prompt: () => {
    // The prompt shows only in this computer's main window; anywhere else
    // the same choice is General → Your account.
    if (promptEligible()) useOwnerAccountPromptReopenStore.getState().reopen()
    else useSettingsStore.getState().openSettings('general')
  },
}

function hasOpener(opens: string): opens is FirstRunOpens {
  return Object.prototype.hasOwnProperty.call(OPENERS, opens)
}

/** Run the opener for `opens`. False (and nothing happens) for a value
 *  this client doesn't know. */
export function openFirstRunTarget(opens: string | null | undefined): boolean {
  if (!opens || !hasOpener(opens)) return false
  OPENERS[opens]()
  return true
}
