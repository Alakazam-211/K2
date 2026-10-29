// @vitest-environment jsdom
// The password-reset permission card (0.46.6, `apps.user-password-reset`;
// prd-password-reset-card-v1 §5, §10): Allow by mode, "Email them a link
// instead" only when offered, and the one-time reveal of what the daemon
// handed THIS person in their answer (`forYou`): shown once, Copy, gone on
// the countdown, 10 s after a copy, on Close or on unmount, never stored.
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ChoiceCard } from './ChoiceCard'
import { threadItemsFromSnapshot, type OverlayChoice } from './overlayThread'
import { forYouSecret, permissionAnswerBody, permissionErrorText, type PermissionCardActions } from './permissionCardView'
import { permissionDoc } from './permissionCard.fixtures'
import { AFTER_COPY_SECS, clock } from './PermissionReveal'

const SECRET = 'Ab3de-fGh4k-Mn5pq-Rs6tu'
const LINK = 'https://box.k2.dev/cli/skin/password/set#k2inv_abc123'

const RESET_VIEW = {
  requestId: 'pr_reset1',
  action: 'apps.user-password-reset',
  setting: "Reset adam's password (agents.nsi)",
  whatChanges: 'K2 makes a new password for adam and shows it to you once; agent-nsi never sees it.',
  grant: 'one-time',
  scope: 'adam on agents.nsi',
  duration: 'once',
  consequences: 'adam is signed out everywhere and their sign-in lock is cleared.',
  values: [
    { key: 'workspace', value: 'agent-nsi' },
    { key: 'user', value: 'adam' },
  ],
  why: { by: 'agent-nsi', text: 'Adam is locked out' },
  notes: [],
  askedFor: null,
  approvers: { floor: 'owner', label: 'owner', names: ['rosson'] },
  settingsPage: 'people',
}

function reveal(mode: 'password' | 'link' | 'email', emailOffered: boolean): Record<string, unknown> {
  return permissionDoc({ view: { ...RESET_VIEW, reveal: { mode, emailOffered } } })
}

function choiceOf(doc: Record<string, unknown>): OverlayChoice {
  const snap = threadItemsFromSnapshot({ conversation_id: 'c', items: [{ collection: 'thread', seq: 9, id: doc.id, doc }] })
  const c = snap.items[0]?.doc.choice
  if (!c) throw new Error('no choice on the doc')
  return c
}

function card(doc: Record<string, unknown>, actions: PermissionCardActions): { unmount: () => void } {
  const c = choiceOf(doc)
  return render(
    <ChoiceCard
      options={c.options}
      allowCustom={c.allow_custom}
      status={c.status}
      answer={c.answer}
      expiresAt={c.expiresAt}
      nowSec={1761062000}
      permission={c.permission}
      permissionActions={actions}
      from="agent-nsi"
      onPick={() => {
        throw new Error('a permission card never answers through onPick')
      }}
    />,
  )
}

function answering(forYou: Record<string, unknown> | null): PermissionCardActions & { answer: ReturnType<typeof vi.fn> } {
  return { answer: vi.fn(async () => ({ ok: true, ...(forYou ? { forYou } : {}) })) }
}

const PW_FOR_YOU = { kind: 'apps.user-password-reset', mode: 'password', username: 'adam', app: 'agents.nsi', secret: SECRET, revealSecs: 60 }

let clipboard = ''
const writeText = vi.fn(async (t: string) => {
  clipboard = t
})
const readText = vi.fn(async () => clipboard)
const stored = vi.fn()

beforeEach(() => {
  clipboard = ''
  writeText.mockClear()
  readText.mockClear()
  Object.defineProperty(navigator, 'clipboard', { value: { writeText, readText }, configurable: true })
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(stored)
  stored.mockClear()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.restoreAllMocks()
})

