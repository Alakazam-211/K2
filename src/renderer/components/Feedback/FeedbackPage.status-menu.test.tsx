// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { FeedbackCard } from './FeedbackPage'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'
import type { FeedbackListRow } from './feedback-api'

const api = vi.hoisted(() => ({
  resolveFeedback: vi.fn(async () => {}),
}))

vi.mock('./feedback-api', async () => {
  const actual = await vi.importActual<typeof import('./feedback-api')>('./feedback-api')
  return { ...actual, resolveFeedback: api.resolveFeedback }
})

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliGetText: vi.fn(async () => ''),
  daemonCliPost: vi.fn(async () => ({})),
  RecoveringError: class RecoveringError extends Error {},
}))

const row: FeedbackListRow = {
  id: 'fb-1',
  projectId: 'p1',
  sessionId: null,
  sessionKind: null,
  agentName: 'claude',
  kind: 'question',
  title: 'Need a decision',
  body: null,
  options: null,
  priority: 1,
  status: 'waiting',
  answer: null,
  createdAt: 1_700_000_000,
  updatedAt: 1_700_000_000,
  answeredAt: null,
  commentCount: 1,
  assignees: [],
  projectPath: '/ws',
  projectName: 'Alpha',
  linked: true,
}

/** A ticket whose workspace was removed (`list-all` row, linked: false). */
const unlinkedRow: FeedbackListRow = {
  ...row,
  id: '3f2a9c1e-7b4d-4e0a-9d6f-0123456789ab',
  projectId: 'gone-project-id',
  agentName: 'scout',
  title: 'Approve the vendor contract',
  createdAt: 1_759_241_100, // 2025-09-30 14:05 UTC
  projectPath: null,
  projectName: null,
  linked: false,
}

function renderInClip(ui: React.ReactElement): HTMLElement {
  const clip = document.createElement('div')
  clip.style.position = 'static'
  clip.style.zIndex = '99999'
  clip.style.overflow = 'hidden'
  document.body.appendChild(clip)
  render(ui, { container: clip })
  return clip
}

function expectLifted(menu: HTMLElement, trigger: HTMLElement, clip: HTMLElement): void {
  expect(clip.contains(trigger)).toBe(true)
  expect(clip.contains(menu)).toBe(false)
  expect(menu.parentElement).toBe(document.body)
  const layer = menuLayerForTrigger(trigger)
  expect(layer).toBeGreaterThanOrEqual(400)
  expect(layer).toBeGreaterThan(99999)
  expect(menu.style.position).toBe('fixed')
  expect(menu.style.zIndex).toBe(String(layer))
  const styleText = `${menu.getAttribute('style') ?? ''} ${menu.style.cssText}`
  expect(styleText).not.toContain('z-index: 9999')
  const opaque =
    menu.classList.contains('bg-[var(--color-bg)]') ||
    menu.style.backgroundColor === 'var(--color-bg)' ||
    /background-color:\s*var\(--color-bg\)(?:\s|;|$)/.test(styleText)
  expect(opaque).toBe(true)
  const paint = `${menu.className} ${styleText}`
  expect(paint).not.toContain('--color-bg-surface')
  expect(paint).not.toContain('--color-bg-elevated')
  expect(paint).not.toContain('--color-bg-stripe')
}

afterEach(() => {
  cleanup()
  document.body.replaceChildren()
  api.resolveFeedback.mockClear()
})

