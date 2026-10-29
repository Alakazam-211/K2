// @vitest-environment jsdom
// The control rail beside the device (research-mobile-pane-polish-v1 §4
// item 2), fail loud:
//  - Home, Rotate, Light/Dark and Screenshot call their gestures; Back and
//    Recents only on Android;
//  - the device's buttons wait for its screen and say why;
//  - ⋯ holds Open URL, Frame on/off, the frame rate (remote only), About
//    (with the isolation note) and Stop in red.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { MobileControlRail, type MobileControlRailProps } from './MobileControlRail'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))

function props(over: Partial<MobileControlRailProps> = {}): MobileControlRailProps {
  return {
    lane: 'android',
    live: true,
    busy: false,
    appearance: 'light',
    screenOnly: false,
    fps: null,
    deviceName: 'Pixel 9',
    deviceOs: 'android-35',
    isolation: 'Separate, not sealed.',
    onHome: vi.fn(),
    onBack: vi.fn(),
    onRecents: vi.fn(),
    onRotate: vi.fn(),
    onAppearance: vi.fn(),
    onScreenshot: vi.fn(),
    onStop: vi.fn(),
    onOpenUrl: vi.fn(),
    onScreenOnly: vi.fn(),
    onFps: vi.fn(),
    ...over,
  }
}

function tool(name: string): HTMLButtonElement {
  const el = document.querySelector(`[data-mobile-tool="${name}"]`) as HTMLButtonElement | null
  if (!el) throw new Error(`no ${name}`)
  return el
}

function more(): HTMLElement {
  fireEvent.click(tool('more'))
  const m = document.querySelector('[data-mobile-more]') as HTMLElement | null
  if (!m) throw new Error('⋯ did not open')
  return m
}

afterEach(() => cleanup())

describe('MobileControlRail', () => {
  it('the device buttons call their gestures; Back and Recents are Android only', () => {
    const p = props()
    const view = render(<MobileControlRail {...p} />)
    for (const n of ['home', 'back', 'recents', 'rotate', 'appearance', 'screenshot']) fireEvent.click(tool(n))
    expect(p.onHome).toHaveBeenCalledTimes(1)
    expect(p.onBack).toHaveBeenCalledTimes(1)
    expect(p.onRecents).toHaveBeenCalledTimes(1)
    expect(p.onRotate).toHaveBeenCalledTimes(1)
    expect(p.onAppearance).toHaveBeenCalledWith('dark')
    expect(p.onScreenshot).toHaveBeenCalledTimes(1)
    view.rerender(<MobileControlRail {...p} lane="ios" appearance="dark" />)
    expect(document.querySelector('[data-mobile-tool="back"]')).toBeNull()
    expect(document.querySelector('[data-mobile-tool="recents"]')).toBeNull()
    expect(tool('appearance').getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(tool('appearance'))
    expect(p.onAppearance).toHaveBeenLastCalledWith('light')
  })

  it('before the screen draws, the device buttons are off and say why', () => {
    render(<MobileControlRail {...props({ live: false })} />)
    for (const n of ['home', 'back', 'rotate', 'appearance']) {
      expect(tool(n).disabled).toBe(true)
      expect(tool(n).title).toBe("Waits for the device's screen")
    }
    expect(tool('screenshot').disabled).toBe(false)
  })

  it('⋯: About shows the device and the isolation note; Stop is there, in red', () => {
    const p = props()
    render(<MobileControlRail {...p} />)
    const m = more()
    expect(m.querySelector('[data-mobile-about]')?.textContent).toBe('Pixel 9android-35Separate, not sealed.')
    expect(m.querySelector('[data-mobile-fps]')).toBeNull()
    const stop = tool('stop')
    expect(stop.className).toContain('--color-danger')
    fireEvent.click(stop)
    expect(p.onStop).toHaveBeenCalledTimes(1)
    expect(document.querySelector('[data-mobile-more]')).toBeNull()
  })

  it('⋯: Frame on/off and the remote frame rate', () => {
    const p = props({ fps: 8 })
    render(<MobileControlRail {...p} />)
    const m = more()
    expect(m.querySelector('[data-mobile-menu="frame"]')?.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(m.querySelector('[data-mobile-menu="frame"]') as HTMLElement)
    expect(p.onScreenOnly).toHaveBeenCalledWith(true)
    const m2 = more()
    expect(Array.from(m2.querySelectorAll('[data-mobile-fps-choice]')).map((b) => b.textContent)).toEqual([
      '2 fps',
      '4 fps',
      '8 fps',
      '12 fps',
      '15 fps',
    ])
    expect(m2.querySelector('[data-mobile-fps-choice="8"]')?.getAttribute('aria-checked')).toBe('true')
    fireEvent.click(m2.querySelector('[data-mobile-fps-choice="12"]') as HTMLElement)
    expect(p.onFps).toHaveBeenCalledWith(12)
  })

  it('⋯: Open URL sends the trimmed URL on Enter', () => {
    const p = props()
    render(<MobileControlRail {...p} />)
    fireEvent.click(more().querySelector('[data-mobile-menu="open-url"]') as HTMLElement)
    const input = document.querySelector('[data-mobile-open-url]') as HTMLInputElement
    fireEvent.change(input, { target: { value: '  myapp://settings ' } })
    fireEvent.keyDown(input, { key: 'Enter' })
    expect(p.onOpenUrl).toHaveBeenCalledWith('myapp://settings')
    expect(document.querySelector('[data-mobile-more]')).toBeNull()
  })
})
