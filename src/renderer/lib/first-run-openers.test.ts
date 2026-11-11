// @vitest-environment jsdom
//
// Setup checklist openers: each `opens` value opens its own target, and a
// value this client doesn't know does nothing.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  openSettings: vi.fn(),
  startAddWorkspace: vi.fn(async () => {}),
  eligible: true,
  calls: [] as string[],
  openWorkspaceThread: vi.fn(async (_id: string) => true),
  startChiefOfStaff: vi.fn(async () => 'ws-kessel'),
  addToast: vi.fn(),
  daemonCliPost: vi.fn(),
  load: vi.fn(async () => {}),
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: { getState: () => ({ openSettings: h.openSettings }) },
}))
vi.mock('@/lib/start-add-workspace', () => ({ startAddWorkspace: h.startAddWorkspace }))
vi.mock('@/components/OwnerAccountPrompt/OwnerAccountPrompt', () => ({ promptEligible: () => h.eligible }))
vi.mock('@/lib/open-workspace-thread', () => ({ openWorkspaceThread: (id: string) => h.openWorkspaceThread(id) }))
vi.mock('@/stores/toast', () => ({ useToastStore: { getState: () => ({ addToast: h.addToast }) } }))
vi.mock('@/stores/first-run', () => ({
  useFirstRunStore: { getState: () => ({ startChiefOfStaff: h.startChiefOfStaff, load: h.load }) },
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: (...a: unknown[]) => h.daemonCliPost(...a),
}))

import {
  COS_START_FAILED_TEXT,
  DEV_TOOLS_INSTALL_FAILED_TEXT,
  openFirstRunTarget,
  startChiefOfStaffHire,
  startDevToolsInstall,
} from './first-run-openers'
import { useKesselSheetStore } from '@/stores/kessel-sheet'
import { useLlmSignInRequestStore } from '@/stores/llm-sign-in-request'
import { usePhonePairingFocusStore } from '@/stores/phone-pairing-focus'
import { useOwnerAccountPromptReopenStore } from '@/stores/owner-account-prompt-reopen'

beforeEach(() => {
  h.openSettings.mockReset()
  h.startAddWorkspace.mockClear()
  h.eligible = true
  useLlmSignInRequestStore.setState({ pending: null })
  usePhonePairingFocusStore.setState({ requestSeq: 0, handledSeq: 0 })
  useOwnerAccountPromptReopenStore.setState({ forced: false })
  useKesselSheetStore.setState({ isOpen: false })
  h.calls = []
  h.openWorkspaceThread.mockReset()
  h.openWorkspaceThread.mockImplementation(async (id: string) => {
    h.calls.push(`open:${id}`)
    return true
  })
  h.startChiefOfStaff.mockReset()
  h.startChiefOfStaff.mockImplementation(async () => {
    h.calls.push('start')
    return 'ws-kessel'
  })
  h.addToast.mockReset()
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockResolvedValue({ state: 'installing', started: 'started', howTo: [], waiting: [] })
  h.load.mockClear()
})

describe('openFirstRunTarget', () => {
  it("dev_tools POSTs the git-tools install (Apple's installer), then reads the list again", async () => {
    expect(openFirstRunTarget('dev_tools')).toBe(true)
    await vi.waitFor(() => expect(h.load).toHaveBeenCalledTimes(1))
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost.mock.calls[0][1]).toBe('git-tools/install')
    expect(h.addToast).not.toHaveBeenCalled()
    expect(h.openSettings).not.toHaveBeenCalled()
  })

  it('dev_tools: a refusal is a toast, and the list is read again', async () => {
    h.daemonCliPost.mockRejectedValue(new Error('Apple’s command line tools are already installed'))
    await startDevToolsInstall()
    expect(h.addToast).toHaveBeenCalledWith(
      `${DEV_TOOLS_INSTALL_FAILED_TEXT}: Apple’s command line tools are already installed`,
      'error',
      6000,
    )
    expect(h.load).toHaveBeenCalledTimes(1)
  })

  it('llm_sign_in opens LLM tokens with a Claude sign-in asked for', () => {
    expect(openFirstRunTarget('llm_sign_in')).toBe(true)
    expect(h.openSettings).toHaveBeenCalledWith('llm-tokens')
    expect(useLlmSignInRequestStore.getState().pending).toEqual({ tool: 'claude', toolLabel: 'Claude' })
  })

  it('add_workspace starts the folder pick and the Add Workspace dialog', () => {
    expect(openFirstRunTarget('add_workspace')).toBe(true)
    expect(h.startAddWorkspace).toHaveBeenCalledTimes(1)
    expect(h.openSettings).not.toHaveBeenCalled()
  })

  it('tunnel opens Settings on the tunnel', () => {
    expect(openFirstRunTarget('tunnel')).toBe(true)
    expect(h.openSettings).toHaveBeenCalledWith('k2-connect')
  })

  it('phone opens K2 Companion and asks for pairing, once per click', () => {
    expect(openFirstRunTarget('phone')).toBe(true)
    expect(h.openSettings).toHaveBeenCalledWith('companion')
    expect(usePhonePairingFocusStore.getState().takeRequest()).toBe(true)
    expect(usePhonePairingFocusStore.getState().takeRequest()).toBe(false)
    openFirstRunTarget('phone')
    expect(usePhonePairingFocusStore.getState().takeRequest()).toBe(true)
  })

  it('owner_account_prompt reopens the prompt where it can show', () => {
    expect(openFirstRunTarget('owner_account_prompt')).toBe(true)
    expect(useOwnerAccountPromptReopenStore.getState().forced).toBe(true)
    expect(h.openSettings).not.toHaveBeenCalled()
  })

  it('owner_account_prompt opens General where the prompt cannot show', () => {
    h.eligible = false
    expect(openFirstRunTarget('owner_account_prompt')).toBe(true)
    expect(useOwnerAccountPromptReopenStore.getState().forced).toBe(false)
    expect(h.openSettings).toHaveBeenCalledWith('general')
  })

  it('kessel_sheet opens the Kessel sheet', () => {
    expect(openFirstRunTarget('kessel_sheet')).toBe(true)
    expect(useKesselSheetStore.getState().isOpen).toBe(true)
    expect(h.openSettings).not.toHaveBeenCalled()
  })

  it("kessel_thread_cos starts the hire, then opens Kessel's Thread", async () => {
    expect(openFirstRunTarget('kessel_thread_cos')).toBe(true)
    await vi.waitFor(() => expect(h.openWorkspaceThread).toHaveBeenCalledWith('ws-kessel'))
    expect(h.calls).toEqual(['start', 'open:ws-kessel'])
    expect(h.addToast).not.toHaveBeenCalled()
  })

  it('kessel_thread_cos: a refusal is a toast and opens nothing', async () => {
    h.startChiefOfStaff.mockRejectedValue(new Error('only an Owner or Admin can start this'))
    await startChiefOfStaffHire()
    expect(h.openWorkspaceThread).not.toHaveBeenCalled()
    expect(h.addToast).toHaveBeenCalledWith(`${COS_START_FAILED_TEXT}: only an Owner or Admin can start this`, 'error', 6000)
  })

  it.each(['kessel_chat', 'cli_settings', '', 'constructor', '__proto__', null, undefined])('ignores %s', (v) => {
    expect(openFirstRunTarget(v as string | null | undefined)).toBe(false)
    expect(h.openSettings).not.toHaveBeenCalled()
    expect(h.startAddWorkspace).not.toHaveBeenCalled()
  })
})
