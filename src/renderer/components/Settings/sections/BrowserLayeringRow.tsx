// Settings → General → This computer → "Keep web pages live under K2's
// menus" (macOS, 0.46.17). On = the browser underlay (`browserLayering:
// underlay`, the default on macOS); off = the previous behaviour, where a
// menu or toast over a page freezes it. Per client (localStorage,
// `stores/browser-layering`), not a daemon setting. Hidden off macOS:
// Windows and Linux keep the previous behaviour.

import { Toggle } from '@/components/ui'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { webFeatures } from '@/web/features'
import { readBrowserLayering, useBrowserLayeringStore } from '@/stores/browser-layering'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'

export const BROWSER_LAYERING_LABEL = "Keep web pages live under K2's menus"

export function BrowserLayeringRow({ mac = isMacPlatform() }: { mac?: boolean }): React.JSX.Element | null {
  const mode = useBrowserLayeringStore((s) => s.mode)
  const failed = useBrowserLayeringStore((s) => s.failed)
  const setMode = useBrowserLayeringStore((s) => s.setMode)
  const save = useSaveStatus()
  if (!mac || !webFeatures.browserPane) return null
  const on = mode === 'underlay'
  const flip = (next: boolean): void => {
    const want = next ? 'underlay' : 'legacy'
    setMode(want)
    if (readBrowserLayering(true) === want) save.saved()
    else save.failed("this computer's storage is blocked, so it lasts until K2 closes")
  }
  return (
    <SettingSwitchRow
      settingId="general.browser-layering"
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle checked={on} onChange={flip} aria-label={BROWSER_LAYERING_LABEL} />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>{BROWSER_LAYERING_LABEL}</div>
      <div className={SETTING_ROW_HINT}>
        In a browser tab, menus, toasts and popovers no longer freeze the page, and clicks, scrolling and hover reach it. Off: the previous behaviour.
        {on && failed ? ' It could not start in this window, so the previous behaviour is in use until K2 restarts.' : ''}
      </div>
      <SaveError state={save.state} />
    </SettingSwitchRow>
  )
}
