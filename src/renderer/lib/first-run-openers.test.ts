// @vitest-environment jsdom
//
// Setup checklist openers: each `opens` value opens its own target, and a
// value this client doesn't know does nothing.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  openSettings: vi.fn(),
  startAddWorkspace: vi.fn(async () => {}),
  eligible: true,
}))

vi.mock('@/stores/settings', () => ({
  useSettingsStore: { getState: () => ({ openSettings: h.openSettings }) },
}))
vi.mock('@/lib/start-add-workspace', () => ({ startAddWorkspace: h.startAddWorkspace }))
vi.mock('@/components/OwnerAccountPrompt/OwnerAccountPrompt', () => ({ promptEligible: () => h.eligible }))

import { openFirstRunTarget } from './first-run-openers'
import { useLlmSignInRequestStore } from '@/stores/llm-sign-in-request'
import { usePhonePairingFocusStore } from '@/stores/phone-pairing-focus'
import { useOwnerAccountPromptReopenStore } from '@/stores/owner-account-prompt-reopen'
import { useCliCommandFocusStore } from '@/stores/cli-command-focus'

beforeEach(() => {
  h.openSettings.mockReset()
  h.startAddWorkspace.mockClear()
  h.eligible = true
  useLlmSignInRequestStore.setState({ pending: null })
  usePhonePairingFocusStore.setState({ requestSeq: 0, handledSeq: 0 })
  useOwnerAccountPromptReopenStore.setState({ forced: false })
  useCliCommandFocusStore.setState({ requestSeq: 0, handledSeq: 0 })
})

describe('openFirstRunTarget', () => {
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

  it('cli_settings opens General and asks for the k2 command row, once per click', () => {
    expect(openFirstRunTarget('cli_settings')).toBe(true)
    expect(h.openSettings).toHaveBeenCalledWith('general')
    expect(useCliCommandFocusStore.getState().takeRequest()).toBe(true)
    expect(useCliCommandFocusStore.getState().takeRequest()).toBe(false)
    openFirstRunTarget('cli_settings')
    expect(useCliCommandFocusStore.getState().takeRequest()).toBe(true)
  })

  it.each(['k5_chat', '', 'constructor', '__proto__', null, undefined])('ignores %s', (v) => {
    expect(openFirstRunTarget(v as string | null | undefined)).toBe(false)
    expect(h.openSettings).not.toHaveBeenCalled()
    expect(h.startAddWorkspace).not.toHaveBeenCalled()
  })
})
