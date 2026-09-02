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
  findVisibleComposeTextarea,
  tryFocusPreferredInputInDashboardPane,
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

  it('ignores a compose bar parked in a hidden tab (display:none + aria-hidden)', () => {
    workspaceSwitchFocus = 'composer'
    document.body.innerHTML = ''

    const hiddenWrap = document.createElement('div')
    hiddenWrap.style.display = 'none'
    hiddenWrap.setAttribute('aria-hidden', 'true')
    const hiddenBar = document.createElement('div')
    hiddenBar.setAttribute('data-compose-bar', '')
    const hiddenTa = document.createElement('textarea')
    hiddenTa.setAttribute('data-test', 'hidden')
    hiddenBar.appendChild(hiddenTa)
    hiddenWrap.appendChild(hiddenBar)
    document.body.appendChild(hiddenWrap)

    const visibleBar = document.createElement('div')
    visibleBar.setAttribute('data-compose-bar', '')
    const visibleTa = document.createElement('textarea')
    visibleTa.setAttribute('data-test', 'visible')
    visibleBar.appendChild(visibleTa)
    document.body.appendChild(visibleBar)

    expect(findVisibleComposeTextarea()).toBe(visibleTa)

    applyWorkspaceSwitchFocus()
    expect(document.activeElement).toBe(visibleTa)
    expect(document.activeElement).not.toBe(hiddenTa)
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

function mountDashPane(
  workspaceId: string,
  opts: { compose?: boolean; otherPane?: boolean } = {},
): {
  compose: HTMLTextAreaElement | null
  terminal: HTMLDivElement
  kessel: HTMLTextAreaElement
} {
  const pane = document.createElement('div')
  pane.setAttribute('data-dash-pane-ws', workspaceId)
  const terminal = document.createElement('div')
  terminal.setAttribute('data-terminal-container', '')
  terminal.tabIndex = -1
  const kessel = document.createElement('textarea')
  kessel.setAttribute('data-kessel-shadow', '')
  terminal.appendChild(kessel)
  pane.appendChild(terminal)
  let compose: HTMLTextAreaElement | null = null
  if (opts.compose) {
    const bar = document.createElement('div')
    bar.setAttribute('data-compose-bar', '')
    compose = document.createElement('textarea')
    bar.appendChild(compose)
    pane.appendChild(bar)
  }
  document.body.appendChild(pane)

  if (opts.otherPane) {
    const other = document.createElement('div')
    other.setAttribute('data-dash-pane-ws', 'other-ws')
    const otherBar = document.createElement('div')
    otherBar.setAttribute('data-compose-bar', '')
    const otherTa = document.createElement('textarea')
    otherTa.setAttribute('data-test', 'other')
    otherBar.appendChild(otherTa)
    other.appendChild(otherBar)
    document.body.appendChild(other)
  }

  return { compose, terminal, kessel }
}

describe('tryFocusPreferredInputInDashboardPane', () => {
  beforeEach(() => {
    workspaceSwitchFocus = 'terminal'
    settingsOpen = false
    page = 'projects'
    document.body.innerHTML = ''
  })

  afterEach(() => {
    document.body.innerHTML = ''
  })

  it('⌘N with Message agent focuses that pane compose bar, not kessel', () => {
    const { compose, kessel } = mountDashPane('ws-a', { compose: true, otherPane: true })
    workspaceSwitchFocus = 'composer'
    kessel.focus()

    const ok = tryFocusPreferredInputInDashboardPane('ws-a')
    expect(ok).toBe(true)
    expect(document.activeElement).toBe(compose)
    expect(document.activeElement).not.toBe(kessel)
    const other = document.querySelector('[data-test="other"]')
    expect(document.activeElement).not.toBe(other)
  })

  it('⌘N with Terminal focuses the pane terminal, not compose', () => {
    const { compose, terminal } = mountDashPane('ws-a', { compose: true })
    workspaceSwitchFocus = 'terminal'
    compose?.focus()

    const ok = tryFocusPreferredInputInDashboardPane('ws-a')
    expect(ok).toBe(true)
    expect(document.activeElement).toBe(terminal)
    expect(document.activeElement).not.toBe(compose)
  })

  it('composer pref does not fall through to kessel when the bar is missing', () => {
    const { kessel } = mountDashPane('ws-a', { compose: false })
    workspaceSwitchFocus = 'composer'
    kessel.focus()

    const ok = tryFocusPreferredInputInDashboardPane('ws-a')
    expect(ok).toBe(false)
    expect(document.activeElement).toBe(kessel)
  })

  it('unknown pane id is a no-op', () => {
    const { compose } = mountDashPane('ws-a', { compose: true })
    workspaceSwitchFocus = 'composer'
    compose?.blur()

    const ok = tryFocusPreferredInputInDashboardPane('missing')
    expect(ok).toBe(false)
    expect(document.activeElement).not.toBe(compose)
  })
})
