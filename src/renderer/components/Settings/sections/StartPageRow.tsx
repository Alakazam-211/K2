// Settings → General → "Start page" (Rosson 2026-10-30): the page this
// window opens on, Agents (the default) or Home. Per client (localStorage,
// `stores/page-view` readStartPage / writeStartPage), not a daemon setting.
// Drawn as Settings' segmented control (Styles → Scheme), so it reads the
// same in every Style.

import { useState } from 'react'
import { readStartPage, writeStartPage, type StartPage } from '@/stores/page-view'
import { Segmented } from '@/components/ui'

const OPTIONS: Array<{ id: StartPage; label: string }> = [
  { id: 'home', label: 'Home' },
  { id: 'agents', label: 'Agents' },
]

export function StartPageRow(): React.JSX.Element {
  const [value, setValue] = useState<StartPage>(() => readStartPage())
  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.start-page"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Start page</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          The page K2 opens on when it starts or after you sign in. On this computer only.
        </p>
      </div>
      <Segmented
        aria-label="Start page"
        data-testid="start-page"
        value={value}
        onChange={(next) => {
          writeStartPage(next)
          setValue(next)
        }}
        options={OPTIONS.map((opt) => ({
          value: opt.id,
          label: opt.label,
          buttonProps: { 'data-start-page': opt.id },
        }))}
      />
    </div>
  )
}
