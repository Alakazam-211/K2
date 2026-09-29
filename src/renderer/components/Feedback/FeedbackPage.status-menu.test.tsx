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
