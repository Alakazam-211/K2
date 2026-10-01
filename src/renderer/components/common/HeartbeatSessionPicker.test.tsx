// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { HeartbeatSessionPicker } from './HeartbeatSessionPicker'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'

const cli = vi.hoisted(() => ({
  get: vi.fn(async (route: string, _params?: unknown) => {
    if (route === 'chat/list') {
      return [{
        sessionId: 's1',
        title: 'Design review',
        timestamp: 1,
        messageCount: 4,
        provider: 'claude',
      }]
    }
    if (route === 'chat/pinned') return []
    if (route === 'chat/custom-names') return {}
    return []
  }),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((route: string, params?: unknown) =>
      params === undefined ? cli.get(route) : cli.get(route, params)),
    daemonCliGetText: primaryOnly(vi.fn(async () => '')),
    daemonCliPost: primaryOnly(vi.fn(async () => ({}))),
    RecoveringError: class RecoveringError extends Error {},
  }
})

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
  cli.get.mockClear()
})

describe('HeartbeatSessionPicker menu', () => {
  it('portals the wakeup list above the clipping parent and still fetches sessions', async () => {
    const onSelect = vi.fn()
    const clip = renderInClip(
      <HeartbeatSessionPicker
        projectPath="/ws/proj"
        projectId="p1"
        value={{ mode: 'auto' }}
        onSelect={onSelect}
      />,
    )
    const trigger = screen.getByRole('button', { name: 'Heartbeat wakeup destination' })
    fireEvent.click(trigger)
    const menu = screen.getByTestId('heartbeat-wakeup-menu')
    expectLifted(menu, trigger, clip)
    expect(screen.getByRole('option', { name: /Pinned chat/ })).toBeTruthy()
    expect(screen.getByRole('option', { name: /Own session/ })).toBeTruthy()

    fireEvent.mouseDown(menu)
    expect(screen.getByTestId('heartbeat-wakeup-menu')).toBeTruthy()
    expect(await screen.findByRole('option', { name: /Design review/ })).toBeTruthy()
    expect(cli.get).toHaveBeenCalledWith('chat/list', { project_path: '/ws/proj' })
    expect(cli.get).toHaveBeenCalledWith('chat/pinned')

    fireEvent.mouseDown(document.body)
    expect(screen.queryByTestId('heartbeat-wakeup-menu')).toBeNull()

    fireEvent.click(trigger)
    fireEvent.click(screen.getByRole('option', { name: /Pinned chat/ }))
    expect(onSelect).toHaveBeenCalledWith({ mode: 'pinned' })
    expect(screen.queryByTestId('heartbeat-wakeup-menu')).toBeNull()
  })
})
