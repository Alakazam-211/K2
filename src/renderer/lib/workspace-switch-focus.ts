// After a workspace switch, focus the input the user asked for
// (Settings → General → Workspaces → "When moving between workspaces,
// auto-select").
//
// The compose bar only mounts once the terminal pane is ready/connecting
// with a sessionId — often later than restoreWorkspace(). The App.tsx
// 200ms "refocus terminal" poll used to win that race. Composer mode
// must NOT fall back to the terminal until we have given the bar time
// to appear, and the idle/click steal paths must honor the same pref.

import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'

let applyGen = 0
let observer: MutationObserver | null = null

function isHidden(el: Element | null): boolean {
  if (!(el instanceof HTMLElement)) return true
  if (el.style.display === 'none') return true
  const style = window.getComputedStyle(el)
  return style.display === 'none' || style.visibility === 'hidden'
}

export function findVisibleComposeTextarea(): HTMLTextAreaElement | null {
  const nodes = document.querySelectorAll('[data-compose-bar] textarea')
  for (const el of nodes) {
    if (!(el instanceof HTMLTextAreaElement)) continue
    if (isHidden(el)) continue
    const bar = el.closest('[data-compose-bar]')
    if (bar && isHidden(bar)) continue
    return el
  }
  return null
}

function focusVisibleTerminal(): boolean {
  const terminal = document.querySelector(
    '[data-terminal-container][data-terminal-visible="true"]',
  ) as HTMLElement | null
  if (!terminal || isHidden(terminal)) return false
  terminal.focus()
  return true
}

function shouldApply(): boolean {
  if (useSettingsStore.getState().settingsOpen) return false
  if (usePageViewStore.getState().page !== 'agents') return false
  return true
}

export function preferredWorkspaceSwitchFocus(): 'terminal' | 'composer' {
  return useSettingsStore.getState().workspaceSwitchFocus === 'composer'
    ? 'composer'
    : 'terminal'
}

/** One-shot: focus the configured target if it is already in the DOM.
 *  Returns true when something was focused (or we should stop trying). */
export function tryFocusPreferredWorkspaceInput(): boolean {
  if (typeof document === 'undefined') return true
  if (!shouldApply()) return true

  if (preferredWorkspaceSwitchFocus() === 'composer') {
    const textarea = findVisibleComposeTextarea()
    if (textarea) {
      textarea.focus()
      return true
    }
    return false
  }
  return focusVisibleTerminal()
}

function stopWatching(): void {
  if (observer) {
    observer.disconnect()
    observer = null
  }
}

function watchForComposeBar(gen: number): void {
  stopWatching()
  if (typeof MutationObserver === 'undefined' || !document.body) return
  observer = new MutationObserver(() => {
    if (gen !== applyGen) return
    if (tryFocusPreferredWorkspaceInput()) {
      stopWatching()
    }
  })
  observer.observe(document.body, { childList: true, subtree: true })
}

/** Focus the configured workspace-switch input. Composer mode waits for
 *  the bar to mount (observer + retries) instead of immediately focusing
 *  the terminal. */
export function applyWorkspaceSwitchFocus(): void {
  if (typeof document === 'undefined' || typeof window === 'undefined') return
  if (!shouldApply()) return

  const gen = ++applyGen
  stopWatching()

  const run = (giveUpToTerminal: boolean): void => {
    if (gen !== applyGen) return
    if (tryFocusPreferredWorkspaceInput()) {
      stopWatching()
      return
    }
    if (giveUpToTerminal && preferredWorkspaceSwitchFocus() === 'composer') {
      focusVisibleTerminal()
    }
  }

  run(false)
  if (preferredWorkspaceSwitchFocus() === 'composer') {
    watchForComposeBar(gen)
  }

  requestAnimationFrame(() => {
    if (gen !== applyGen) return
    run(false)
    window.setTimeout(() => run(false), 50)
    window.setTimeout(() => run(false), 150)
    window.setTimeout(() => run(false), 400)
    window.setTimeout(() => run(false), 800)
    window.setTimeout(() => run(true), 2000)
  })
}

/** Cancel in-flight retries (jsdom tests). */
export function __resetWorkspaceSwitchFocusForTests(): void {
  applyGen++
  stopWatching()
}
