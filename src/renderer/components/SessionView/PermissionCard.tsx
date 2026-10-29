// Permission cards v1 (prd-agent-settings-cards-v1 rev 3.1 §4.3–§4.5, §8):
// the desktop Thread's `PermissionCard` and `PermissionBatchCard`, drawn
// from the daemon's `cards=2` view (`permissionCardView.ts`). ChoiceCard
// routes here; the Garden sheet (ZenPermissionSheet) draws the same card.
//
// The look is ChoiceCard's: the `--thread-card-*` tokens, the square
// Style's chamfers, 11px chips and 10px muted lines, so a permission card
// sits in the Thread like any other card in both themes.
//
// What the person sees depends only on the view the daemon wrote for them:
//   - `approve` (at the floor): K2's headline, values, what it does, how
//     long, "<agent> says:", the notes, who it was asked for, Allow / Deny
//     (Deny takes an optional reason; a confirm action says so and runs the
//     same confirm Settings does), "Already on" with Close, and after an
//     approval "Turn off again" for 7 days.
//   - `wait` (below the floor): the waiting text (you can keep working with
//     the agent) and a note box for the owner or admin. No buttons.
// Every state line is the phone contract's (§17.2.4). The daemon re-checks
// the floor and the confirm at the click; nothing here decides who may.

import { useCallback, useState, type CSSProperties, type JSX, type ReactNode } from 'react'
import { ConfirmStep } from '@/components/Settings/sections/DataSourceConfirm'
import { CHAMFER_CUT, chamferCutFor, chamferVars } from '@/components/Thread/chamfer'
import { cardExpiryLabel } from './overlayThread'
import { PermissionReveal } from './PermissionReveal'
import {
  APP_USERS_SWITCH_COVERS,
  approverWord,
  DENY_REASON_MAX,
  forYouSecret,
  noteProblem,
  PERMISSION_LABEL,
  permissionAgent,
  permissionErrorText,
  settingsSectionFor,
  stateLine,
  stateTone,
  waitingLine,
  type PermissionAnswer,
  type PermissionBatchView,
  type PermissionCardActions,
  type PermissionCardView,
  type PermissionItem,
  type PermissionViewer,
  type RevealedSecret,
  type RevealMode,
} from './permissionCardView'

const EDGE = 'var(--thread-card-border,var(--color-border))'
const ACCENT = 'var(--thread-card-accent,var(--color-accent))'
const FILL = 'var(--thread-card-bg,var(--color-bg))'
const TEXT = 'var(--thread-card-text,var(--color-text-primary))'
const TEXT2 = 'var(--thread-card-text-secondary,var(--color-text-secondary))'
const MUTED = 'var(--thread-card-muted,var(--color-text-muted))'
const AMBER = 'var(--zen-needs-you,var(--color-status-needs-you,#e0a43a))'
const OK = 'var(--zen-ok,var(--color-status-ok,#5cc08a))'
const DANGER = 'var(--zen-danger,var(--color-status-error,#f07167))'
const RADIUS = 'rounded-[var(--thread-card-radius,0px)]'

const TONE: Record<'ok' | 'danger' | 'muted', string> = { ok: OK, danger: DANGER, muted: MUTED }

/** The card's box: ChoiceCard's frame. */
function CardFrame({ testId, children, attrs }: { testId: string; children: ReactNode; attrs: Record<string, string> }): JSX.Element {
  return (
    <div
      role="group"
      aria-label="Permission request"
      data-testid={testId}
      data-chamfer=""
      {...attrs}
      style={chamferVars({ cut: CHAMFER_CUT.surface, fill: FILL, edge: EDGE, border: 1 })}
      className={`mt-1 w-full box-border flex flex-col gap-1.5 ${RADIUS} border border-[var(--thread-card-border,var(--color-border))] bg-[var(--thread-card-bg,var(--color-bg))] px-2 py-1.5 text-[11px]`}
    >
      {children}
    </div>
  )
}

