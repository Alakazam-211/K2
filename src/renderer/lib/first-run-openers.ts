// Setup checklist: what each row's `opens` value opens. Every opener opens
// the thing itself (Apple's installer, the sign-in sheet, the folder pick,
// the pairing block, the account prompt, the Kessel sheet, Kessel's Thread), never
// a page to go looking on. A value this client doesn't know does nothing.

import { useSettingsStore } from '@/stores/settings'
import { requestLlmSignIn } from '@/stores/llm-sign-in-request'
import { usePhonePairingFocusStore } from '@/stores/phone-pairing-focus'
import { useOwnerAccountPromptReopenStore } from '@/stores/owner-account-prompt-reopen'
import { promptEligible } from '@/components/OwnerAccountPrompt/OwnerAccountPrompt'
import { startAddWorkspace } from '@/lib/start-add-workspace'
import { useFirstRunStore, type FirstRunOpens } from '@/stores/first-run'
import { openKesselSheet } from '@/stores/kessel-sheet'
import { openWorkspaceThread } from '@/lib/open-workspace-thread'
import { useToastStore } from '@/stores/toast'
import { installGitTools } from '@/lib/git-tools'

/** `kessel_thread_cos` failed to start the hire; the daemon's reason follows. */
export const COS_START_FAILED_TEXT = "Couldn't start hiring your Chief of Staff"

/** Post "Let's hire my Chief of Staff." into Kessel's Thread as this person,
 *  then open Kessel's Chat tab in the Thread view. A refusal is a toast. */
export async function startChiefOfStaffHire(): Promise<void> {
  try {
    const workspaceId = await useFirstRunStore.getState().startChiefOfStaff()
    await openWorkspaceThread(workspaceId)
  } catch (e) {
    const why = (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
    useToastStore.getState().addToast(why ? `${COS_START_FAILED_TEXT}: ${why}` : `${COS_START_FAILED_TEXT}.`, 'error', 6000)
  }
}

/** `dev_tools` couldn't open Apple's installer; the daemon's reason follows. */
export const DEV_TOOLS_INSTALL_FAILED_TEXT = "Couldn't open Apple's installer"

/** Open Apple's installer on the server's Mac (`POST /cli/git-tools/install`,
 *  `xcode-select --install`), then read the list again: the row says the
 *  installer is open (`note: installing`), or reads done when git already
 *  runs. A refusal is a toast. */
export async function startDevToolsInstall(): Promise<void> {
  try {
    await installGitTools()
  } catch (e) {
    const why = (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '')
    useToastStore.getState().addToast(why ? `${DEV_TOOLS_INSTALL_FAILED_TEXT}: ${why}` : `${DEV_TOOLS_INSTALL_FAILED_TEXT}.`, 'error', 6000)
  } finally {
    void useFirstRunStore.getState().load()
  }
}

const OPENERS: Record<FirstRunOpens, () => void> = {
  dev_tools: () => void startDevToolsInstall(),
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
  kessel_sheet: () => openKesselSheet(),
  kessel_thread_cos: () => void startChiefOfStaffHire(),
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
