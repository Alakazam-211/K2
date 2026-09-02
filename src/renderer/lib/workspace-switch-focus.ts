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

/** Walk ancestors. Inactive tabs stay mounted under `display:none` /
 *  `aria-hidden` (TerminalArea). getComputedStyle on the textarea itself
 *  is still `inline`/`block` — you have to look up. Focusing those bars
 *  sends keystrokes to a hidden session (looks like “another workspace”). */
export function isEffectivelyHidden(el: Element | null): boolean {
  let node: HTMLElement | null = el instanceof HTMLElement ? el : null
  while (node) {
    if (node.getAttribute('aria-hidden') === 'true') return true
    if (node.hidden || node.style.display === 'none') return true
    try {
      const style = window.getComputedStyle(node)
      if (style.display === 'none' || style.visibility === 'hidden') return true
    } catch {
      /* jsdom / detached */
    }
    node = node.parentElement
  }
  return false
}

export function findVisibleComposeTextarea(): HTMLTextAreaElement | null {
  const nodes = document.querySelectorAll('[data-compose-bar] textarea')
  for (const el of nodes) {
    if (!(el instanceof HTMLTextAreaElement)) continue
    if (isEffectivelyHidden(el)) continue
    return el
  }
  return null
}

function focusVisibleTerminal(): boolean {
  const terminals = document.querySelectorAll('[data-terminal-container]')
  for (const el of terminals) {
    if (!(el instanceof HTMLElement)) continue
    if (isEffectivelyHidden(el)) continue
    el.focus()
    return true
  }
  return false
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

function dashPaneRoot(workspaceId: string): HTMLElement | null {
  if (typeof document === 'undefined') return null
  const escaped =
    typeof CSS !== 'undefined' && typeof CSS.escape === 'function'
      ? CSS.escape(workspaceId)
      : workspaceId.replace(/\\/g, '\\\\').replace(/"/g, '\\"')
  const el = document.querySelector(`[data-dash-pane-ws="${escaped}"]`)
  return el instanceof HTMLElement ? el : null
}

/** Projects dashboard ⌘1…⌘9 (and Esc-to-pane): focus the configured
 *  input **inside that pane**, not the first visible bar on the page.
 *  Composer pref never falls through to the kessel textarea. */
export function tryFocusPreferredInputInDashboardPane(workspaceId: string): boolean {
  const pane = dashPaneRoot(workspaceId)
  if (!pane || isEffectivelyHidden(pane)) return false

  if (preferredWorkspaceSwitchFocus() === 'composer') {
    const nodes = pane.querySelectorAll('[data-compose-bar] textarea')
    for (const el of nodes) {
      if (!(el instanceof HTMLTextAreaElement)) continue
      if (isEffectivelyHidden(el)) continue
      el.focus()
      return true
    }
    return false
  }

  const terminals = pane.querySelectorAll('[data-terminal-container]')
  for (const el of terminals) {
    if (!(el instanceof HTMLElement)) continue
    if (isEffectivelyHidden(el)) continue
    el.focus()
    return true
  }
  const fallback = pane.querySelector('textarea')
  if (fallback instanceof HTMLTextAreaElement && !isEffectivelyHidden(fallback)) {
    fallback.focus()
    return true
  }
  return false
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
