// Settings → General → "Start page" (Rosson 2026-10-30): the page this
// window opens on, Agents (the default) or Home. Per client (localStorage,
// `stores/page-view` readStartPage / writeStartPage), not a daemon setting.
// Drawn as Settings' segmented control (Styles → Scheme), so it reads the
// same in every Style. A row of General's "This computer" card, so the
// card's hint says it applies here only.

import { useState } from 'react'
import { readStartPage, writeStartPage, type StartPage } from '@/stores/page-view'
import { Segmented } from '@/components/ui'
import { SaveError, SaveNote, SettingRow, useSaveStatus } from '../controls/SettingControls'

const OPTIONS: Array<{ id: StartPage; label: string }> = [
  { id: 'home', label: 'Home' },
  { id: 'agents', label: 'Agents' },
]

export function StartPageRow(): React.JSX.Element {
  const [value, setValue] = useState<StartPage>(() => readStartPage())
  const save = useSaveStatus()
  return (
    <SettingRow
      label="Start page"
      hint="The page K2 opens on when it starts or after you sign in."
      settingId="general.start-page"
      below={typeof save.state === 'object' ? <SaveError state={save.state} /> : undefined}
    >
      <SaveNote state={save.state} />
      <Segmented
        aria-label="Start page"
        data-testid="start-page"
        value={value}
        onChange={(next) => {
          writeStartPage(next)
          setValue(next)
          // Read back: blocked storage keeps it for this window only.
          if (readStartPage() === next) save.saved()
          else save.failed("this computer's storage is blocked, so it lasts until K2 closes")
        }}
        options={OPTIONS.map((opt) => ({
          value: opt.id,
          label: opt.label,
          buttonProps: { 'data-start-page': opt.id },
        }))}
      />
    </SettingRow>
  )
}
