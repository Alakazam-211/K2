// @vitest-environment jsdom
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { focusVisibleBrowserAddress } from '../components/BrowserPane/focusBrowserAddress'
import { applyCmdL, useServerSwitcherStore } from './server-switcher'

const here = dirname(fileURLToPath(import.meta.url))

function sliceFn(src: string, startNeedle: string, until: string | null): string {
  const start = src.indexOf(startNeedle)
  if (start < 0) throw new Error(`missing ${startNeedle}`)
  if (until === null) return src.slice(start)
  const end = src.indexOf(until, start + startNeedle.length)
  if (end < 0) throw new Error(`missing ${until} after ${startNeedle}`)
  return src.slice(start, end)
}

describe('useServerSwitcherStore', () => {
  afterEach(() => {
    useServerSwitcherStore.setState({ open: false })
    vi.restoreAllMocks()
    document.body.replaceChildren()
  })

  it('toggle opens then closes', () => {
    expect(useServerSwitcherStore.getState().open).toBe(false)
    useServerSwitcherStore.getState().toggle()
    expect(useServerSwitcherStore.getState().open).toBe(true)
    useServerSwitcherStore.getState().toggle()
    expect(useServerSwitcherStore.getState().open).toBe(false)
  })

  it('setOpen is explicit', () => {
    useServerSwitcherStore.getState().setOpen(true)
    expect(useServerSwitcherStore.getState().open).toBe(true)
    useServerSwitcherStore.getState().setOpen(false)
    expect(useServerSwitcherStore.getState().open).toBe(false)
  })
})

