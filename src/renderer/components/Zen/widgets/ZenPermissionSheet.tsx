// Permission cards v1 in a Garden (prd-agent-settings-cards-v1 §8): K2's
// sheet for a permission card a `k2-thread@1` widget shows. It is K2's own
// DOM over the widget's card, outside the sealed frame (the RS-A15 / G8
// pattern): widget code can't draw it, click it or read it. It draws the
// same PermissionCard as the desktop Thread, live (the card's next state
// shows here as it lands), and answers on the Thread's own server as this
// person. Done (also the ×, Esc and a press outside: ZenSheet) closes it
// and answers nothing; the widget only hears that it closed.

import { useEffect, useMemo, useState } from 'react'
import { ChoiceCard } from '@/components/SessionView/ChoiceCard'
import { permissionCardActions } from '@/components/SessionView/permissionCardView'
import { THREAD_CARD_TOKENS } from '@/components/Thread/ThreadZenKit'
import { onZenThreadChange, zenThreadItemTarget } from '@/lib/zen/zen-data'
import type { ZenPermissionAsk } from '@/lib/zen/permission-sheet'
import { zenButtonStyle } from './ZenOverlay'
import { ZEN_SHEET_TITLE_STYLE, ZenSheet } from './ZenSheet'

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
    <ZenSheet
      label="Permission request"
      testId="zen-permission-sheet"
      onCancel={onDone}
      maxWidth={380}
      scroll
      panelStyle={THREAD_CARD_TOKENS}
    >
      <p style={{ color: 'var(--zen-text-muted)', fontSize: 11, ...ZEN_SHEET_TITLE_STYLE }}>
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
    </ZenSheet>
  )
}