/** Let the answer's promise and React settle. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
  })
}

describe('the password-reset card: Allow', () => {
  it('says what Allow shows, and sends reveal with the asked mode', async () => {
    const actions = answering(null)
    card(reveal('password', false), actions)
    expect(screen.getByTestId('permission-allow').textContent).toBe('Allow: show me a password')
    expect(screen.queryByTestId('permission-allow-email')).toBeNull()
    expect(screen.getByTestId('permission-reveal-note').textContent).toContain('The agent never sees it')
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow', mode: 'password', reveal: true })
  })

  it('offers "Email them a link instead" only when the daemon does, sent without reveal', async () => {
    const actions = answering(null)
    card(reveal('password', true), actions)
    fireEvent.click(screen.getByTestId('permission-allow-email'))
    await settle()
    expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow', mode: 'email' })
    expect(screen.queryByTestId('permission-reveal')).toBeNull()
  })

  it('an email card needs no screen and has no second button', () => {
    card(reveal('email', true), answering(null))
    expect(screen.getByTestId('permission-allow').textContent).toBe('Allow: email them a link')
    expect(screen.queryByTestId('permission-allow-email')).toBeNull()
    expect(screen.queryByTestId('permission-reveal-note')).toBeNull()
  })

  it('every other card keeps its plain Allow', async () => {
    const actions = answering(null)
    card(permissionDoc({}), actions)
    expect(screen.getByTestId('permission-allow').textContent).toBe('Allow')
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    expect(actions.answer).toHaveBeenCalledWith({ answer: 'Allow' })
  })

  it('the wire body carries reveal and mode as strings', () => {
    expect(permissionAnswerBody('agent-nsi', 'tc_1', { answer: 'Allow', mode: 'password', reveal: true })).toEqual({
      addr: 'agent-nsi',
      id: 'tc_1',
      answer: 'Allow',
      client: 'desktop',
      reveal: '1',
      mode: 'password',
    })
    expect(permissionAnswerBody('a', 'b', { answer: 'Allow' })).toEqual({ addr: 'a', id: 'b', answer: 'Allow', client: 'desktop' })
  })

  it('a phone-only refusal reads in words', () => {
    const e = new Error('{"ok":false,"error":{"code":"reveal_needs_screen","hint":"Open this card in K2 on your computer"}}')
    expect(permissionErrorText(e)).toBe('Open this card in K2 on your computer')
  })
})

describe('the password-reset card: the one-time reveal', () => {
  it('shows the password once, Copy copies it, and nothing is stored', async () => {
    card(reveal('password', false), answering(PW_FOR_YOU))
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    expect(screen.getByTestId('permission-reveal-value').textContent).toBe(SECRET)
    expect(screen.getAllByText(SECRET)).toHaveLength(1)
    fireEvent.click(screen.getByTestId('permission-reveal-copy'))
    await settle()
    expect(writeText).toHaveBeenCalledWith(SECRET)
    expect(screen.getByTestId('permission-reveal-copy').textContent).toBe('Copied')
    expect(stored).not.toHaveBeenCalled()
  })

  it('the countdown wipes it', async () => {
    vi.useFakeTimers()
    card(reveal('password', false), answering(PW_FOR_YOU))
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    expect(screen.getByTestId('permission-reveal-clock').textContent).toContain('Gone in 1:00')
    act(() => {
      vi.advanceTimersByTime(30_000)
    })
    expect(screen.getByTestId('permission-reveal-clock').textContent).toContain('Gone in 0:30')
    act(() => {
      vi.advanceTimersByTime(31_000)
    })
    expect(screen.queryByTestId('permission-reveal')).toBeNull()
    expect(document.body.textContent).not.toContain(SECRET)
  })

  it('a copy shortens it to 10 s and takes it off the clipboard at the end', async () => {
    vi.useFakeTimers()
    card(reveal('password', false), answering(PW_FOR_YOU))
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    fireEvent.click(screen.getByTestId('permission-reveal-copy'))
    await settle()
    expect(clipboard).toBe(SECRET)
    act(() => {
      vi.advanceTimersByTime(AFTER_COPY_SECS * 1000 + 300)
    })
    await settle()
    expect(screen.queryByTestId('permission-reveal')).toBeNull()
    expect(clipboard).toBe('')
  })

  it('Close wipes it, and leaves a clipboard that moved on alone', async () => {
    card(reveal('password', false), answering(PW_FOR_YOU))
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    fireEvent.click(screen.getByTestId('permission-reveal-copy'))
    await settle()
    clipboard = 'something else the person copied'
    fireEvent.click(screen.getByTestId('permission-reveal-close'))
    await settle()
    expect(screen.queryByTestId('permission-reveal')).toBeNull()
    expect(clipboard).toBe('something else the person copied')
  })

  it('unmounting forgets it and clears a copy still on the clipboard', async () => {
    const r = card(reveal('password', false), answering(PW_FOR_YOU))
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    fireEvent.click(screen.getByTestId('permission-reveal-copy'))
    await settle()
    r.unmount()
    await settle()
    expect(clipboard).toBe('')
    expect(document.body.textContent).not.toContain(SECRET)
  })

  it('a link shows as a link, and a code when the box has no front door', async () => {
    card(reveal('link', false), answering({ kind: 'apps.user-password-reset', mode: 'link', username: 'adam', app: 'agents.nsi', link: LINK }))
    expect(screen.getByTestId('permission-allow').textContent).toBe('Allow: show me a link')
    fireEvent.click(screen.getByTestId('permission-allow'))
    await settle()
    expect(screen.getByTestId('permission-reveal').getAttribute('data-kind')).toBe('link')
    expect(screen.getByTestId('permission-reveal-value').textContent).toBe(LINK)
  })
})

describe('forYouSecret', () => {
  it('reads only a password reset’s own shapes', () => {
    expect(forYouSecret({ forYou: PW_FOR_YOU })).toMatchObject({ kind: 'password', value: SECRET, username: 'adam', secs: 60 })
    expect(forYouSecret({ forYou: { kind: 'apps.user-password-reset', mode: 'link', code: 'k2inv_x' } })).toMatchObject({ kind: 'code', value: 'k2inv_x' })
    expect(forYouSecret({ forYou: { kind: 'apps.user-password-reset', mode: 'link', link: 'javascript:alert(1)' } })).toBeNull()
    expect(forYouSecret({ forYou: { kind: 'sew.owner-handoff', openUrl: 'https://x' } })).toBeNull()
    expect(forYouSecret({ ok: true })).toBeNull()
    expect(forYouSecret(null)).toBeNull()
  })

  it('clock reads like a countdown', () => {
    expect(clock(60)).toBe('1:00')
    expect(clock(58.2)).toBe('0:59')
    expect(clock(-3)).toBe('0:00')
  })
})