describe('Cmd+L / Cmd+Shift+L wiring', () => {
  afterEach(() => {
    useServerSwitcherStore.setState({ open: false })
    vi.restoreAllMocks()
    document.body.replaceChildren()
  })

  it('one Cmd+L with no visible address leaves the switcher open; a second call does not toggle it closed', () => {
    const toggle = vi.spyOn(useServerSwitcherStore.getState(), 'toggle')
    const focusAddress = vi.fn(() => false)
    const toggleAssistant = vi.fn()
    applyCmdL({ shift: false, focusAddress, toggleAssistant })
    applyCmdL({ shift: false, focusAddress, toggleAssistant })
    expect(useServerSwitcherStore.getState().open).toBe(true)
    expect(toggle).not.toHaveBeenCalled()
    expect(toggleAssistant).not.toHaveBeenCalled()
  })

  it('Cmd+Shift+L calls toggleAssistant and does not open the switcher', () => {
    const focusAddress = vi.fn(() => false)
    const toggleAssistant = vi.fn()
    applyCmdL({ shift: true, focusAddress, toggleAssistant })
    expect(toggleAssistant).toHaveBeenCalledTimes(1)
    expect(focusAddress).not.toHaveBeenCalled()
    expect(useServerSwitcherStore.getState().open).toBe(false)
  })

  it('a visible address is selected only after a YES resign, not before and not on NO', async () => {
    const input = document.createElement('input')
    input.setAttribute('data-browser-address', '')
    document.body.appendChild(input)
    const events: string[] = []
    let resolveResign: (yes: boolean) => void = () => {}
    const resign = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          events.push('resign-started')
          resolveResign = resolve
        }),
    )
    vi.spyOn(input, 'focus').mockImplementation(() => {
      events.push('focus')
    })
    vi.spyOn(input, 'select').mockImplementation(() => {
      events.push('select')
    })

    expect(focusVisibleBrowserAddress(resign)).toBe(true)
    expect(events).toEqual(['resign-started'])
    expect(useServerSwitcherStore.getState().open).toBe(false)

    resolveResign(false)
    await Promise.resolve()
    expect(events).toEqual(['resign-started'])

    document.body.replaceChildren()
    const again = document.createElement('input')
    again.setAttribute('data-browser-address', '')
    document.body.appendChild(again)
    const yesEvents: string[] = []
    let resolveYes: (yes: boolean) => void = () => {}
    vi.spyOn(again, 'focus').mockImplementation(() => {
      yesEvents.push('focus')
    })
    vi.spyOn(again, 'select').mockImplementation(() => {
      yesEvents.push('select')
    })
    const resignYes = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveYes = resolve
        }),
    )
    expect(focusVisibleBrowserAddress(resignYes)).toBe(true)
    expect(yesEvents).toEqual([])
    resolveYes(true)
    await Promise.resolve()
    expect(yesEvents).toEqual(['focus', 'select'])
  })

  it('a rejected resign is not YES and does not select', async () => {
    const input = document.createElement('input')
    input.setAttribute('data-browser-address', '')
    document.body.appendChild(input)
    const focus = vi.spyOn(input, 'focus')
    const select = vi.spyOn(input, 'select')
    let rejectResign: (err: unknown) => void = () => {}
    const resign = vi.fn(
      () =>
        new Promise<boolean>((_resolve, reject) => {
          rejectResign = reject
        }),
    )
    expect(focusVisibleBrowserAddress(resign)).toBe(true)
    rejectResign(new Error('no BOOL'))
    await Promise.resolve()
    expect(focus).not.toHaveBeenCalled()
    expect(select).not.toHaveBeenCalled()
  })

  it('an aria-hidden address is ignored', async () => {
    const hiddenWrap = document.createElement('div')
    hiddenWrap.setAttribute('aria-hidden', 'true')
    const hidden = document.createElement('input')
    hidden.setAttribute('data-browser-address', '')
    hiddenWrap.appendChild(hidden)
    const visible = document.createElement('input')
    visible.setAttribute('data-browser-address', '')
    document.body.append(hiddenWrap, visible)

    const hiddenFocus = vi.spyOn(hidden, 'focus')
    const hiddenSelect = vi.spyOn(hidden, 'select')
    const visibleFocus = vi.spyOn(visible, 'focus')
    const visibleSelect = vi.spyOn(visible, 'select')
    let resolveResign: (yes: boolean) => void = () => {}
    const resign = vi.fn(
      () =>
        new Promise<boolean>((resolve) => {
          resolveResign = resolve
        }),
    )

    expect(focusVisibleBrowserAddress(resign)).toBe(true)
    expect(resign).toHaveBeenCalledTimes(1)
    resolveResign(true)
    await Promise.resolve()
    expect(hiddenFocus).not.toHaveBeenCalled()
    expect(hiddenSelect).not.toHaveBeenCalled()
    expect(visibleFocus).toHaveBeenCalledTimes(1)
    expect(visibleSelect).toHaveBeenCalledTimes(1)

    document.body.replaceChildren()
    const onlyHiddenWrap = document.createElement('div')
    onlyHiddenWrap.setAttribute('aria-hidden', 'true')
    const onlyHidden = document.createElement('input')
    onlyHidden.setAttribute('data-browser-address', '')
    onlyHiddenWrap.appendChild(onlyHidden)
    document.body.appendChild(onlyHiddenWrap)
    const ignored = vi.fn(() => Promise.resolve(true))
    expect(focusVisibleBrowserAddress(ignored)).toBe(false)
    expect(ignored).not.toHaveBeenCalled()
    applyCmdL({
      shift: false,
      focusAddress: () => focusVisibleBrowserAddress(ignored),
      toggleAssistant: vi.fn(),
    })
    expect(useServerSwitcherStore.getState().open).toBe(true)
  })

  it('renderer: keydown owns Cmd+L; the menu click opens once and does not toggle', () => {
    const app = readFileSync(resolve(here, '../App.tsx'), 'utf8')
    const keydown = sliceFn(app, "if (e.metaKey && !e.ctrlKey && !e.altKey && (e.key === 'l' || e.key === 'L'))", 'if (e.metaKey && e.key === \'j\')')
    expect(keydown).toContain('shift: e.shiftKey')
    expect(keydown).toContain('focusAddress: focusVisibleBrowserAddress')
    expect(keydown).toContain('applyCmdL(')
    expect(keydown).not.toContain('toggle()')
    expect(keydown).not.toContain('setFocus')

    const menu = sliceFn(app, "listen('menu:server-switcher'", "listen('menu:focus-window'")
    expect(menu).toContain('shift: false')
    expect(menu).toContain('focusAddress: focusVisibleBrowserAddress')
    expect(menu).toContain('applyCmdL(')
    expect(menu).not.toContain('toggle()')

    expect(app).not.toContain('useServerSwitcherStore.getState().toggle()')

    const chord = readFileSync(resolve(here, './server-switcher.ts'), 'utf8')
    const apply = sliceFn(chord, 'export function applyCmdL', null)
    expect(apply).toContain('args.toggleAssistant()')
    expect(apply).toContain('setOpen(true)')
    expect(apply).not.toContain('toggle()')

    const pane = readFileSync(
      resolve(here, '../components/BrowserPane/focusBrowserAddress.ts'),
      'utf8',
    )
    const focusFn = sliceFn(pane, 'export function focusVisibleBrowserAddress', 'function resignUiWebview')
    const resignAt = focusFn.indexOf('resign()')
    const focusAt = focusFn.indexOf('input.focus()')
    const selectAt = focusFn.indexOf('input.select()')
    expect(resignAt).toBeGreaterThan(-1)
    expect(focusAt).toBeGreaterThan(resignAt)
    expect(selectAt).toBeGreaterThan(focusAt)
    expect(focusFn).not.toContain('setFocus')
    expect(focusFn).not.toContain('.catch(select)')
    expect(pane).toContain("invoke<boolean>('ui_webview_make_first_responder')")
  })

  it('macOS View menu: Switch Server has no accelerator; Cmd+Shift+L stays the assistant', () => {
    const menu = readFileSync(resolve(here, '../../../src-tauri/src/menu.rs'), 'utf8')
    expect(menu).toContain(
      'MenuItem::with_id(handle, "server-switcher", "Switch Server", true, None::<&str>)',
    )
    expect(menu).not.toContain('Some("CmdOrCtrl+L")')
    expect(menu).toContain(
      'MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+Shift+L"))',
    )
    expect(menu).not.toContain(
      'MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+L"))',
    )
    expect(menu).toContain('emit_to_focused(app, "menu:server-switcher")')

    const resign = readFileSync(
      resolve(here, '../../../src-tauri/src/commands/ui_webview_focus.rs'),
      'utf8',
    )
    const command = sliceFn(
      resign,
      'pub async fn ui_webview_make_first_responder',
      'fn make_first_responder_yes',
    )
    const yesCall = command.indexOf('make_first_responder_yes(')
    const sendAt = command.indexOf('tx.send(yes)')
    expect(yesCall).toBeGreaterThan(-1)
    expect(sendAt).toBeGreaterThan(yesCall)
    const helper = sliceFn(resign, 'fn make_first_responder_yes', null)
    const callAt = helper.indexOf('makeFirstResponder:')
    const compareAt = helper.indexOf('accepted == YES')
    expect(callAt).toBeGreaterThan(-1)
    expect(compareAt).toBeGreaterThan(callAt)
    expect(resign).not.toMatch(/\.set_focus\s*\(/)
    expect(resign).not.toContain('performKeyEquivalent')
  })
})
