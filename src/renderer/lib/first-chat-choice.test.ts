import { describe, it, expect } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { firstChatChoices, harnessStatusText, isHarnessNotInstalled } from './first-chat-choice'

const refusal = (code: string, extra: Record<string, unknown> = {}): DaemonHttpError => {
  const error = { code, hint: 'h', ...extra }
  return new DaemonHttpError('h', 409, code, error, { error })
}

describe('firstChatChoices', () => {
  it('reads the harnesses from the choose-first refusal, dropping entries with no harness', () => {
    const err = refusal('harness_choice_needed', {
      harnesses: [
        { harness: 'claude', label: 'Claude', installed: true, install: true, signedIn: false, signIn: true },
        { label: 'no id' },
        { harness: 'pi', installed: 'yes' },
      ],
    })
    expect(firstChatChoices(err)).toEqual([
      { harness: 'claude', label: 'Claude', installed: true, install: true, signedIn: false, signIn: true },
      { harness: 'pi', label: 'pi', installed: false, install: false, signedIn: false, signIn: false },
    ])
  })

  it('is null for any other refusal or error', () => {
    expect(firstChatChoices(refusal('harness_not_installed'))).toBeNull()
    expect(firstChatChoices(new Error('spawn failed'))).toBeNull()
    expect(firstChatChoices(null)).toBeNull()
    expect(isHarnessNotInstalled(refusal('harness_not_installed'))).toBe(true)
    expect(isHarnessNotInstalled(refusal('harness_choice_needed'))).toBe(false)
  })
})

describe('harnessStatusText', () => {
  const base = { harness: 'claude', label: 'Claude', installed: true, install: true, signedIn: false, signIn: true }
  it('says what to do next', () => {
    expect(harnessStatusText({ ...base, installed: false })).toBe('Not installed. Install it to start here.')
    expect(harnessStatusText({ ...base, installed: false, install: false })).toBe('Not installed on this server.')
    expect(harnessStatusText({ ...base, signedIn: true })).toBe('Signed in.')
    expect(harnessStatusText(base)).toContain('Sign in first')
    expect(harnessStatusText({ ...base, signIn: false })).toBe('Installed.')
  })
})
