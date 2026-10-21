// Permission cards v1 in a Garden (prd-agent-settings-cards-v1 §8): K2's
// sheet for a permission card a `k2-thread@1` widget shows. It is K2's own
// DOM over the widget's card, outside the sealed frame (the RS-A15 / G8
// pattern): widget code can't draw it, click it or read it. It draws the
// same PermissionCard as the desktop Thread, live (the card's next state
// shows here as it lands), and answers on the Thread's own server as this
// person. Done closes it; the widget only hears that it closed.

import { useEffect, useMemo, useState } from 'react'
import { ChoiceCard } from '@/components/SessionView/ChoiceCard'
import { permissionCardActions } from '@/components/SessionView/permissionCardView'
import { THREAD_CARD_TOKENS } from '@/components/Thread/ThreadZenKit'
import { onZenThreadChange, zenThreadItemTarget } from '@/lib/zen/zen-data'
import type { ZenPermissionAsk } from '@/lib/zen/permission-sheet'
import { zenButtonStyle } from './ZenOverlay'

export interface ZenPermissionSheetProps {
  ask: ZenPermissionAsk
  onDone: () => void
  /** Tests: the target lookup and its change feed (zen-data by default). */
  target?: typeof zenThreadItemTarget
  onChange?: typeof onZenThreadChange
}

export function ZenPermissionSheet({
  ask,
  onDone,
  target = zenThreadItemTarget,
  onChange = onZenThreadChange,
}: ZenPermissionSheetProps): React.JSX.Element {
  const [t, setT] = useState(() => target(ask.address, ask.cardId))
  useEffect(() => onChange(ask.address, () => setT(target(ask.address, ask.cardId))), [ask.address, ask.cardId, target, onChange])
  const scope = t?.scope ?? null
  const threadAddr = t?.threadAddr ?? ''
  const actions = useMemo(
    () => (scope ? permissionCardActions(scope, threadAddr, ask.cardId) : null),
    [scope, threadAddr, ask.cardId],
  )
  const choice = t?.item.doc.choice ?? null
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Permission request"
      data-testid="zen-permission-sheet"
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
    >
      <div
        className="w-full"
        style={{
          maxWidth: 380,
          maxHeight: '100%',
          overflow: 'auto',
          padding: 12,
          fontSize: 12,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
          ...THREAD_CARD_TOKENS,
        }}
      >
        <p style={{ color: 'var(--zen-text-muted)', fontSize: 11 }}>
          K2 shows this outside the widget. Only you can answer it here.
        </p>
        {choice?.permission ? (
          <ChoiceCard
            options={choice.options}
            allowCustom={false}
            status={choice.status}
            answer={choice.answer}
            expiresAt={choice.expiresAt}
            resultMessage={choice.resultMessage}
            permission={choice.permission}
            permissionActions={actions}
            from={t?.item.doc.from ?? ''}
            onPick={() => undefined}
          />
        ) : (
          <p data-testid="zen-permission-sheet-gone">This card isn’t on screen any more.</p>
        )}
        <div className="flex" style={{ gap: 6 }}>
          <button
            type="button"
            className="cursor-pointer"
            style={zenButtonStyle(false)}
            data-testid="zen-permission-sheet-done"
            onClick={onDone}
          >
            Done
          </button>
        </div>
      </div>
    </div>
  )
}
