// A K2 notice in an agent's Thread, drawn as a system row so it is plain it
// comes from K2 and not the agent (`parseSystemNotice` decides which posts
// are notices: the daemon's system stamp, never the words).
//
//   - A row, not a bubble: centred across the column, no picture and no
//     speaker name. A quiet chip with a small K2 mark says "K2 · system";
//     the words sit under it in the secondary colour, a step smaller than a
//     message.
//   - Two looks share the markup (`data-notice-look`): `rules` puts hairline
//     rules either side of the chip; `band` lays a quiet tinted band behind
//     the row.
//   - Its kind sets one tone (`--notice-tone`): info is muted; delivery (a
//     message that didn't arrive) is the status warning colour, with a
//     warning icon, never the error red; action is the accent.
//   - Its buttons are the notice's own actions: a route posts its body to
//     the Thread's server (busy while it goes, then a quiet "Sent again"
//     with a tick, or the reason in muted words); open_terminal puts the
//     agent's terminal on screen. A button this view can't carry out (no
//     server here, no terminal here) isn't drawn.
//   - The row takes focus (a note named by its label and words), and its
//     buttons follow it in the tab order.

import { useCallback, useId, useState, type CSSProperties, type JSX } from 'react'
import { CheckIcon, TriangleAlertIcon } from 'lucide-react'
import k2Logo from '@/assets/k2-logo.png'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { useSessionViewChrome } from '@/components/SessionView/sessionViewChrome'
import { daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { CHAMFER_CUT, CHAMFER_SCOPE, chamferCutFor, chamferPolygon, chamferVars } from './chamfer'
import { useThreadAttachmentSource } from './ThreadImageObjects'
import type { SystemNotice, SystemNoticeAction, SystemNoticeKind } from './systemNotice'

/** Which look the row wears: hairline rules beside the label, or a quiet
 *  tinted band behind the row. */
export type SystemNoticeLook = 'rules' | 'band'
export const SYSTEM_NOTICE_LOOK: SystemNoticeLook = 'rules'

/** The label on every notice. */
export const SYSTEM_NOTICE_LABEL = 'K2 · system'

/** Each kind's tone: the chip, the rules or band, and the icon. */
export const SYSTEM_NOTICE_TONE: Record<SystemNoticeKind, string> = {
  info: 'var(--zen-text-muted)',
  delivery: 'var(--color-status-warn-text, var(--color-status-warn))',
  action: 'var(--zen-focus-ring, var(--zen-accent))',
}

/** What a posting button says while it goes and once it went, by route. */
const POST_WORDS: Record<string, { busy: string; done: string }> = {
  'threads/resend': { busy: 'Sending…', done: 'Sent again' },
}
const DEFAULT_POST_WORDS = { busy: 'Working…', done: 'Done' }
export const postWords = (route: string): { busy: string; done: string } => POST_WORDS[route] ?? DEFAULT_POST_WORDS

/** Buttons that went through, by notice and action, so a row drawn again
 *  (scrolled away and back) still says so. */
const doneActions = new Set<string>()

export function resetSystemNoticeDoneForTests(): void {
  doneActions.clear()
}

const ROW = '[data-thread-system-notice]'
const TERMINAL = `${CHAMFER_SCOPE} ${ROW}`
const corners = (px: number): string => `--ch-tl: ${px}px; --ch-tr: ${px}px; --ch-br: ${px}px; --ch-bl: ${px}px;`
const tint = (pct: number): string => `color-mix(in srgb, var(--notice-tone) ${pct}%, transparent)`

export const THREAD_SYSTEM_NOTICE_CSS = `
${ROW} { box-sizing: border-box; width: 100%; max-width: min(100%, 600px); margin: 4px auto; display: flex; flex-direction: column; align-items: center; gap: 6px; text-align: center; color: var(--zen-text-secondary, var(--color-text-secondary, var(--zen-text))); outline: none; border-radius: calc(var(--zen-radius) / 2); }
${ROW}:focus-visible { outline: 2px solid var(--zen-focus-ring, var(--zen-accent)); outline-offset: 3px; }
${ROW} [data-notice-head] { display: flex; align-items: center; gap: 10px; width: 100%; min-width: 0; }
${ROW} [data-notice-rule] { flex: 1 1 0; min-width: 12px; height: 1px; background: ${tint(45)}; }
${ROW}[data-notice-kind="info"] [data-notice-rule] { background: var(--zen-border); }
${ROW} [data-notice-label] { flex: 0 0 auto; display: inline-flex; align-items: center; gap: 5px; padding: 1px 8px 1px 4px; font-size: var(--zen-type-sm, 11px); line-height: 16px; font-weight: 500; letter-spacing: 0.01em; color: var(--zen-text-muted); white-space: nowrap; border: 1px solid ${tint(30)}; background: ${tint(10)}; border-radius: 999px; }
${ROW}[data-notice-kind="info"] [data-notice-label] { border-color: var(--zen-border); background: color-mix(in srgb, var(--zen-text) 4%, transparent); }
${ROW} [data-notice-mark] { width: 12px; height: 12px; border-radius: 3px; flex: 0 0 auto; }
${ROW} [data-notice-body] { display: flex; align-items: flex-start; justify-content: center; gap: 6px; max-width: 100%; min-width: 0; padding: 0 8px; font-size: 0.92em; line-height: var(--zen-line-height, 1.45); overflow-wrap: anywhere; }
${ROW} [data-notice-icon] { flex: 0 0 auto; width: 1.05em; height: 1.05em; margin-top: 0.2em; color: var(--notice-tone); }
${ROW} [data-notice-text] { min-width: 0; text-align: center; }
${ROW} [data-notice-text] .chat-markdown p { margin: 0; }
${ROW} [data-notice-actions] { display: flex; flex-wrap: wrap; align-items: center; justify-content: center; gap: 8px; }
${ROW} [data-notice-action] { border-radius: calc(var(--zen-radius) / 2); }
${ROW} [data-notice-action]:focus-visible { outline: 2px solid var(--zen-focus-ring, var(--zen-accent)); outline-offset: 2px; }
${ROW} [data-notice-action][aria-busy="true"] { cursor: progress; }
${ROW} [data-notice-done] { display: inline-flex; align-items: center; gap: 4px; font-size: var(--zen-type-sm, 11px); color: var(--zen-text-muted); }
${ROW} [data-notice-tick] { width: 12px; height: 12px; color: var(--zen-ok, currentColor); }
${ROW} [data-notice-error] { flex-basis: 100%; font-size: var(--zen-type-sm, 11px); }
${ROW}[data-notice-look="band"] { padding: 8px 14px 10px; background: ${tint(8)}; border-radius: var(--zen-radius); }
${ROW}[data-notice-look="band"][data-notice-kind="info"] { background: color-mix(in srgb, var(--zen-text) 5%, transparent); }
${ROW}[data-notice-look="band"] [data-notice-rule] { display: none; }
${ROW}[data-notice-look="band"] [data-notice-head] { justify-content: center; }
${TERMINAL} { border-radius: 0; }
${TERMINAL} [data-notice-label] { border: none; border-radius: 0; padding: 2px 9px 2px 5px; background: ${tint(16)}; ${corners(chamferCutFor(18))} clip-path: ${chamferPolygon('0px')}; }
${TERMINAL}[data-notice-kind="info"] [data-notice-label] { background: color-mix(in srgb, var(--zen-text) 9%, transparent); }
${TERMINAL} [data-notice-mark] { border-radius: 0; }
${TERMINAL}[data-notice-look="band"] { background: transparent; position: relative; isolation: isolate; ${corners(CHAMFER_CUT.surface)} }
${TERMINAL}[data-notice-look="band"]::before { content: ''; position: absolute; inset: 0; z-index: -1; pointer-events: none; background: ${tint(8)}; clip-path: ${chamferPolygon('0px')}; }
${TERMINAL}[data-notice-look="band"][data-notice-kind="info"]::before { background: color-mix(in srgb, var(--zen-text) 5%, transparent); }
@media (prefers-reduced-transparency: reduce) {
  ${ROW}[data-notice-look="band"] { background: var(--zen-surface); }
}
`

const ACTION_BUTTON: CSSProperties = { minHeight: 24 }

/** One button: posts, or opens the terminal. Null when this view can't. */
function NoticeActionButton({
  docId,
  action,
  primary,
}: {
  docId: string
  action: SystemNoticeAction
  primary: boolean
}): JSX.Element | null {
  const source = useThreadAttachmentSource()
  const chrome = useSessionViewChrome()
  const key = `${docId}\u0000${action.id}`
  const [busy, setBusy] = useState(false)
  const [done, setDone] = useState(() => doneActions.has(key))
  const [error, setError] = useState<string | null>(null)
  const call = action.call
  const scope = source?.scope ?? null
  const openTerminal = chrome?.openTerminal
  const run = useCallback(async () => {
    if (call.kind !== 'post' || !scope || busy) return
    setBusy(true)
    setError(null)
    try {
      await daemonCliPost(scope, call.route, call.body)
      doneActions.add(key)
      setDone(true)
    } catch (err) {
      console.warn('[thread] notice action failed:', err)
      setError(daemonErrorText(err) || 'K2 couldn’t do that.')
    } finally {
      setBusy(false)
    }
  }, [call, scope, busy, key])
  if (call.kind === 'open_terminal') {
    if (!openTerminal) return null
    return (
      <NoticeButton label={action.label} testId={`notice-action-${action.id}`} primary={primary} onClick={openTerminal} />
    )
  }
  if (!scope) return null
  const words = postWords(call.route)
  if (done) {
    return (
      <span data-notice-done="" role="status">
        <CheckIcon data-notice-tick="" aria-hidden="true" strokeWidth={2.5} />
        {words.done}
      </span>
    )
  }
  return (
    <>
      <NoticeButton
        label={busy ? words.busy : action.label}
        testId={`notice-action-${action.id}`}
        primary={primary}
        busy={busy}
        onClick={() => void run()}
      />
      {error && (
        <span data-notice-error="" role="alert" style={{ color: 'var(--zen-text-muted)' }}>
          {error}
        </span>
      )}
    </>
  )
}

/** The Thread cards' button (chamfered in Terminal, rounded in Glass). */
function NoticeButton({
  label,
  testId,
  primary,
  busy = false,
  onClick,
}: {
  label: string
  testId: string
  primary: boolean
  busy?: boolean
  onClick: () => void
}): JSX.Element {
  const edge = primary ? 'var(--zen-accent)' : 'var(--zen-border)'
  const fill = primary ? 'color-mix(in srgb, var(--zen-accent) 16%, transparent)' : 'transparent'
  return (
    <button
      type="button"
      data-testid={testId}
      data-notice-action=""
      data-chamfer=""
      disabled={busy}
      aria-busy={busy ? 'true' : undefined}
      onClick={onClick}
      style={{
        ...ACTION_BUTTON,
        ...chamferVars({ cut: chamferCutFor(24), fill, edge, border: 1 }),
        borderColor: edge,
        background: fill,
        color: primary ? 'var(--zen-text)' : 'var(--zen-text-secondary, var(--color-text-secondary, var(--zen-text)))',
      }}
      className="border px-3 py-1 text-[11px] font-medium whitespace-nowrap transition-colors cursor-pointer hover:text-[var(--zen-text)] disabled:opacity-70"
    >
      {label}
    </button>
  )
}

/** The notice row (inside the Thread's row for `item`). */
export function ThreadSystemNoticeRow({
  item,
  notice,
  look = SYSTEM_NOTICE_LOOK,
}: {
  item: OverlayThreadItem
  notice: SystemNotice
  look?: SystemNoticeLook
}): JSX.Element {
  const id = useId()
  const labelId = `${id}-label`
  const textId = `${id}-text`
  const firstPost = notice.actions.findIndex((a) => a.call.kind === 'post')
  return (
    <div
      data-testid="thread-system-notice"
      data-thread-system-notice=""
      data-notice-kind={notice.kind}
      data-notice-code={notice.code || undefined}
      data-notice-look={look}
      role="note"
      tabIndex={0}
      aria-labelledby={`${labelId} ${textId}`}
      style={{ ['--notice-tone' as string]: SYSTEM_NOTICE_TONE[notice.kind] } as CSSProperties}
    >
      <div data-notice-head="">
        <span data-notice-rule="" aria-hidden="true" />
        <span data-notice-label="" id={labelId}>
          <img data-notice-mark="" src={k2Logo} alt="" draggable={false} />
          {SYSTEM_NOTICE_LABEL}
        </span>
        <span data-notice-rule="" aria-hidden="true" />
      </div>
      <div data-notice-body="">
        {notice.kind === 'delivery' && <TriangleAlertIcon data-notice-icon="warn" aria-hidden="true" strokeWidth={2} />}
        <div data-notice-text="" id={textId} className="selectable-copy">
          <ChatMessageBody text={notice.text} style={{ color: 'inherit', fontSize: 'inherit' }} />
        </div>
      </div>
      {notice.actions.length > 0 && (
        <div data-notice-actions="">
          {notice.actions.map((a, i) => (
            <NoticeActionButton key={a.id} docId={item.id} action={a} primary={i === firstPost} />
          ))}
        </div>
      )}
    </div>
  )
}
