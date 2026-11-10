// @vitest-environment jsdom
// K2's sheets over a Garden widget's card all cancel the same way (ZenSheet):
// the × (Close (Esc)), Esc with focus anywhere, and a press outside the sheet
// each give the sheet's no-op answer, once, and never its yes. Fixtures are
// made up (example.test, guestdesk).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'

const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', () => ({ daemonCliPost, daemonCliGet: vi.fn() }))
vi.mock('@/lib/zen/zen-api', () => ({ zenLocalScope: () => ({ id: 'primary', isPrimary: true }) }))

import { ZenDownloadSheet } from './ZenDownloadSheet'
import { ZenGuestRoomsSheet } from './ZenGuestRoomsSheet'
import { ZenPermissionSheet } from './ZenPermissionSheet'
import { ZenSecretSheet } from './ZenSecretSheet'
import { ZenWorkSheet } from './ZenWorkSheet'

afterEach(() => {
  cleanup()
  daemonCliPost.mockReset()
})

/** The three ways to cancel, each on a fresh sheet. */
const WAYS: Array<[string, (sheetTestId: string) => void]> = [
  ['the ×', () => fireEvent.click(screen.getByRole('button', { name: 'Close (Esc)' }))],
  [
    'Esc with focus outside the sheet',
    () => {
      ;(document.activeElement as HTMLElement | null)?.blur()
      fireEvent.keyDown(document.body, { key: 'Escape' })
    },
  ],
  ['a press on the dimmed card', (id) => fireEvent.pointerDown(screen.getByTestId(id))],
  ['a press elsewhere on the page', () => fireEvent.pointerDown(screen.getByTestId('page'))],
]

function page(sheet: React.JSX.Element): void {
  render(
    <div>
      <button type="button" data-testid="page">
        Another widget
      </button>
      {sheet}
    </div>,
  )
}

const download = { widget: 'Board', garden: 'Fleet Ops', name: 'report.csv', size: 2048 }
const guests = {
  secretId: 'guests:abc',
  change: {
    app: 'svc-1',
    appName: 'jobs',
    appHandle: 'initech/jobs',
    username: 'dana',
    role: null,
    before: ['p-own'],
    beforeHandles: ['initech'],
    after: ['p-acme'],
    afterHandles: ['acme'],
    widget: 'guestdesk',
    madeBy: 'agent:board-builder',
  },
}

describe.each(WAYS)('Garden sheets cancel with %s', (_way, cancel) => {
  it('k2.download: Don’t save', () => {
    const onDone = vi.fn()
    page(<ZenDownloadSheet ask={download} onDone={onDone} />)
    cancel('zen-download-sheet')
    expect(onDone.mock.calls).toEqual([['deny']])
  })

  it('Workbench: Cancel (the change does not run)', () => {
    const onDone = vi.fn()
    page(<ZenWorkSheet change={{ kind: 'end', name: 'notes', worktree: null }} widget="Garden chat" onDone={onDone} />)
    cancel('zen-work-sheet')
    expect(onDone.mock.calls).toEqual([[false]])
  })

  it('guests.setRooms: declined, no vault confirm', () => {
    const onDone = vi.fn()
    page(<ZenGuestRoomsSheet ask={guests} onDone={onDone} />)
    cancel('zen-guest-rooms-sheet')
    expect(onDone.mock.calls).toEqual([[null]])
    expect(daemonCliPost).not.toHaveBeenCalled()
  })

  it('a permission card: closes, answers nothing', () => {
    const onDone = vi.fn()
    page(
      <ZenPermissionSheet ask={{ address: 'sales', cardId: 'c1' }} onDone={onDone} target={() => null} onChange={() => () => undefined} />,
    )
    cancel('zen-permission-sheet')
    expect(onDone).toHaveBeenCalledTimes(1)
  })

  it('a secret card: closes, neither answers nor dismisses it', () => {
    const onDone = vi.fn()
    const answer = vi.fn(async () => undefined)
    const dismiss = vi.fn(async () => undefined)
    page(
      <ZenSecretSheet
        ask={{ address: 'sales', cardId: 'c1' }}
        answer={answer}
        dismiss={dismiss}
        onDone={onDone}
        target={() => null}
        onChange={() => () => undefined}
      />,
    )
    cancel('zen-secret-sheet')
    expect(onDone).toHaveBeenCalledTimes(1)
    expect(answer).not.toHaveBeenCalled()
    expect(dismiss).not.toHaveBeenCalled()
  })
})

describe('ZenSheet', () => {
  it('a press inside the sheet does not cancel it', () => {
    const onDone = vi.fn()
    page(<ZenDownloadSheet ask={download} onDone={onDone} />)
    fireEvent.pointerDown(screen.getByTestId('zen-download-name'))
    expect(onDone).not.toHaveBeenCalled()
  })

  it('the guest-rooms sheet answers once: a late confirm after a cancel is dropped', () => {
    const onDone = vi.fn()
    page(<ZenGuestRoomsSheet ask={guests} onDone={onDone} />)
    fireEvent.click(screen.getByRole('button', { name: 'Close (Esc)' }))
    fireEvent.click(screen.getByTestId('zen-guest-rooms-cancel'))
    expect(onDone.mock.calls).toEqual([[null]])
  })

  it('takes focus as it opens, so Esc works when focus was in the widget', () => {
    const onDone = vi.fn()
    page(<ZenPermissionSheet ask={{ address: 'sales', cardId: 'c1' }} onDone={onDone} target={() => null} onChange={() => () => undefined} />)
    expect(screen.getByTestId('zen-permission-sheet').contains(document.activeElement)).toBe(true)
  })
})
