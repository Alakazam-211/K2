// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { PresetsBar } from './PresetsBar'
import { menuLayerForTrigger } from '@/components/Settings/controls/SettingControls'
import { usePresetsStore } from '@/stores/presets'

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

const realFetch = usePresetsStore.getState().fetchPresets
const realCreate = usePresetsStore.getState().createPreset

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

beforeEach(() => {
  usePresetsStore.setState({
    presets: [],
    showPresetsBar: true,
    fetchPresets: vi.fn(async () => {}),
    createPreset: vi.fn(async () => ({ id: 'new' }) as never),
  })
})

afterEach(() => {
  cleanup()
  document.body.replaceChildren()
  usePresetsStore.setState({
    presets: [],
    showPresetsBar: true,
    fetchPresets: realFetch,
    createPreset: realCreate,
  })
})

describe('PresetsBar add form', () => {
  it('portals the add form under the plus button, above the clipping bar', async () => {
    const clip = renderInClip(<PresetsBar cwd="/ws" />)
    const trigger = screen.getByTitle('Add agent preset')
    const bar = trigger.parentElement as HTMLElement
    expect(bar.style.overflowY).toBe('hidden')
    expect(bar.style.height).toBe('32px')

    fireEvent.click(trigger)
    const form = screen.getByTestId('presets-bar-form')
    expectLifted(form, trigger, clip)
    expect(bar.contains(form)).toBe(false)
    expect(screen.getByPlaceholderText('Label')).toBeTruthy()
    expect(screen.getByPlaceholderText(/Command/)).toBeTruthy()

    fireEvent.change(screen.getByPlaceholderText('Label'), { target: { value: 'Aider' } })
    fireEvent.change(screen.getByPlaceholderText(/Command/), { target: { value: 'aider' } })
    fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    await waitFor(() => {
      expect(usePresetsStore.getState().createPreset).toHaveBeenCalledWith({
        label: 'Aider',
        command: 'aider',
        icon: undefined,
      })
    })
    expect(screen.queryByTestId('presets-bar-form')).toBeNull()

    fireEvent.click(screen.getByTitle('Add agent preset'))
    fireEvent.keyDown(screen.getByTestId('presets-bar-form'), { key: 'Escape' })
    expect(screen.queryByTestId('presets-bar-form')).toBeNull()
  })
})
