// Settings → Multiplayer: the owner's switch for the live workbook preview.
// Per computer (see lib/live-view/preview-key.ts); shown to whoever can change
// this server's multiplayer settings.

import { BetaTag, Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SettingsGroup, SettingSwitchRow } from '@/components/Settings/controls/SettingControls'
import { setLiveWorkbookPreview, useLiveWorkbookPreview } from '@/lib/live-view/preview-key'

export const LIVE_WORKBOOK_PREVIEW_NAME = 'Watch workbooks live (preview)'
export const LIVE_WORKBOOK_PREVIEW_LINE =
  'Workbook (.xlsx) tabs get a “Watch live” view: who’s here, what an agent holds and what it changed, cell by cell. For now it plays K2’s sample workbook, not your file. On this computer only.'

export function LiveWorkbookPreviewGroup(): React.JSX.Element {
  const on = useLiveWorkbookPreview()
  return (
    <SettingsGroup title="Workbooks" card testId="multiplayer-live-workbook">
      <SettingSwitchRow
        settingId="multiplayer.live-workbook-preview"
        control={<Toggle checked={on} onChange={setLiveWorkbookPreview} aria-label={`${LIVE_WORKBOOK_PREVIEW_NAME} (Beta)`} data-testid="live-workbook-preview-toggle" />}
      >
        <div className="type-row flex items-center gap-2 text-[var(--color-text-primary)]">
          <span>{LIVE_WORKBOOK_PREVIEW_NAME}</span>
          <BetaTag />
        </div>
        <div className={SETTING_ROW_HINT}>{LIVE_WORKBOOK_PREVIEW_LINE}</div>
      </SettingSwitchRow>
    </SettingsGroup>
  )
}
