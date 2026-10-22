// RS-A15: K2's confirm sheet for a Garden widget's `guests.setRooms`
// (room-scope amendment 2026-10-18; E4, the G8 pattern). It is K2's own
// DOM laid over the widget's card, outside the sealed frame: widget code
// can't draw it, click it or read it. K2 writes every word from the
// daemon's `confirm_required` change (never from the widget). Zen tokens
// only (T4.3).
//
// Approve runs the person's vault confirm (`ConfirmStep`: Touch ID on a Mac,
// else the login or vault password), bound to exactly this change; Cancel
// answers the widget `declined`.

import { useState } from 'react'
import { ConfirmStep } from '@/components/Settings/sections/DataSourceConfirm'
import { zenLocalScope } from '@/lib/zen/zen-api'
import { ZEN_GUESTS_CONFIRM_ACTION, type ZenGuestRoomsAsk } from '@/lib/zen/zen-custom-guests'
import { useGardenAccessSwitchStore } from '@/lib/zen/garden-access-api'
import { zenButtonStyle } from './ZenOverlay'

function list(handles: string[]): string {
  return handles.length ? handles.join(', ') : 'no rooms'
}

const muted: React.CSSProperties = { color: 'var(--zen-text-muted)' }

export interface ZenGuestRoomsSheetProps {
  ask: ZenGuestRoomsAsk
  /** The vault token, or null when the person said no. */
  onDone: (token: string | null) => void
}

export function ZenGuestRoomsSheet({ ask, onDone }: ZenGuestRoomsSheetProps): React.JSX.Element {
  const [confirming, setConfirming] = useState(false)
  const accessOn = useGardenAccessSwitchStore((s) => s.on) === true
  const c = ask.change
  const added = c.afterHandles.filter((h) => !c.beforeHandles.includes(h))
  const removed = c.beforeHandles.filter((h) => !c.afterHandles.includes(h))
  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-label="Change a guest’s rooms"
      data-testid="zen-guest-rooms-sheet"
      className="absolute inset-0 z-20 flex items-center justify-center"
      style={{ padding: 12, background: 'rgba(0, 0, 0, 0.4)' }}
    >
      <div
        className="w-full"
        style={{
          maxWidth: 360,
          padding: 12,
          fontSize: 12,
          color: 'var(--zen-text)',
          background: 'var(--zen-surface-raised)',
          border: '1px solid var(--zen-border)',
          display: 'flex',
          flexDirection: 'column',
          gap: 8,
        }}
      >
        <p style={{ fontWeight: 500 }}>
          The widget <span data-testid="zen-guest-rooms-widget">{c.widget}</span> wants to change which rooms{' '}
          <span data-testid="zen-guest-rooms-user">{c.username}</span> can reach in {c.appName}.
        </p>
        <dl style={{ fontSize: 11, display: 'flex', flexDirection: 'column', gap: 2 }}>
          <div>
            <dt className="inline" style={muted}>
              Now:{' '}
            </dt>
            <dd className="inline" data-testid="zen-guest-rooms-before">
              {list(c.beforeHandles)}
            </dd>
          </div>
          <div>
            <dt className="inline" style={muted}>
              After:{' '}
            </dt>
            <dd className="inline" data-testid="zen-guest-rooms-after">
              {list(c.afterHandles)}
            </dd>
          </div>
          {added.length > 0 && <div>Adds {added.join(', ')}.</div>}
          {removed.length > 0 && <div>Removes {removed.join(', ')}.</div>}
          {c.role && <div style={muted}>Their role ({c.role}) still decides what they can do in each room.</div>}
          {/* prd-garden-access-v1 G23: with Garden access on, `madeBy` is the last signer. */}
          {c.madeBy.startsWith('agent:') && (
            <div style={muted}>
              {accessOn ? `An agent last signed this widget (${c.madeBy}).` : `An agent made this widget (${c.madeBy}).`}
            </div>
          )}
        </dl>
        {confirming ? (
          <ConfirmStep
            scope={zenLocalScope()}
            action={ZEN_GUESTS_CONFIRM_ACTION}
            secretId={ask.secretId}
            label="guest rooms change"
            onToken={(t) => onDone(t)}
            onCancel={() => onDone(null)}
          />
        ) : (
          <div className="flex" style={{ gap: 6 }}>
            <button
              type="button"
              className="cursor-pointer"
              style={zenButtonStyle(true)}
              data-testid="zen-guest-rooms-approve"
              onClick={() => setConfirming(true)}
            >
              Approve
            </button>
            <button
              type="button"
              className="cursor-pointer"
              style={zenButtonStyle(false)}
              data-testid="zen-guest-rooms-cancel"
              onClick={() => onDone(null)}
            >
              Cancel
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
