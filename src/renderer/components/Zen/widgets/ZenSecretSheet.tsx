// Secret cards in a custom Garden (catalog v50): K2's sheet for a secret
// card a widget shows. It is K2's own DOM over the widget's card, outside
// the sealed frame (the permission-card pattern): widget code can't draw
// it, click it or read it. It draws the compact SecretCard that K2's
// Conversation widget draws, live, and the person's Save goes through the
// window's own answer path with no widget origin, exactly like K2's
// Thread. Save or Close closes it; the widget only hears that it closed.

import { useEffect, useState } from 'react'
import { SecretCard } from '@/components/SessionView/SecretCard'
import type { ThreadAnswerPayload } from '@/components/SessionView/overlayThread'
import { THREAD_CARD_TOKENS } from '@/components/Thread/ThreadZenKit'
import { onZenThreadChange, zenThreadItemTarget } from '@/lib/zen/zen-data'
import type { ZenSecretAsk } from '@/lib/zen/secret-sheet'
import { zenButtonStyle } from './ZenOverlay'

export interface ZenSecretSheetProps {
  ask: ZenSecretAsk
  /** The window's own answer path (the Conversation widget's): never a
   *  widget origin. Throws the daemon's refusal. */
  answer: (payload: ThreadAnswerPayload & { secret: string }) => Promise<void>
  /** Dismiss the card, as the person's Dismiss on K2's Thread. */
  dismiss: () => Promise<void>
  onDone: () => void
  /** Tests: the target lookup and its change feed (zen-data by default). */
  target?: typeof zenThreadItemTarget
  onChange?: typeof onZenThreadChange
}

export function ZenSecretSheet({
  ask,
  answer,
  dismiss,
  onDone,
  target = zenThreadItemTarget,
  onChange = onZenThreadChange,
}: ZenSecretSheetProps): React.JSX.Element {
  const [t, setT] = useState(() => target(ask.address, ask.cardId))
  useEffect(() => onChange(ask.address, () => setT(target(ask.address, ask.cardId))), [ask.address, ask.cardId, target, onChange])
  const secret = t?.item.doc.kind === 'secret' ? (t.item.doc.secret ?? null) : null
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Secret request"
      data-testid="zen-secret-sheet"
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onDone()
        }
      }}
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
          K2 shows this outside the widget. The widget never sees what you type here.
        </p>
        {secret ? (
          <SecretCard
            compact
            secret={secret}
            agent={t?.item.doc.from ?? ''}
            server={t?.scope.label}
            onSubmit={async (payload) => {
              await answer(payload)
              onDone()
            }}
            onDismiss={() => {
              void dismiss()
                .catch((err: unknown) => console.warn('[zen] dismissing the secret card failed:', err))
                .finally(onDone)
            }}
          />
        ) : (
          <p data-testid="zen-secret-sheet-gone">This card isn’t on screen any more.</p>
        )}
        <div className="flex" style={{ gap: 6 }}>
          <button type="button" className="cursor-pointer" style={zenButtonStyle(false)} data-testid="zen-secret-sheet-close" onClick={onDone}>
            Close
          </button>
        </div>
      </div>
    </div>
  )
}
