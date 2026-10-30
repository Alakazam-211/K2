// Settings → General → "Start page" (Rosson 2026-10-30): the page this
// window opens on, Agents (the default) or Home. Per client (localStorage,
// `stores/page-view` readStartPage / writeStartPage), not a daemon setting.
// Drawn as Settings' segmented control (Styles → Scheme), so it reads the
// same in every Style.

import { useState } from 'react'
import { readStartPage, writeStartPage, type StartPage } from '@/stores/page-view'

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
      <div role="radiogroup" aria-label="Start page" className="inline-flex flex-shrink-0 border border-[var(--color-border)]" data-testid="start-page">
        {OPTIONS.map((opt) => {
          const on = value === opt.id
          return (
            <button
              key={opt.id}
              type="button"
              role="radio"
              aria-checked={on}
              data-start-page={opt.id}
              onClick={() => {
                writeStartPage(opt.id)
                setValue(opt.id)
              }}
              className={`px-3 py-1 text-[11px] no-drag transition-colors ${
                on
                  ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer'
              }`}
            >
              {opt.label}
            </button>
          )
        })}
      </div>
    </div>
  )
}
