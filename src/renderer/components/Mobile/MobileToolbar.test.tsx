// @vitest-environment jsdom
// The Mobile top bar (research-mobile-pane-polish-v1 §4 items 2 and 4): a
// device chip (state dot, name, OS) that opens the grouped device list,
// then Take over / Hand back, or "Watching" for a watcher. Fail loud:
//  - the chip shows the device that is up, else the tab's pick, else
//    "No devices"; it keeps its data-mobile-device hook and aria-label;
//  - it opens the device list; a row's Start calls back and closes it;
//  - a watcher can't open it;
//  - Take over shows only for a controller with a running device.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { MobileToolbar, type MobileToolbarProps } from './MobileToolbar'
import type { SimDeviceProfile, SimView } from './simApi'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

const PIXEL_9 = { id: 'pixel_9', name: 'Pixel 9', os: 'android-35', default: true } as SimDeviceProfile
const PIXEL_8A = { id: 'pixel_8a', name: 'Pixel 8a', os: 'android-34', default: false } as SimDeviceProfile
const RUNNING = { state: 'running', lane: 'android', profile: PIXEL_9 } as unknown as SimView

function props(over: Partial<MobileToolbarProps> = {}): MobileToolbarProps {
  return {
    session: null,
    picked: { name: 'Pixel 8a', os: 'android-34' },
    picker: {
      groups: [{ platform: 'android', devices: [PIXEL_9, PIXEL_8A], ready: true, needs: [], loading: false }],
      up: null,
      selected: 'pixel_8a',
      pending: null,
      canControl: true,
      busy: false,
      onStart: vi.fn(),
      onShowLicence: vi.fn(),
      onCopy: vi.fn(),
    },
    canControl: true,
    live: true,
    tookOver: false,
    onTakeOver: vi.fn(),
    onHandBack: vi.fn(),
    ...over,
  }
}

function chip(): HTMLButtonElement {
  const el = document.querySelector('[data-mobile-device]') as HTMLButtonElement | null
  if (!el) throw new Error('no device chip')
  return el
}

afterEach(() => cleanup())

describe('MobileToolbar device chip', () => {
  it('shows the tab’s pick, the running device, or No devices', () => {
    const view = render(<MobileToolbar {...props()} />)
    expect(chip().getAttribute('aria-label')).toBe('Device')
    expect(chip().textContent).toBe('Pixel 8aandroid-34')
    expect(document.querySelector('[data-mobile-device-state]')?.getAttribute('data-mobile-device-state')).toBe('none')
    view.rerender(<MobileToolbar {...props({ session: RUNNING })} />)
    expect(chip().textContent).toBe('Pixel 9android-35')
    expect(document.querySelector('[data-mobile-device-state]')?.getAttribute('data-mobile-device-state')).toBe('running')
    view.rerender(<MobileToolbar {...props({ picked: null })} />)
    expect(document.querySelector('[data-mobile-device-label]')?.textContent).toBe('No devices')
  })

  it('opens the device list; a row’s Start calls back and closes it', () => {
    const p = props()
    render(<MobileToolbar {...p} />)
    fireEvent.click(chip())
    const menu = document.querySelector('[data-mobile-device-menu]')
    if (!menu) throw new Error('the device list did not open')
    expect(Array.from(menu.querySelectorAll('[data-mobile-device-name]')).map((n) => n.textContent)).toEqual(['Pixel 9', 'Pixel 8a'])
    fireEvent.click(menu.querySelector('[data-mobile-start="pixel_9"]') as HTMLButtonElement)
    expect(p.picker.onStart).toHaveBeenCalledWith('android', 'pixel_9')
    expect(document.querySelector('[data-mobile-device-menu]')).toBeNull()
  })

  it('a watcher can’t open it', () => {
    render(<MobileToolbar {...props({ canControl: false })} />)
    expect(chip().disabled).toBe(true)
    fireEvent.click(chip())
    expect(document.querySelector('[data-mobile-device-menu]')).toBeNull()
  })
})

describe('MobileToolbar take over', () => {
  it('shows for a controller with a running device; Hand back once taken', () => {
    const p = props({ session: RUNNING })
    const view = render(<MobileToolbar {...p} />)
    fireEvent.click(document.querySelector('[data-mobile-tool="take-over"]') as HTMLButtonElement)
    expect(p.onTakeOver).toHaveBeenCalledTimes(1)
    view.rerender(<MobileToolbar {...p} tookOver />)
    fireEvent.click(document.querySelector('[data-mobile-tool="hand-back"]') as HTMLButtonElement)
    expect(p.onHandBack).toHaveBeenCalledTimes(1)
  })

  it('no device running: no Take over; a watcher sees Watching instead', () => {
    render(<MobileToolbar {...props()} />)
    expect(document.querySelector('[data-mobile-tool="take-over"]')).toBeNull()
    cleanup()
    render(<MobileToolbar {...props({ session: RUNNING, canControl: false })} />)
    expect(document.querySelector('[data-mobile-tool="take-over"]')).toBeNull()
    expect(document.querySelector('[data-mobile-watching]')?.textContent).toBe('Watching')
  })
})
