// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import HeartbeatScheduleDialog from './HeartbeatScheduleDialog'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'
import { useHeartbeatScheduleStore } from '@/stores/heartbeat-schedule'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => []),
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
  useHeartbeatScheduleStore.setState({ isOpen: false, projectId: null })
})

describe('HeartbeatScheduleDialog menus', () => {
  it('portals the frequency list and the long ordinal list above the clipping panel', () => {
    useHeartbeatScheduleStore.setState({ isOpen: true, projectId: 'p1' })
    const clip = renderInClip(<HeartbeatScheduleDialog />)
    const panel = clip.querySelector('.overflow-hidden')
    expect(panel).toBeTruthy()

    const frequency = screen.getByRole('button', { name: 'Daily' })
    expect(panel!.contains(frequency)).toBe(true)
    fireEvent.click(frequency)
    const menu = screen.getByTestId('setting-dropdown-menu')
    expectLifted(menu, frequency, clip)
    expect(panel!.contains(menu)).toBe(false)
    expect(screen.getByRole('button', { name: 'Yearly' })).toBeTruthy()

    fireEvent.mouseDown(menu)
    expect(screen.getByTestId('setting-dropdown-menu')).toBeTruthy()
    fireEvent.mouseDown(document.body)
    expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()

    fireEvent.click(screen.getByRole('button', { name: 'Daily' }))
    fireEvent.click(screen.getByRole('button', { name: 'Monthly' }))
    expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()
    expect(screen.getByRole('button', { name: 'Monthly' })).toBeTruthy()

    const ordinalDay = screen.getByRole('button', { name: 'day' })
    fireEvent.click(ordinalDay)
    const ordinalMenu = screen.getByTestId('setting-dropdown-menu')
    expectLifted(ordinalMenu, ordinalDay, clip)
    expect(panel!.contains(ordinalMenu)).toBe(false)
    expect(screen.getByRole('button', { name: 'wednesday' })).toBeTruthy()
  })
})
