// The no-Mac-node card (prd-ios-on-mac-node-0467-v1 §3.9): shown in the
// empty Mobile tab when no iPhone lane can run on this server. The owner
// and admins get the steps to add a Mac mini as a compute node and a button
// to Compute settings; everyone else is told who to ask.

import { useSettingsStore } from '@/stores/settings'
import type { MacNodeCard } from './simApi'

export function MacNodeNote({ note, card }: { note: string; card: MacNodeCard }): React.JSX.Element {
  return (
    <div className="flex flex-col items-center gap-1 text-center text-[11px] text-[var(--color-text-muted)]" data-mobile-mac-node={card.kind}>
      <div data-mobile-iphone-note="">{note}</div>
      {card.kind === 'add' ? (
        <div className="flex flex-col items-center gap-1">
          <div className="text-[var(--color-text-secondary)]">Add a Mac mini as a compute node:</div>
          <ol className="list-decimal pl-4 text-left" data-mobile-mac-steps="">
            {card.steps.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ol>
          <button
            type="button"
            className="h-6 w-fit cursor-pointer border border-[var(--color-border)] px-2 text-xs text-[var(--color-text)] hover:border-[var(--color-accent)]"
            onClick={() => useSettingsStore.getState().openSettings('compute')}
            data-mobile-open-compute=""
          >
            Open Compute settings
          </button>
        </div>
      ) : (
        <div data-mobile-mac-ask="">{card.line}</div>
      )}
    </div>
  )
}
