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
