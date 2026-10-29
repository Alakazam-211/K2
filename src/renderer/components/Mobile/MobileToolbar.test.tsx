// @vitest-environment jsdom
// The Mobile top bar (research-mobile-pane-polish-v1 §4 item 2): the
// platform and device menus are K2's standard dropdown (SettingDropdown),
// then Take over / Hand back, or "Watching" for a watcher. Fail loud:
//  - the trigger shows the current value and keeps its data-mobile-* hook
//    and aria-label;
//  - picking an item calls the callback with the same value as before;
//  - the disabled conditions block opening;
//  - an empty device list reads "No devices";
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

const DEVICES = [
  { id: 'pixel_9', name: 'Pixel 9' },
  { id: 'pixel_8a', name: 'Pixel 8a' },
] as unknown as SimDeviceProfile[]

const RUNNING = { state: 'running', lane: 'android' } as unknown as SimView

function props(over: Partial<MobileToolbarProps> = {}): MobileToolbarProps {
  return {
    session: null,
    lane: 'android',
    platforms: ['android', 'ios'],
    platform: 'android',
    onPlatform: vi.fn(),
    devices: DEVICES,
    device: 'pixel_9',
    canControl: true,
    busy: false,
    live: true,
    tookOver: false,
    onDevice: vi.fn(),
    onTakeOver: vi.fn(),
    onHandBack: vi.fn(),
    ...over,
  }
}

function trigger(attr: string): HTMLButtonElement {
  const el = document.querySelector(`[${attr}]`) as HTMLButtonElement | null
  if (!el) throw new Error(`no ${attr} trigger`)
  if (el.tagName !== 'BUTTON') throw new Error(`${attr} is a ${el.tagName}, not the K2 dropdown button`)
  return el
}

function menu(): HTMLElement | null {
  return document.querySelector('[data-testid="setting-dropdown-menu"]')
}

function pick(attr: string, label: string): void {
  fireEvent.click(trigger(attr))
  const open = menu()
  if (!open) throw new Error(`${attr} did not open a menu`)
  const item = Array.from(open.querySelectorAll('button')).find((b) => b.textContent === label)
  if (!item) throw new Error(`no ${label} in ${attr}`)
  fireEvent.click(item)
}

afterEach(() => cleanup())

describe('MobileToolbar menus', () => {
  it('each trigger shows its current value with its label and hooks', () => {
    render(<MobileToolbar {...props({ platform: 'ios' })} />)
    expect(trigger('data-mobile-platform').textContent).toBe('iPhone')
    expect(trigger('data-mobile-platform').getAttribute('aria-label')).toBe('Platform')
    expect(trigger('data-mobile-device').textContent).toBe('Pixel 9')
    expect(trigger('data-mobile-device').getAttribute('aria-label')).toBe('Device')
  })

  it('the open menu marks the current item and lists the same labels', () => {
    render(<MobileToolbar {...props()} />)
    fireEvent.click(trigger('data-mobile-device'))
    const open = menu()
    if (!open) throw new Error('device menu did not open')
    expect(Array.from(open.querySelectorAll('button')).map((b) => b.textContent)).toEqual(['Pixel 9', 'Pixel 8a'])
    const current = Array.from(open.querySelectorAll('button')).filter((b) => b.querySelector('svg'))
    expect(current.map((b) => b.textContent)).toEqual(['Pixel 9'])
  })

  it('picking an item calls the callback with the value and closes the menu', () => {
    const p = props()
    render(<MobileToolbar {...p} />)
    pick('data-mobile-platform', 'iPhone')
    expect(p.onPlatform).toHaveBeenCalledWith('ios')
    expect(menu()).toBeNull()
    pick('data-mobile-device', 'Pixel 8a')
    expect(p.onDevice).toHaveBeenCalledWith('pixel_8a')
  })

  it('a running device locks the platform; busy locks the device; neither opens', () => {
    render(<MobileToolbar {...props({ session: RUNNING })} />)
    expect(trigger('data-mobile-platform').disabled).toBe(true)
    fireEvent.click(trigger('data-mobile-platform'))
    expect(menu()).toBeNull()
    expect(trigger('data-mobile-device').disabled).toBe(false)
    cleanup()
    render(<MobileToolbar {...props({ busy: true })} />)
    expect(trigger('data-mobile-platform').disabled).toBe(true)
    expect(trigger('data-mobile-device').disabled).toBe(true)
    fireEvent.click(trigger('data-mobile-device'))
    expect(menu()).toBeNull()
    cleanup()
    render(<MobileToolbar {...props({ canControl: false })} />)
    expect(trigger('data-mobile-platform').disabled).toBe(true)
    expect(trigger('data-mobile-device').disabled).toBe(true)
  })

  it('no devices reads "No devices" and is disabled', () => {
    render(<MobileToolbar {...props({ devices: [], device: '' })} />)
    expect(trigger('data-mobile-device').textContent).toBe('No devices')
    expect(trigger('data-mobile-device').disabled).toBe(true)
  })

  it('one platform shows no platform menu', () => {
    render(<MobileToolbar {...props({ platforms: ['android'] })} />)
    expect(document.querySelector('[data-mobile-platform]')).toBeNull()
    expect(trigger('data-mobile-device').textContent).toBe('Pixel 9')
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
