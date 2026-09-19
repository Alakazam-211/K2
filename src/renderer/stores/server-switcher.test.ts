import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { describe, expect, it, beforeEach } from 'vitest'
import { useServerSwitcherStore } from './server-switcher'

const here = dirname(fileURLToPath(import.meta.url))

describe('useServerSwitcherStore', () => {
  beforeEach(() => {
    useServerSwitcherStore.setState({ open: false })
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
  it('renderer: no-shift L opens the switcher; shift L opens the assistant', () => {
    const app = readFileSync(resolve(here, '../App.tsx'), 'utf8')
    expect(app).toContain("if (e.shiftKey) toggleAssistant()")
    expect(app).toContain('useServerSwitcherStore.getState().toggle()')
    expect(app).toContain("listen('menu:server-switcher'")
  })

  it('macOS View menu: Cmd+L Switch Server, Cmd+Shift+L Toggle Assistant', () => {
    const menu = readFileSync(resolve(here, '../../../src-tauri/src/menu.rs'), 'utf8')
    expect(menu).toContain(
      'MenuItem::with_id(handle, "server-switcher", "Switch Server", true, Some("CmdOrCtrl+L"))',
    )
    expect(menu).toContain(
      'MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+Shift+L"))',
    )
    expect(menu).not.toContain(
      'MenuItem::with_id(handle, "toggle-assistant", "Toggle Assistant", true, Some("CmdOrCtrl+L"))',
    )
  })
})
