// Settings → Multiplayer: the owner's preview switches for the live views,
// one row per entry in LIVE_PREVIEW_SWITCHES (workbooks, 3D scenes,
// timelines), in one card. Per computer; shown to whoever can change this
// server's multiplayer settings. Merging them into one switch is a change
// to that list only.

import { BetaTag, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '@/components/Settings/controls/SettingControls'
import { LIVE_PREVIEW_SWITCHES, setPreviewSwitch, usePreviewSwitch, type LivePreviewSwitch } from '@/lib/live-view/shared/preview-keys'

function SwitchRow({ s }: { s: LivePreviewSwitch }): React.JSX.Element {
  const on = usePreviewSwitch(s)
  return (
    <SettingSwitchRow settingId={`multiplayer.${s.id}`} control={<Toggle checked={on} onChange={(v: boolean) => setPreviewSwitch(s, v)} aria-label={`${s.name} (Beta)`} data-testid={s.testId} />}>
      <div className="type-row flex items-center gap-2 text-[var(--color-text-primary)]">
        <span>{s.name}</span>
        <BetaTag />
      </div>
      <div className={SETTING_ROW_HINT}>{s.line}</div>
    </SettingSwitchRow>
  )
}

/** The card. Its test id predates the scene and timeline rows: other tests
 *  leave every switch inside it out of their counts. */
export function LivePreviewSwitches(): React.JSX.Element {
  return (
    <SettingsGroup title="Watch live (preview)" card testId="multiplayer-live-workbook">
      {LIVE_PREVIEW_SWITCHES.map((s) => (
        <SwitchRow key={s.id} s={s} />
      ))}
    </SettingsGroup>
  )
}
