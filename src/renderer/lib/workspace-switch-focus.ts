// After a workspace switch, focus the input the user asked for
// (Settings → General → Workspaces → "When moving between workspaces,
// auto-select"). Do NOT call this from inside `_doSetActiveProject`
// before restoreWorkspace settles — the new tab DOM is not ready yet.
// Callers kick an immediate apply (fast path) and again after
// restoreWorkspace so remounts still land focus.

import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'

let applyGen = 0

function isHidden(el: Element | null): boolean {
  if (!(el instanceof HTMLElement)) return true
  if (el.style.display === 'none') return true
  const style = window.getComputedStyle(el)
  return style.display === 'none' || style.visibility === 'hidden'
}

function findVisibleComposeTextarea(): HTMLTextAreaElement | null {
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

function attemptFocus(): boolean {
  if (!shouldApply()) return true

  const target = useSettingsStore.getState().workspaceSwitchFocus
  if (target === 'composer') {
    const textarea = findVisibleComposeTextarea()
    if (textarea) {
      textarea.focus()
      return true
    }
  }
  return focusVisibleTerminal()
}

/** Focus the configured workspace-switch input. Retries (rAF + 50/150/400ms)
 *  because tabs remount after restoreWorkspace. */
export function applyWorkspaceSwitchFocus(): void {
  if (typeof document === 'undefined' || typeof window === 'undefined') return
  if (!shouldApply()) return

  const gen = ++applyGen
  const run = (): void => {
    if (gen !== applyGen) return
    attemptFocus()
  }

  run()
  requestAnimationFrame(() => {
    if (gen !== applyGen) return
    run()
    window.setTimeout(run, 50)
    window.setTimeout(run, 150)
    window.setTimeout(run, 400)
  })
}

/** Cancel in-flight retries (jsdom tests). */
export function __resetWorkspaceSwitchFocusForTests(): void {
  applyGen++
}
