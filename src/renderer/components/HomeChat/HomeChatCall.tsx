// DM calls on Home (prd-home-people-channels-v1 §11a.3, §11a.4): the pieces
// a person sees. Presentational only: each takes what to show and callbacks,
// so K2 Design can restyle them; the logic lives in `useDmCall`
// (`HomeChatCallControls.tsx`) and `stores/home-chat-calls.ts`.
//
// - `HomeChatRingCard`: someone is calling you (Answer, Decline).
// - `HomeChatCallingBar`: you are calling them ("Calling Alex…", Cancel; after
//   a decline or no answer, Call again).
// - `HomeChatCallStrip`: you are in the call (time, mute, leave), or on it on
//   another device.
// - `HomeChatCallRecord`: a call's line in the DM ("Missed call", "Declined",
//   "Cancelled", "Call · 12:04"), with Call back. Not a bubble: no reactions,
//   no reply.
// - `HomeChatCallButton`: the call button in the DM header.

import type { JSX } from 'react'
import type { CallRecordLine } from '@/lib/home-chat/calls'
import { formatCallDuration } from '@/lib/home-chat/calls'
import { Button } from '@/components/ui/Button'
import { floatMaterial } from '@/components/ui/float-material'
import { HomeChatPersonAvatar } from './HomeChatGlyphs'

const BTN =
  'k2-focus rounded-[var(--radius-field)] px-2.5 py-1 text-[12px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] disabled:opacity-50'

/** The ring layer's top (px): under the open chat's header when one is on
 *  screen (`headerBottom`, its bottom edge), so a ring never covers the
 *  header's controls; else just under the top bar. */
export const RING_LAYER_MIN_TOP = 56
export function ringLayerTop(headerBottom: number | null): number {
  return headerBottom === null ? RING_LAYER_MIN_TOP : Math.max(RING_LAYER_MIN_TOP, Math.round(headerBottom + 8))
}

/** Someone is calling you. A floating card: Frost with the float shadow in
 *  Glass (floatMaterial), a solid raised tile with cut corners in Terminal
 *  (the chamfer-float wrapper carries its shadow). Answer is the filled
 *  primary and Decline a quiet danger button, both the shared Button at
 *  32px or more. The name wraps to two lines rather than cutting off. */
export function HomeChatRingCard({
  callerName,
  username,
  picture,
  silent,
  serverLabel,
  onAccept,
  onDecline,
  busy = false,
}: {
  callerName: string
  /** Their username on that server (the avatar's colour). */
  username: string
  picture: string | null
  /** A silent ring: no sound (busy in a call, Do Not Disturb, a muted DM). */
  silent: boolean
  /** Another server's ring says which. */
  serverLabel: string | null
  onAccept: () => void
  onDecline: () => void
  busy?: boolean
}): JSX.Element {
  return (
    <div data-k2-chamfer-float="">
      <div
        role="alertdialog"
        aria-label={`${callerName} is calling you`}
        data-testid="home-chat-ring-card"
        data-silent={silent ? 'true' : 'false'}
        data-k2-chamfer="surface"
        {...floatMaterial('popover', 'frost')}
        className="flex w-72 flex-col gap-3 rounded-[var(--radius-box)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] p-3 [box-shadow:var(--shadow-3)]"
      >
        <div className="flex min-w-0 items-center gap-3">
          <HomeChatPersonAvatar username={username} name={callerName} picture={picture} size={36} />
          <div className="min-w-0 flex-1">
            <div className="line-clamp-2 break-words text-[13px] text-[var(--color-text-primary)]" data-testid="home-chat-ring-name">
              {callerName}
            </div>
            <div className="text-[11px] text-[var(--color-text-muted)]">
              {silent ? 'is calling you (silent)' : 'is calling you'}
              {serverLabel ? ` · ${serverLabel}` : ''}
            </div>
          </div>
        </div>
        <div className="flex justify-end gap-2">
          <Button variant="danger-quiet" size="md" className="min-h-8" data-testid="home-chat-ring-decline" disabled={busy} onClick={onDecline}>
            Decline
          </Button>
          <Button variant="accent" size="md" className="min-h-8" data-testid="home-chat-ring-accept" disabled={busy} onClick={onAccept}>
            Answer
          </Button>
        </div>
      </div>
    </div>
  )
}