describe('ticket status menu', () => {
  it('portals the status list above the clipping parent and still writes a status', async () => {
    const onMutated = vi.fn()
    const clip = renderInClip(
      <FeedbackCard
        row={row}
        workspace={undefined}
        nowSec={1_700_000_100}
        selected={false}
        onSelect={vi.fn()}
        onMutated={onMutated}
      />,
    )
    const trigger = screen.getByTitle('Change status')
    fireEvent.click(trigger)
    const menu = screen.getByTestId('ticket-status-menu')
    expectLifted(menu, trigger, clip)
    const labels = Array.from(menu.querySelectorAll('button')).map((button) => button.textContent?.trim())
    expect(labels).toEqual(['Waiting', 'Needs discussion', 'Planned', 'Resolved', 'Dismissed'])

    fireEvent.mouseDown(menu)
    expect(screen.getByTestId('ticket-status-menu')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Planned' }))
    await waitFor(() => {
      expect(api.resolveFeedback).toHaveBeenCalledWith('fb-1', 'planned')
      expect(onMutated).toHaveBeenCalled()
    })
    expect(screen.queryByTestId('ticket-status-menu')).toBeNull()
  })
})

// Rosson 2026-10-24: on a ticket at the very bottom of the list, the status
// menu opened downward past the window, so no status could be picked. It is
// the app's anchored menu now: it flips upward when there isn't room below.
describe('ticket status menu placement in the list', () => {
  /** Give `el` a fixed rect (jsdom has no layout). */
  function setRect(el: Element, r: { top: number; left: number; width: number; height: number }): void {
    const rect = { ...r, right: r.left + r.width, bottom: r.top + r.height, x: r.left, y: r.top }
    ;(el as HTMLElement).getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect
  }

  function renderList(): HTMLElement[] {
    const rows: FeedbackListRow[] = [
      { ...row, id: 'fb-top', title: 'First' },
      { ...row, id: 'fb-mid', title: 'Middle' },
      { ...row, id: 'fb-last', title: 'Last' },
    ]
    render(
      <div style={{ overflowY: 'auto', height: window.innerHeight }}>
        {rows.map((r) => (
          <FeedbackCard
            key={r.id}
            row={r}
            workspace={undefined}
            nowSec={1_700_000_100}
            selected={false}
            onSelect={vi.fn()}
            onMutated={vi.fn()}
          />
        ))}
      </div>,
    )
    const triggers = screen.getAllByTestId('card-status')
    expect(triggers).toHaveLength(3)
    return triggers
  }

  it('opens upward from the last row at the bottom of the window, and stays in view', () => {
    const triggers = renderList()
    const last = triggers[2]
    const vh = window.innerHeight
    // The chip's box sits 12px above the window's bottom edge.
    setRect(last.parentElement as HTMLElement, { top: vh - 28, left: 40, width: 70, height: 16 })
    fireEvent.click(last)
    const menu = screen.getByTestId('ticket-status-menu')
    expect(menu.getAttribute('data-placement')).toBe('up')
    expect(menu.style.position).toBe('fixed')
    expect(menu.style.top).toBe('')
    // Its bottom edge sits just above the chip (2px gap).
    expect(menu.style.bottom).toBe(`${vh - (vh - 28) + 2}px`)
    // Still a working menu.
    expect(Array.from(menu.querySelectorAll('button')).map((b) => b.textContent?.trim())).toEqual([
      'Waiting',
      'Needs discussion',
      'Planned',
      'Resolved',
      'Dismissed',
    ])
  })

  it('opens downward from a row with room below', () => {
    const triggers = renderList()
    setRect(triggers[0].parentElement as HTMLElement, { top: 100, left: 40, width: 70, height: 16 })
    fireEvent.click(triggers[0])
    const menu = screen.getByTestId('ticket-status-menu')
    expect(menu.getAttribute('data-placement')).toBe('down')
    expect(menu.style.top).toBe('118px')
    expect(menu.style.bottom).toBe('')
  })

  it('Esc closes the menu without reaching the board', () => {
    const triggers = renderList()
    const board = vi.fn()
    window.addEventListener('keydown', board)
    try {
      fireEvent.click(triggers[2])
      expect(screen.getByTestId('ticket-status-menu')).toBeTruthy()
      fireEvent.keyDown(document.body, { key: 'Escape' })
      expect(screen.queryByTestId('ticket-status-menu')).toBeNull()
      expect(board).not.toHaveBeenCalled()
    } finally {
      window.removeEventListener('keydown', board)
    }
  })
})

describe('unlinked ticket card (prd-tickets-badge-orphans TB18 + Appa A1)', () => {
  it('shows identifying details and offers only Resolve and Dismiss', async () => {
    const onMutated = vi.fn()
    render(
      <FeedbackCard
        row={unlinkedRow}
        workspace={undefined}
        nowSec={1_759_241_200}
        selected={false}
        onSelect={vi.fn()}
        onMutated={onMutated}
      />,
    )
    // No workspace name to show: the card carries the agent, the exact
    // filing date, and a short id for `k2 tickets show` (the list puts it
    // under the "Unlinked workspace" section).
    expect(screen.getByText('Approve the vendor contract')).toBeTruthy()
    const details = screen.getByTestId('unlinked-details')
    expect(details.textContent).toBe('Filed by scout·2025-09-30 14:05 UTC·3f2a9c1e')

    fireEvent.click(screen.getByTitle('Change status'))
    const menu = screen.getByTestId('ticket-status-menu')
    const labels = Array.from(menu.querySelectorAll('button')).map((b) => b.textContent?.trim())
    expect(labels).toEqual(['Resolved', 'Dismissed'])

    fireEvent.click(screen.getByRole('button', { name: 'Dismissed' }))
    await waitFor(() => {
      expect(api.resolveFeedback).toHaveBeenCalledWith(unlinkedRow.id, 'dismissed')
      expect(onMutated).toHaveBeenCalled()
    })
  })

  it('a linked card shows no unlinked details', () => {
    render(
      <FeedbackCard
        row={row}
        workspace={undefined}
        nowSec={1_700_000_100}
        selected={false}
        onSelect={vi.fn()}
        onMutated={vi.fn()}
      />,
    )
    expect(screen.getByText('Need a decision')).toBeTruthy()
    // The compact card names neither the workspace nor the agent.
    expect(screen.queryByText('Alpha')).toBeNull()
    expect(screen.queryByText(/claude/)).toBeNull()
    expect(screen.queryByTestId('unlinked-details')).toBeNull()
  })
})
