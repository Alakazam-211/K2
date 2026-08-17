// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

let workspaceSwitchFocus: string = 'terminal'
let settingsOpen = false
vi.mock('@/stores/settings', () => ({
  useSettingsStore: {
    getState: () => ({ workspaceSwitchFocus, settingsOpen }),
  },
}))

let page = 'agents'
vi.mock('@/stores/page-view', () => ({
  usePageViewStore: {
    getState: () => ({ page }),
  },
}))

import {
  applyWorkspaceSwitchFocus,
  __resetWorkspaceSwitchFocusForTests,
} from './workspace-switch-focus'

function mountPair(opts: { compose?: boolean; composeHidden?: boolean } = {}): {
  textarea: HTMLTextAreaElement | null
  terminal: HTMLDivElement
} {
  document.body.innerHTML = ''
  const terminal = document.createElement('div')
  terminal.setAttribute('data-terminal-container', '')
  terminal.setAttribute('data-terminal-visible', 'true')
  terminal.tabIndex = -1
  document.body.appendChild(terminal)

  let textarea: HTMLTextAreaElement | null = null
  if (opts.compose) {
    const bar = document.createElement('div')
    bar.setAttribute('data-compose-bar', '')
    if (opts.composeHidden) bar.style.display = 'none'
    textarea = document.createElement('textarea')
    bar.appendChild(textarea)
    document.body.appendChild(bar)
  }
  return { textarea, terminal }
}

describe('applyWorkspaceSwitchFocus', () => {
  beforeEach(() => {
    workspaceSwitchFocus = 'terminal'
    settingsOpen = false
    page = 'agents'
    document.body.innerHTML = ''
    __resetWorkspaceSwitchFocusForTests()
  })

  afterEach(() => {
    __resetWorkspaceSwitchFocusForTests()
    document.body.innerHTML = ''
  })

  it('focuses the compose textarea when set to composer', () => {
    const { textarea, terminal } = mountPair({ compose: true })
    workspaceSwitchFocus = 'composer'

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).toBe(textarea)
    expect(document.activeElement).not.toBe(terminal)
  })

  it('focuses the visible terminal when set to terminal', () => {
    const { textarea, terminal } = mountPair({ compose: true })
    workspaceSwitchFocus = 'terminal'
    textarea?.focus()
    expect(document.activeElement).toBe(textarea)

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).toBe(terminal)
  })

  it('does not steal to the terminal while the compose bar is still missing', () => {
    const { terminal } = mountPair({ compose: false })
    workspaceSwitchFocus = 'composer'
    document.body.focus?.()
    terminal.blur()

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).not.toBe(terminal)
  })

  it('does not steal to the terminal while the compose bar is hidden', () => {
    const { textarea, terminal } = mountPair({ compose: true, composeHidden: true })
    workspaceSwitchFocus = 'composer'

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).not.toBe(terminal)
    expect(document.activeElement).not.toBe(textarea)
  })

  it('focuses the compose bar when it appears after apply', () => {
    workspaceSwitchFocus = 'composer'
    mountPair({ compose: false })
    applyWorkspaceSwitchFocus()

    const { textarea } = mountPair({ compose: true })
    applyWorkspaceSwitchFocus()
    expect(document.activeElement).toBe(textarea)
  })

  it('is a no-op when Settings is open', () => {
    const { textarea, terminal } = mountPair({ compose: true })
    workspaceSwitchFocus = 'composer'
    settingsOpen = true
    terminal.focus()

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).toBe(terminal)
    expect(document.activeElement).not.toBe(textarea)
  })

  it('is a no-op when the page is not agents', () => {
    const { textarea, terminal } = mountPair({ compose: true })
    workspaceSwitchFocus = 'composer'
    page = 'projects'
    terminal.focus()

    applyWorkspaceSwitchFocus()

    expect(document.activeElement).toBe(terminal)
    expect(document.activeElement).not.toBe(textarea)
  })
})