/** The caller's bar. `state`: `ringing`, `busy` (they're in another call),
 *  `declined`, `missed` (no answer) or `joining` (before Sew answered). */
export function HomeChatCallingBar({
  name,
  state,
  onCancel,
  onCallAgain,
}: {
  name: string
  state: 'joining' | 'ringing' | 'busy' | 'declined' | 'missed'
  onCancel: () => void
  onCallAgain: () => void
}): JSX.Element {
  const ended = state === 'declined' || state === 'missed'
  const line =
    state === 'declined'
      ? `${name} declined`
      : state === 'missed'
        ? `${name} didn't answer`
        : state === 'busy'
          ? `Calling ${name}… (on another call)`
          : `Calling ${name}…`
  return (
    <div
      className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2 text-[12px]"
      data-testid="home-chat-calling"
      data-state={state}
      aria-live="polite"
    >
      <span className="flex-1 truncate text-[var(--color-text-primary)]">{line}</span>
      {ended ? (
        <button type="button" className={BTN} data-testid="home-chat-call-again" onClick={onCallAgain}>
          Call again
        </button>
      ) : (
        <button type="button" className={BTN} data-testid="home-chat-call-cancel" onClick={onCancel}>
          Cancel
        </button>
      )}
    </div>
  )
}

export function HomeChatCallStrip({
  seconds,
  muted,
  elsewhere,
  onMute,
  onLeave,
}: {
  /** Seconds in the call; null while it connects. */
  seconds: number | null
  muted: boolean
  /** The person is on this call from another device ("your phone"). */
  elsewhere: string | null
  onMute: () => void
  onLeave: () => void
}): JSX.Element {
  if (elsewhere) {
    return (
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2 text-[12px]" data-testid="home-chat-call-strip" data-where="elsewhere">
        <span className="flex-1 truncate text-[var(--color-text-secondary)]">On a call on {elsewhere}</span>
      </div>
    )
  }
  return (
    <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2 text-[12px]" data-testid="home-chat-call-strip" data-where="here">
      <span className="flex-1 truncate text-[var(--color-text-primary)]" data-testid="home-chat-call-time">
        {seconds === null ? 'Connecting…' : `On a call · ${formatCallDuration(seconds * 1000)}`}
      </span>
      <button type="button" className={BTN} aria-pressed={muted} data-testid="home-chat-call-mute" onClick={onMute}>
        {muted ? 'Unmute' : 'Mute'}
      </button>
      <button type="button" className={BTN} data-testid="home-chat-call-leave" onClick={onLeave}>
        Leave
      </button>
    </div>
  )
}

export function HomeChatCallRecord({
  line,
  timeLabel,
  onCallBack,
}: {
  line: CallRecordLine
  timeLabel: string
  /** Absent: no Call back (calls unavailable here, or read-only). */
  onCallBack?: () => void
}): JSX.Element {
  return (
    <div
      className="flex items-center justify-center gap-2 py-1 text-[11px] text-[var(--color-text-muted)]"
      data-testid="home-chat-call-record"
      data-kind={line.kind}
    >
      <span aria-hidden="true">{line.kind === 'missed' ? '↙' : '📞'}</span>
      <span className={line.kind === 'missed' ? 'text-[var(--color-status-error-text)]' : undefined} data-testid="home-chat-call-record-text">
        {line.text}
      </span>
      <span>· {timeLabel}</span>
      {line.callBack && onCallBack && (
        <button type="button" className="k2-focus underline" data-testid="home-chat-call-back" onClick={onCallBack}>
          Call back
        </button>
      )}
    </div>
  )
}

export function HomeChatCallButton({
  disabledReason,
  onCall,
}: {
  /** Why calling isn't possible now (a Mac server, calls off); null: it is. */
  disabledReason: string | null
  onCall: () => void
}): JSX.Element {
  return (
    <button
      type="button"
      className="k2-focus rounded-[var(--radius-field)] px-2 py-1 text-[11px] text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] disabled:opacity-50"
      data-testid="home-chat-call-button"
      disabled={disabledReason !== null}
      title={disabledReason ?? 'Call'}
      aria-label={disabledReason ? `Call (${disabledReason})` : 'Call'}
      onClick={onCall}
    >
      📞 Call
    </button>
  )
}
