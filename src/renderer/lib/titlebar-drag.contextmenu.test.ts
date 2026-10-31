// @vitest-environment jsdom
// Windows: right-click on K2's title bar opens the window's system menu
// (the shell's `window_system_menu`); never over a control, never on
// macOS or Linux.

import { afterEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.hoisted(() => vi.fn(async () => undefined))
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ startDragging: async () => {} }) }))

import type { MouseEvent as ReactMouseEvent } from 'react'
import { titleBarOnContextMenu } from './titlebar-drag'

const realPlatform = navigator.platform

function rightClick(target: HTMLElement): { e: ReactMouseEvent; prevented: () => boolean } {
  let prevented = false
  const e = { target, button: 2, preventDefault: () => (prevented = true) } as unknown as ReactMouseEvent
  return { e, prevented: () => prevented }
}

function setPlatform(p: string): void {
  Object.defineProperty(window.navigator, 'platform', { value: p, configurable: true })
}

afterEach(() => {
  setPlatform(realPlatform)
  invoke.mockClear()
})

describe('title bar right-click', () => {
  it('Windows: opens the system menu on the bar, not over a control', () => {
    setPlatform('Win32')
    const bar = document.createElement('div')
    const r = rightClick(bar)
    titleBarOnContextMenu(r.e)
    expect(r.prevented()).toBe(true)
    expect(invoke).toHaveBeenCalledWith('window_system_menu')

    invoke.mockClear()
    const btn = document.createElement('button')
    bar.appendChild(btn)
    const b = rightClick(btn)
    titleBarOnContextMenu(b.e)
    expect(b.prevented()).toBe(false)
    expect(invoke).not.toHaveBeenCalled()
  })

  it('macOS and Linux: untouched', () => {
    for (const p of ['MacIntel', 'Linux x86_64']) {
      setPlatform(p)
      const r = rightClick(document.createElement('div'))
      titleBarOnContextMenu(r.e)
      expect(r.prevented()).toBe(false)
    }
    expect(invoke).not.toHaveBeenCalled()
  })
})