/** A card button: ChoiceCard's chip, side by side. `primary` is filled. */
function CardButton({
  label,
  testId,
  onClick,
  primary = false,
  danger = false,
  disabled = false,
}: {
  label: string
  testId: string
  onClick: () => void
  primary?: boolean
  danger?: boolean
  disabled?: boolean
}): JSX.Element {
  const edge = primary ? ACCENT : danger ? `color-mix(in srgb, ${DANGER} 60%, ${EDGE})` : EDGE
  const fill = primary ? `color-mix(in srgb, ${ACCENT} 18%, transparent)` : 'transparent'
  return (
    <button
      type="button"
      data-testid={testId}
      data-label={label}
      data-chamfer=""
      disabled={disabled}
      onClick={onClick}
      style={{ ...chamferVars({ cut: chamferCutFor(24), fill, edge, border: 1 }), borderColor: edge, background: fill, color: primary ? TEXT : TEXT2 }}
      className={`${RADIUS} border px-2.5 py-1 text-[11px] font-medium transition-colors cursor-pointer hover:text-[var(--thread-card-text,var(--color-text-primary))] disabled:cursor-not-allowed disabled:opacity-50`}
    >
      {label}
    </button>
  )
}

function Muted({ children, testId, style }: { children: ReactNode; testId?: string; style?: CSSProperties }): JSX.Element {
  return (
    <div data-testid={testId} className="text-[10px]" style={{ color: MUTED, ...style }}>
      {children}
    </div>
  )
}

/** "Loosens access" (amber) or "Tightens access". */
function DirectionTag({ direction }: { direction: 'loosen' | 'tighten' }): JSX.Element {
  const c = direction === 'loosen' ? AMBER : OK
  return (
    <span
      data-testid="permission-direction"
      data-direction={direction}
      className="inline-flex items-center px-1.5 text-[10px] font-medium"
      style={{ color: c, border: `1px solid color-mix(in srgb, ${c} 45%, transparent)`, lineHeight: '16px' }}
    >
      {direction === 'loosen' ? 'Loosens access' : 'Tightens access'}
    </span>
  )
}

/** The request's facts: headline, setting · scope · how long, values, what it does. */
function ItemFacts({ item, headline = true }: { item: PermissionItem; headline?: boolean }): JSX.Element {
  const meta = [item.setting, item.scope, item.duration].filter(Boolean).join(' · ')
  return (
    <>
      {headline && (
        <div data-testid="permission-headline" className="text-[12px] font-medium" style={{ color: TEXT }}>
          {item.whatChanges || item.setting}
        </div>
      )}
      {meta && <Muted testId="permission-meta">{meta}</Muted>}
      {item.values.length > 0 && (
        <div data-testid="permission-values" className="flex flex-wrap gap-1">
          {item.values.map((v) => (
            <span
              key={`${v.key}=${v.value}`}
              className="px-1 font-mono text-[10px]"
              style={{ color: TEXT2, border: `1px solid ${EDGE}` }}
            >
              {v.key}: {v.value}
            </span>
          ))}
        </div>
      )}
      {item.consequences && (
        <div data-testid="permission-consequences" style={{ color: TEXT2 }}>
          {item.consequences}
        </div>
      )}
    </>
  )
}

/** "<agent> says: …", the agent's words, labelled as theirs. */
function WhyLine({ item }: { item: PermissionItem }): JSX.Element | null {
  if (!item.why) return null
  return (
    <div data-testid="permission-why" style={{ color: TEXT2 }}>
      <span style={{ color: MUTED }}>{item.why.by} says: </span>
      {item.why.text}
    </div>
  )
}

/** Notes from people who wait, as approvers see them. */
function NotesList({ item }: { item: PermissionItem }): JSX.Element | null {
  if (item.notes.length === 0) return null
  return (
    <div data-testid="permission-notes" className="flex flex-col gap-0.5">
      {item.notes.map((n) => (
        <div key={`${n.by}-${n.at}`} data-testid="permission-note" style={{ color: TEXT2 }}>
          <span className="font-semibold" style={{ color: TEXT }}>
            {n.by}
          </span>{' '}
          <span style={{ color: MUTED }}>({n.role})</span>: {n.text}
        </div>
      ))}
    </div>
  )
}

