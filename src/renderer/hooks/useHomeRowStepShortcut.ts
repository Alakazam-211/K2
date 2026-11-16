import { useEffect } from 'react'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { zenShownNow } from '@/lib/zen/zen-view'
import { homeRowStepForEvent, homeRowStepKeptBy, stepHomeRow } from '@/lib/home-row-step'

/**
 * ⌥⌘↑ / ⌥⌘↓ (Ctrl+Alt+Up / Down off macOS; rebindable `homeRowUp` /
 * `homeRowDown`): the Home nav's row above / below the selected one
 * (`lib/home-row-step.ts`). On Home only, with Settings closed and Zen not
 * shown; nowhere else does anything with these keys here.
 *
 * Window-level, in the capture phase: a focused terminal would otherwise
 * also send the keys to the agent's PTY. A code editor or a dialog keeps
 * them (`homeRowStepKeptBy`). One step per press: a held key does not
 * repeat (a row on another server can switch the window, as its click does).
 */
export function useHomeRowStepShortcut(): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.isComposing) return
      if (usePageViewStore.getState().page !== 'home') return
      if (useSettingsStore.getState().settingsOpen || zenShownNow()) return
      const step = homeRowStepForEvent(e, useSettingsStore.getState().keybindings, isMacPlatform())
      if (step === null) return
      if (homeRowStepKeptBy(e.target)) return
      e.preventDefault()
      e.stopPropagation()
      if (!e.repeat) stepHomeRow(step)
    }
    window.addEventListener('keydown', handler, true)
    return () => window.removeEventListener('keydown', handler, true)
  }, [])
}
