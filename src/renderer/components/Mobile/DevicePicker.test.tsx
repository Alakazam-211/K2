// @vitest-environment jsdom
// The grouped device list (research-mobile-pane-polish-v1 §4 item 4), fail
// loud: groups by platform, the running device first, OS · state per row,
// Default marked, Start per row with a spinner while it starts, and the
// lane's needs as cards with their one action (licence → Show the licence,
// owner step → Copy), never "go to Settings".

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { DevicePicker, type DevicePickerProps } from './DevicePicker'
import type { SimDeviceProfile } from './simApi'

const dev = (id: string, name: string, os: string, isDefault = false): SimDeviceProfile => ({
  id,
  name,
  os,
  widthPt: 400,
  heightPt: 900,
  scale: 3,
  default: isDefault,
})

const IPHONE_16 = dev('iphone_16', 'iPhone 16', 'iOS 18.2', true)
const IPHONE_SE = dev('iphone_se', 'iPhone SE', 'iOS 18.2')
const PIXEL_9 = dev('pixel_9', 'Pixel 9', 'Android 15', true)
const PIXEL_8A = dev('pixel_8a', 'Pixel 8a', 'Android 14')

function props(over: Partial<DevicePickerProps> = {}): DevicePickerProps {
  return {
    groups: [
      { platform: 'ios', devices: [IPHONE_16, IPHONE_SE], ready: true, needs: [], loading: false },
      { platform: 'android', devices: [PIXEL_9, PIXEL_8A], ready: true, needs: [], loading: false },
    ],
    up: null,
    selected: 'iphone_16',
    pending: null,
    canControl: true,
    busy: false,
    onStart: vi.fn(),
    onShowLicence: vi.fn(),
    onCopy: vi.fn(),
    ...over,
  }
}

function names(platform: string): string[] {
  return Array.from(document.querySelectorAll(`[data-mobile-group="${platform}"] [data-mobile-device-name]`)).map((n) => n.textContent ?? '')
}

function sub(id: string): string {
  const row = document.querySelector(`[data-mobile-device-row="${id}"]`)
  if (!row) throw new Error(`no row ${id}`)
  return row.querySelectorAll('span.truncate')[2]?.textContent ?? ''
}

afterEach(() => cleanup())

describe('DevicePicker', () => {
  it('groups by platform with OS, state and Default; Start boots that device on its platform', () => {
    const p = props()
    render(<DevicePicker {...p} />)
    expect(Array.from(document.querySelectorAll('[data-mobile-group] h3')).map((h) => h.textContent)).toEqual(['iPhone', 'Android'])
    expect(names('ios')).toEqual(['iPhone 16', 'iPhone SE'])
    expect(sub('pixel_8a')).toBe('Android 14 · Stopped')
    expect(document.querySelector('[data-mobile-device-row="iphone_16"]')?.textContent).toContain('Default')
    fireEvent.click(document.querySelector('[data-mobile-start="pixel_8a"]') as HTMLButtonElement)
    expect(p.onStart).toHaveBeenCalledWith('android', 'pixel_8a')
  })

  it('the running device comes first and shows On, with no Start', () => {
    render(<DevicePicker {...props({ up: { id: 'pixel_8a', state: 'running' } })} />)
    expect(names('android')).toEqual(['Pixel 8a', 'Pixel 9'])
    expect(sub('pixel_8a')).toBe('Android 14 · Running')
    expect(document.querySelector('[data-mobile-start="pixel_8a"]')).toBeNull()
  })

  it('a start in flight spins on its row and holds the other Starts', () => {
    render(<DevicePicker {...props({ pending: 'iphone_se' })} />)
    const btn = document.querySelector('[data-mobile-start="iphone_se"]') as HTMLButtonElement
    expect(btn.querySelector('.animate-spin')).not.toBeNull()
    expect(sub('iphone_se')).toBe('iOS 18.2 · Starting…')
    expect((document.querySelector('[data-mobile-start="pixel_9"]') as HTMLButtonElement).disabled).toBe(true)
  })

  it('a licence need is a card with Show the licence; its Starts are off', () => {
    const p = props({
      groups: [
        {
          platform: 'android',
          devices: [PIXEL_9],
          ready: false,
          needs: [{ kind: 'licence', id: 'android-sdk-license', title: 'Android SDK licence' }],
          loading: false,
        },
      ],
    })
    render(<DevicePicker {...p} />)
    const card = document.querySelector('[data-mobile-need="licence"]')
    expect(card?.textContent).toContain('Accept the Android SDK licence')
    expect(card?.textContent).not.toContain('Settings')
    fireEvent.click(document.querySelector('[data-mobile-need-action="licence"]') as HTMLButtonElement)
    expect(p.onShowLicence).toHaveBeenCalledWith('android')
    expect((document.querySelector('[data-mobile-start="pixel_9"]') as HTMLButtonElement).disabled).toBe(true)
  })

  it('an owner step shows the command with Copy; a first-use download is said up front', () => {
    const p = props({
      groups: [
        {
          platform: 'android',
          devices: [],
          ready: false,
          needs: [{ kind: 'owner-step', what: 'add k2 to the kvm group', command: 'sudo usermod -aG kvm k2' }],
          loading: false,
        },
        {
          platform: 'ios',
          devices: [IPHONE_16],
          ready: true,
          needs: [{ kind: 'install', what: 'the iOS runtime', bytes: 7_400_000_000 }],
          loading: false,
        },
      ],
    })
    render(<DevicePicker {...p} />)
    expect(document.querySelector('[data-mobile-need="owner-step"] code')?.textContent).toBe('sudo usermod -aG kvm k2')
    fireEvent.click(document.querySelector('[data-mobile-need-action="owner-step"]') as HTMLButtonElement)
    expect(p.onCopy).toHaveBeenCalledWith('sudo usermod -aG kvm k2')
    expect(document.querySelector('[data-mobile-group="android"]')?.textContent).toContain('No devices')
    expect(document.querySelector('[data-mobile-need="install"]')?.textContent).toBe(
      'The first start downloads the iOS runtime (about 7.4 GB).',
    )
  })

  it('a watcher sees the list without Start or the licence action', () => {
    render(
      <DevicePicker
        {...props({
          canControl: false,
          groups: [
            {
              platform: 'android',
              devices: [PIXEL_9],
              ready: false,
              needs: [{ kind: 'licence', id: 'x', title: 'Android SDK licence' }],
              loading: false,
            },
          ],
        })}
      />,
    )
    expect(document.querySelector('[data-mobile-start]')).toBeNull()
    expect(document.querySelector('[data-mobile-need-action="licence"]')).toBeNull()
  })
})