/** One line for an error the daemon sent back. */
function ErrorLine({ text }: { text: string | null }): JSX.Element | null {
  if (!text) return null
  return (
    <div role="alert" data-testid="permission-error" className="text-[10px]" style={{ color: DANGER }}>
      {text}
    </div>
  )
}

const inputClass = `w-full box-border bg-transparent px-2 py-1 text-[11px] outline-none border border-[var(--thread-card-border,var(--color-border))] ${RADIUS}`

/** The note box for people below the floor (§4.3, §17.5). */
export function PermissionNoteBox({
  item,
  onNote,
}: {
  item: PermissionItem
  onNote: (text: string) => Promise<unknown>
}): JSX.Element {
  const mine = item.noteBox.mine ?? ''
  const [text, setText] = useState(mine)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [saved, setSaved] = useState(false)
  const trimmed = text.trim()
  const problem = noteProblem(trimmed, item.noteBox.max)
  const send = async (value: string): Promise<void> => {
    setBusy(true)
    setError(null)
    setSaved(false)
    try {
      await onNote(value)
      setSaved(true)
      if (value === '') setText('')
    } catch (e) {
      setError(permissionErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  return (
    <div data-testid="permission-note-box" className="flex flex-col gap-1">
      <input
        data-testid="permission-note-input"
        aria-label={`Note for ${approverWord(item.approvers.floor)}`}
        value={text}
        maxLength={item.noteBox.max}
        onChange={(e) => {
          setText(e.target.value)
          setSaved(false)
        }}
        placeholder={`Leave a note for ${approverWord(item.approvers.floor)} (optional)`}
        className={inputClass}
        style={{ color: TEXT }}
      />
      <div className="flex items-center gap-1.5">
        <CardButton
          label={mine ? 'Update note' : 'Send note'}
          testId="permission-note-send"
          disabled={busy || !trimmed || trimmed === mine || problem !== null}
          onClick={() => void send(trimmed)}
        />
        {mine && (
          <CardButton label="Remove note" testId="permission-note-remove" disabled={busy} onClick={() => void send('')} />
        )}
        {saved && <Muted testId="permission-note-saved">Sent to {approverWord(item.approvers.floor)}.</Muted>}
      </div>
      <ErrorLine text={problem && trimmed ? problem : error} />
    </div>
  )
}

type Step = { kind: 'idle' } | { kind: 'deny' } | { kind: 'confirm'; answer: string }

type CardConfirmAction = 'app-approve' | 'node-admin' | 'vault-move' | 'chat-owner-handoff' | 'sew-rollback-data-loss'

/** The password-reset card's Allow, by the mode the agent asked for. */
const REVEAL_ALLOW_LABEL: Record<RevealMode, string> = {
  password: 'Allow: show me a password',
  link: 'Allow: show me a link',
  email: 'Allow: email them a link',
}

/** The vault confirm actions a permission card may ask for (§17.4). */
function confirmActionOf(action: string | undefined): CardConfirmAction | null {
  return action === 'app-approve' ||
    action === 'node-admin' ||
    action === 'vault-move' ||
    action === 'chat-owner-handoff' ||
    action === 'sew-rollback-data-loss'
    ? action
    : null
}

/** Allow / Deny (with its optional reason) / the confirm step, for one
 *  request: a whole card, or one batch item (`item` set). */
function AnswerControls({
  item,
  actions,
  batchItem,
  always = false,
  onReveal,
}: {
  item: PermissionItem
  actions: PermissionCardActions
  batchItem: boolean
  /** The daemon offers `PERMISSION_LABEL.always` on this card. */
  always?: boolean
  /** A card that shows its approver a secret: hand it to the card. */
  onReveal?: (r: RevealedSecret) => void
}): JSX.Element {
  const [step, setStep] = useState<Step>({ kind: 'idle' })
  const [reason, setReason] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const confirm = item.proof.kind === 'confirm'
  const send = async (a: PermissionAnswer): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const r = await actions.answer(batchItem ? { ...a, item: item.requestId } : a)
      const secret = forYouSecret(r)
      if (secret && onReveal) onReveal(secret)
      setStep({ kind: 'idle' })
    } catch (e) {
      setError(permissionErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const confirmAction = confirmActionOf(item.proof.action)
  if (step.kind === 'confirm' && actions.confirmScope && confirmAction && item.proof.secretId) {
    return (
      <div data-testid="permission-confirm-sheet" className="flex flex-col gap-1">
        <div style={{ color: TEXT2 }}>Confirm it’s you to allow this. It’s the same confirm Settings asks for.</div>
        {step.answer === PERMISSION_LABEL.always && (
          <div data-testid="permission-always-covers" style={{ color: TEXT2 }}>
            This also turns on “Let this agent manage its Apps’ users”: {APP_USERS_SWITCH_COVERS}
          </div>
        )}
        <ConfirmStep
          scope={actions.confirmScope}
          action={confirmAction}
          secretId={item.proof.secretId}
          label={item.setting || item.action}
          onToken={(token) => void send({ answer: step.answer, confirmToken: token })}
          onCancel={() => setStep({ kind: 'idle' })}
        />
        <ErrorLine text={error} />
      </div>
    )
  }
  if (step.kind === 'deny') {
    return (
      <div data-testid="permission-deny-box" className="flex flex-col gap-1">
        <input
          data-testid="permission-deny-reason"
          aria-label="Why (optional)"
          value={reason}
          maxLength={DENY_REASON_MAX}
          onChange={(e) => setReason(e.target.value.replace(/[\r\n]+/g, ' '))}
          placeholder="Why? (optional; the agent sees it)"
          className={inputClass}
          style={{ color: TEXT }}
        />
        <div className="flex gap-1.5">
          <CardButton
            label={PERMISSION_LABEL.deny}
            testId="permission-deny-send"
            danger
            disabled={busy}
            onClick={() => void send({ answer: PERMISSION_LABEL.deny, ...(reason.trim() ? { reason: reason.trim() } : {}) })}
          />
          <CardButton label="Back" testId="permission-deny-back" disabled={busy} onClick={() => setStep({ kind: 'idle' })} />
        </div>
        <ErrorLine text={error} />
      </div>
    )
  }
  const canConfirmHere = !!actions.confirmScope && !!confirmAction && !!item.proof.secretId
  if (item.reveal && onReveal) {
    const mode = item.reveal.mode
    // Only a client that shows the secret says `reveal`; email needs none.
    const allowWith = (m: RevealMode): PermissionAnswer =>
      m === 'email' ? { answer: PERMISSION_LABEL.allow, mode: m } : { answer: PERMISSION_LABEL.allow, mode: m, reveal: true }
    return (
      <div className="flex flex-col gap-1">
        <div className="flex flex-wrap gap-1.5">
          <CardButton label={REVEAL_ALLOW_LABEL[mode]} testId="permission-allow" primary disabled={busy} onClick={() => void send(allowWith(mode))} />
          {item.reveal.emailOffered && mode !== 'email' && (
            <CardButton label="Email them a link instead" testId="permission-allow-email" disabled={busy} onClick={() => void send(allowWith('email'))} />
          )}
          <CardButton label={PERMISSION_LABEL.deny} testId="permission-deny" disabled={busy} onClick={() => setStep({ kind: 'deny' })} />
        </div>
        {mode !== 'email' && <Muted testId="permission-reveal-note">Shown here once, only to you. The agent never sees it.</Muted>}
        <ErrorLine text={error} />
      </div>
    )
  }
  return (
    <div className="flex flex-col gap-1">
      <div className="flex flex-wrap gap-1.5">
        <CardButton
          label={confirm ? 'Allow (needs your confirm)' : PERMISSION_LABEL.allow}
          testId="permission-allow"
          primary
          disabled={busy || (confirm && !canConfirmHere)}
          onClick={() =>
            confirm ? setStep({ kind: 'confirm', answer: PERMISSION_LABEL.allow }) : void send({ answer: PERMISSION_LABEL.allow })
          }
        />
        {always && (
          <CardButton
            label={PERMISSION_LABEL.always}
            testId="permission-always"
            disabled={busy || (confirm && !canConfirmHere)}
            onClick={() =>
              confirm ? setStep({ kind: 'confirm', answer: PERMISSION_LABEL.always }) : void send({ answer: PERMISSION_LABEL.always })
            }
          />
        )}
        <CardButton label={PERMISSION_LABEL.deny} testId="permission-deny" disabled={busy} onClick={() => setStep({ kind: 'deny' })} />
      </div>
      {confirm && !canConfirmHere && (
        <Muted testId="permission-confirm-elsewhere">Open the agent’s Thread to confirm this, or answer it on your phone.</Muted>
      )}
      <ErrorLine text={error} />
    </div>
  )
}

/** "Turn off again" on an approved standing grant (7 days, §4.4). */
function UndoButton({ label, onUndo }: { label: string; onUndo: () => Promise<unknown> }): JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return (
    <div className="flex flex-col gap-1">
      <div>
        <CardButton
          label={label}
          testId="permission-undo"
          disabled={busy}
          onClick={() => {
            setBusy(true)
            setError(null)
            onUndo()
              .catch((e: unknown) => setError(permissionErrorText(e)))
              .finally(() => setBusy(false))
          }}
        />
      </div>
      <ErrorLine text={error} />
    </div>
  )
}

/** "Close" on an "Already on" card: answers it as a no-op. */
function CloseButton({ onClose }: { onClose: () => Promise<unknown> }): JSX.Element {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  return (
    <div className="flex flex-col gap-1">
      <div>
        <CardButton
          label={PERMISSION_LABEL.close}
          testId="permission-close"
          disabled={busy}
          onClick={() => {
            setBusy(true)
            setError(null)
            onClose()
              .catch((e: unknown) => setError(permissionErrorText(e)))
              .finally(() => setBusy(false))
          }}
        />
      </div>
      <ErrorLine text={error} />
    </div>
  )
}

/** The state line, coloured by its tone. */
function StateLine({ text, item }: { text: string; item: PermissionItem }): JSX.Element {
  return (
    <div data-testid="permission-state-line" className="font-medium" style={{ color: TONE[stateTone(item.state)] }}>
      {text}
    </div>
  )
}

/** Everything under the facts for one request: the buttons, the waiting
 *  text and note box, or the closed state with its result and undo. */
function ItemOutcome({
  item,
  viewer,
  agent,
  actions,
  resultMessage,
  nowSec,
  batchItem,
  offered = true,
  always = false,
  onReveal,
}: {
  always?: boolean
  onReveal?: (r: RevealedSecret) => void
  item: PermissionItem
  viewer: PermissionViewer
  agent: string
  actions: PermissionCardActions | null
  resultMessage: string | null
  nowSec: number
  batchItem: boolean
  /** The daemon offers this viewer a label now (`choice.options`). */
  offered?: boolean
}): JSX.Element | null {
  const approver = viewer.mode === 'approve' && viewer.canAnswer
  const open = item.state === 'pending' || (item.state === 'already_done' && !item.answer)
  // A batch item's note and undo name the item (A2's routes take `item`).
  const forItem = batchItem ? item.requestId : undefined
  const note = actions?.note
  const undoFn = actions?.undo
  if (item.state === 'pending') {
    if (!approver) {
      return (
        <>
          <div data-testid="permission-waiting" style={{ color: TEXT2 }}>
            {waitingLine(item, viewer, agent)}
          </div>
          {!batchItem && <Muted testId="permission-approvers">Who can answer: {item.approvers.label}</Muted>}
          {item.noteBox.allowed && note && <PermissionNoteBox item={item} onNote={(text) => (forItem ? note(text, forItem) : note(text))} />}
        </>
      )
    }
    if (!offered) return null
    return actions ? (
      <AnswerControls item={item} actions={actions} batchItem={batchItem} always={always} onReveal={onReveal} />
    ) : (
      <Muted testId="permission-answer-elsewhere">Answer this in the agent’s Thread.</Muted>
    )
  }
  const line = stateLine(item, viewer, agent, resultMessage)
  const undo = item.undo && item.undo.until > nowSec && approver ? item.undo : null
  return (
    <>
      {line && <StateLine text={line} item={item} />}
      {item.state === 'denied' && item.answer?.reason && (
        <div data-testid="permission-deny-reason-shown" style={{ color: TEXT2 }}>
          “{item.answer.reason}”
        </div>
      )}
      {item.state === 'approved' && resultMessage && <Muted testId="permission-result">{resultMessage}</Muted>}
      {open && item.state === 'already_done' && approver && actions && offered && (
        <CloseButton onClose={() => actions.answer(batchItem ? { answer: PERMISSION_LABEL.close, item: item.requestId } : { answer: PERMISSION_LABEL.close })} />
      )}
      {undo && undoFn && <UndoButton label={undo.label} onUndo={() => (forItem ? undoFn(forItem) : undoFn())} />}
    </>
  )
}

function expiryOf(state: string, expiresAt: number | null, nowSec: number): string | null {
  if (state !== 'pending' && state !== 'already_done') return null
  return cardExpiryLabel(expiresAt, nowSec)
}

/** The "Open in Settings" link (approvers only). */
function SettingsLink({ item, actions }: { item: PermissionItem; actions: PermissionCardActions | null }): JSX.Element | null {
  const section = settingsSectionFor(item.settingsPage)
  if (!section || !actions?.openSettings) return null
  const open = actions.openSettings
  return (
    <button
      type="button"
      data-testid="permission-open-settings"
      onClick={() => open(section)}
      className="self-start text-[10px] underline-offset-2 hover:underline cursor-pointer"
      style={{ color: ACCENT, background: 'transparent', border: 0, padding: 0 }}
    >
      Open in Settings
    </button>
  )
}

export interface PermissionCardProps {
  view: PermissionCardView
  /** `action.result.message` once closed. */
  resultMessage: string | null
  /** The doc's sender (the agent's address), when `why` has no name. */
  from: string
  /** Absent: drawn read-only ("Answer this in the agent's Thread"). */
  actions: PermissionCardActions | null
  /** `choice.options`: the labels the daemon offers THIS viewer now (A3).
   *  Empty (a wait viewer, "applying"): no buttons, whatever the view says. */
  options?: { label: string }[]
  nowSec?: number
}

/** One permission request (`action.id = "permission"`). */
export function PermissionCard({ view, resultMessage, from, actions, options, nowSec }: PermissionCardProps): JSX.Element {
  // The daemon draws the options per viewer; no label offered, no button.
  const offered = options === undefined || options.length > 0
  const now = nowSec ?? Math.floor(Date.now() / 1000)
  const agent = permissionAgent(view, from)
  const approver = view.viewer.mode === 'approve' && view.viewer.canAnswer
  // Open = still waiting on someone (an "Already on" card until its Close).
  const expiry = expiryOf(view.answer ? 'answered' : view.state, view.expiresAt, now)
  // A password or link this person allowed: in this card's state only.
  const [revealed, setRevealed] = useState<RevealedSecret | null>(null)
  const forget = useCallback(() => setRevealed(null), [])
  return (
    <CardFrame testId="permission-card" attrs={{ 'data-state': view.state, 'data-mode': view.viewer.mode }}>
      <div className="flex items-center gap-1.5">
        <span className="text-[10px] font-semibold uppercase tracking-wide" style={{ color: MUTED }}>
          Permission
        </span>
        <DirectionTag direction={view.direction} />
        {view.state === 'already_done' && (
          <span data-testid="permission-already-on" className="text-[10px] font-medium" style={{ color: OK }}>
            Already on
          </span>
        )}
      </div>
      <ItemFacts item={view} />
      <WhyLine item={view} />
      {approver && <NotesList item={view} />}
      {approver && view.askedFor && <Muted testId="permission-asked-for">Asked while working with {view.askedFor}</Muted>}
      <ItemOutcome
        item={view}
        viewer={view.viewer}
        agent={agent}
        actions={actions}
        resultMessage={resultMessage}
        nowSec={now}
        batchItem={false}
        offered={offered}
        always={options?.some((o) => o.label === PERMISSION_LABEL.always) ?? false}
        onReveal={view.reveal ? setRevealed : undefined}
      />
      {revealed && <PermissionReveal secret={revealed} onGone={forget} />}
      {expiry && <Muted testId="permission-expiry">{expiry}</Muted>}
      {approver && <SettingsLink item={view} actions={actions} />}
    </CardFrame>
  )
}

export interface PermissionBatchCardProps {
  view: PermissionBatchView
  from: string
  actions: PermissionCardActions | null
  nowSec?: number
}

/** Several requests from one agent at once (`permission.batch`, §4.5). */
export function PermissionBatchCard({ view, from, actions, nowSec }: PermissionBatchCardProps): JSX.Element {
  const now = nowSec ?? Math.floor(Date.now() / 1000)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const agent = view.items.find((i) => i.why)?.why?.by || from || 'The agent'
  const approver = view.viewer.mode === 'approve' && view.viewer.canAnswer
  const pending = view.items.filter((i) => i.state === 'pending')
  const all = async (answer: string): Promise<void> => {
    if (!actions) return
    setBusy(true)
    setError(null)
    try {
      await actions.answer({ answer })
    } catch (e) {
      setError(permissionErrorText(e))
    } finally {
      setBusy(false)
    }
  }
  const expiry = expiryOf(view.state === 'pending' ? 'pending' : 'done', view.expiresAt, now)
  return (
    <CardFrame testId="permission-batch-card" attrs={{ 'data-state': view.state, 'data-mode': view.viewer.mode }}>
      <div className="flex items-center gap-1.5">
        <span className="text-[10px] font-semibold uppercase tracking-wide" style={{ color: MUTED }}>
          Permissions
        </span>
      </div>
      <div data-testid="permission-batch-headline" className="text-[12px] font-medium" style={{ color: TEXT }}>
        {agent} asks for {view.items.length} permissions.
      </div>
      {approver && pending.length > 0 && actions && (
        <div className="flex flex-wrap gap-1.5">
          {view.viewer.canAnswerAll && (
            <CardButton
              label={`${PERMISSION_LABEL.allowAll} (${pending.length})`}
              testId="permission-allow-all"
              primary
              disabled={busy}
              onClick={() => void all(PERMISSION_LABEL.allowAll)}
            />
          )}
          <CardButton label={PERMISSION_LABEL.denyAll} testId="permission-deny-all" disabled={busy} onClick={() => void all(PERMISSION_LABEL.denyAll)} />
        </div>
      )}
      <ErrorLine text={error} />
      <div className="flex flex-col gap-1.5">
        {view.items.map((item) => (
          <div
            key={item.requestId}
            data-testid="permission-batch-item"
            data-request={item.requestId}
            data-state={item.state}
            className="flex flex-col gap-1 pt-1.5"
            style={{ borderTop: `1px solid ${EDGE}` }}
          >
            <div className="flex items-center gap-1.5">
              <DirectionTag direction={item.direction} />
              {item.proof.kind === 'confirm' && item.state === 'pending' && approver && (
                <Muted testId="permission-batch-confirm">needs your confirm</Muted>
              )}
            </div>
            <ItemFacts item={item} />
            <WhyLine item={item} />
            {approver && <NotesList item={item} />}
            <ItemOutcome
              item={item}
              viewer={view.viewer}
              agent={agent}
              actions={actions}
              resultMessage={null}
              nowSec={now}
              batchItem
            />
          </div>
        ))}
      </div>
      {!approver && <Muted testId="permission-approvers">Who can answer: {view.approvers.label}</Muted>}
      {expiry && <Muted testId="permission-expiry">{expiry}</Muted>}
    </CardFrame>
  )
}
