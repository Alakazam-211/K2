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

const THREAD_COMPOSE_TEXTAREA =
  '[data-testid="agent-session-thread-compose-slot"] [data-compose-bar] textarea'
const COMPOSE_TEXTAREA = '[data-compose-bar] textarea'

function firstVisibleTextarea(
  root: ParentNode,
  selector: string,
): HTMLTextAreaElement | null {
  const nodes = root.querySelectorAll(selector)
  for (const el of nodes) {
    if (!(el instanceof HTMLTextAreaElement)) continue
    if (isEffectivelyHidden(el)) continue
    return el
  }
  return null
}

/** Visible Message-the-agent box. Split mounts PTY compose first, then
 *  Thread — prefer the Thread-column bar when that slot is on screen. */
export function findVisibleComposeTextarea(
  root: ParentNode = document,
): HTMLTextAreaElement | null {
  return (
    firstVisibleTextarea(root, THREAD_COMPOSE_TEXTAREA) ??
    firstVisibleTextarea(root, COMPOSE_TEXTAREA)
  )
}

/** Click on Thread compose-slot padding (not the textarea) should still
 *  put the caret in the bar — App.tsx capture-click otherwise refocuses
 *  the terminal. Interactive children keep their own focus. */
export function focusThreadComposeSlotTextarea(
  slot: EventTarget | null,
  clickTarget: EventTarget | null,
): boolean {
  if (!(slot instanceof HTMLElement)) return false
  if (
    clickTarget instanceof Element &&
    clickTarget.closest('textarea, input, button, select, [contenteditable="true"]')
  ) {
    return false
  }
  const textarea = findVisibleComposeTextarea(slot)
  if (!textarea) return false
  textarea.focus()
  return true
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
    const textarea = findVisibleComposeTextarea(pane)
    if (!textarea) return false
    textarea.focus()
    return true
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
