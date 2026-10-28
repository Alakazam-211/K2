// The Thread's look, shared by the Zen Thread widget (Gardens, from K2
// Companion) and the desktop Thread beside an agent (Rosson 2026-10-16:
// "update the thread UI in K2 to match what we have in mobile/the zen
// garden"). One set of pieces, so the two stay in sync:
//
//   - `ThreadBubble`: a message as a text bubble (mine right, others left),
//     the choice and secret cards inside it, "author · time" under it.
//   - `ThreadTypingIndicator`, `ThreadLoadOlder`, `ThreadEmptyNote`.
//   - The message box parts (`ThreadComposeBox`, `ThreadRoundButton`,
//     `ThreadSendButton`, `ThreadAttachmentChip`).
//   - `THREAD_ZEN_CSS`: motion and hover rules for all of the above.
//
// Everything paints with `--zen-*` tokens. Inside Zen the theme engine sets
// them; the desktop Thread sets them from the app's Style
// (`THREAD_ZEN_DESKTOP_TOKENS` on a `[data-k2-thread-zen]` box), so the
// shapes are Zen's and the colours are the window's.

import { isValidElement, memo, useCallback, useMemo, useState, type CSSProperties, type JSX, type ReactNode, type Ref } from 'react'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { ChoiceCard } from '@/components/SessionView/ChoiceCard'
import { SecretCard } from '@/components/SessionView/SecretCard'
import { HandoffCard } from '@/components/SessionView/HandoffCard'
import type { HandoffDecideBody, HandoffDecideResult } from '@/components/Settings/sections/handoff-api'
import { coerceHandoff, sameThreadItem, type OverlayThreadItem, type ThreadAnswerPayload } from '@/components/SessionView/overlayThread'
import { cardAnswererLabel, type ThreadViewer } from '@/lib/thread-viewer'
import { zenBubbleShield } from '@/lib/zen/zen-style-shield'
import { ZEN_SOLID_PROPS, ZEN_TILE_RAISED_PROPS } from '@/lib/zen/zen-materials'
import { ThreadContextLine } from './ThreadContextChip'
import { ThreadFileObjects } from './ThreadFileObjects'
import { useThreadAttachmentSource } from './ThreadImageObjects'
import {
  permissionCardActions,
  permissionClient,
  type PermissionCardActions,
} from '@/components/SessionView/permissionCardView'
import { openSettingsPage } from '@/lib/open-settings-page'
import type { SettingsSection } from '@/stores/settings'
import { threadBubbleMaxWidth, useThreadColumnNarrow } from './ThreadColumn'

import {
  CHAMFER_CUT,
  THREAD_CHAMFER_CSS,
  THREAD_ZEN_SCOPE_ATTR,
  chamferCutFor,
  chamferVars,
} from './chamfer'

// The square Style's chamfers live in ./chamfer (the cards use them too).
export {
  CHAMFER_CUT,
  CHAMFER_SCOPE,
  THREAD_CHAMFER_CSS,
  THREAD_ZEN_SCOPE_ATTR,
  chamferCutFor,
  chamferVars,
  type ChamferCorners,
} from './chamfer'

/** The `--zen-*` tokens the shared pieces read, from the app's Style. */
export const THREAD_ZEN_DESKTOP_TOKENS = {
  '--zen-canvas': 'var(--color-bg)',
  '--zen-surface': 'var(--color-bg-surface)',
  '--zen-surface-raised': 'var(--color-bg-elevated)',
  '--zen-border': 'var(--color-border)',
  '--zen-text': 'var(--color-text-primary)',
  '--zen-text-muted': 'var(--color-text-muted)',
  '--zen-accent': 'var(--color-accent)',
  '--zen-accent-text': 'var(--color-on-accent)',
  '--zen-bubble-me': 'var(--color-accent)',
  '--zen-bubble-me-text': 'var(--color-on-accent)',
  '--zen-bubble-agent': 'var(--color-bg-elevated)',
  '--zen-bubble-agent-text': 'var(--color-text-primary)',
  '--zen-working': 'var(--color-status-working)',
  '--zen-idle': 'var(--color-text-muted)',
  '--zen-needs-you': 'var(--color-status-needs-you)',
  '--zen-ok': 'var(--color-status-ok)',
  '--zen-danger': 'var(--color-status-error)',
  // Zen's default shape (zen-tokens), not the Style's square corners.
  '--zen-radius': '14px',
  '--zen-bubble-radius': '18px',
  '--zen-gap': '12px',
  '--zen-line-height': '1.45',
} as CSSProperties

/** The Thread cards take their look from these (Styles values otherwise). */
export const THREAD_CARD_TOKENS = {
  '--thread-card-border': 'var(--zen-border)',
  '--thread-card-bg': 'var(--zen-surface)',
  '--thread-card-accent': 'var(--zen-accent)',
  '--thread-card-text': 'var(--zen-text)',
  '--thread-card-text-secondary': 'var(--zen-text)',
  '--thread-card-muted': 'var(--zen-text-muted)',
  '--thread-card-radius': 'calc(var(--zen-radius) / 2)',
} as CSSProperties

/** Where the shared rules apply: a Zen root, or the desktop Thread's box. */
const SCOPE = `:is([data-zen-root], [${THREAD_ZEN_SCOPE_ATTR}])`

const anim = (name: string, ms: number, ease = 'cubic-bezier(0.22, 1, 0.36, 1)'): string =>
  `var(--zen-anim-${name}-duration, ${ms}ms) var(--zen-anim-${name}-ease, ${ease})`

/** Motion and hover rules for the shared pieces (Zen and the desktop Thread). */
export const THREAD_ZEN_CSS = `
${SCOPE} [data-zen-message] { animation: zen-message-in ${anim('messageIn', 220)} both; }
${SCOPE} [data-zen-message][data-zen-message-still] { animation: none; }
${SCOPE} [data-zen-pulse] { animation: zen-pulse var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; }
${SCOPE} [data-zen-typing] span { animation: zen-typing var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; }
${SCOPE} [data-zen-typing] span:nth-child(2) { animation-delay: 160ms; }
${SCOPE} [data-zen-typing] span:nth-child(3) { animation-delay: 320ms; }
${SCOPE} [data-zen-soft-button] { transition: background-color ${anim('rowMove', 140)}, opacity ${anim('rowMove', 140)}; }
${SCOPE} [data-zen-soft-button]:hover:not(:disabled) { background: var(--zen-surface-raised); }
${SCOPE} [data-zen-soft-button]:disabled { opacity: 0.45; }
${SCOPE} [data-zen-compose-input]::placeholder { color: var(--zen-text-muted); opacity: 1; }
${SCOPE} [data-zen-compose-input]:focus { outline: none; }
[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-compose] [data-zen-soft-button]:hover:not(:disabled) { background: var(--zen-surface); }
[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-bubble="agent"] *::selection,
[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-bubble="agent"]::selection { background: color-mix(in srgb, var(--zen-accent) 30%, transparent); color: var(--zen-bubble-agent-text); }
[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-bubble="me"] *::selection,
[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-bubble="me"]::selection { background: color-mix(in srgb, var(--zen-bubble-me-text) 35%, transparent); color: var(--zen-bubble-me-text); }
@keyframes zen-message-in { from { opacity: 0; transform: translateY(6px) scale(0.98); } to { opacity: 1; transform: none; } }
@keyframes zen-pulse { 0%, 100% { opacity: 1; transform: scale(1); } 50% { opacity: 0.45; transform: scale(0.82); } }
@keyframes zen-typing { 0%, 80%, 100% { opacity: 0.25; transform: translateY(0); } 40% { opacity: 1; transform: translateY(-2px); } }
@media (prefers-reduced-motion: reduce) {
  ${SCOPE} [data-zen-message], ${SCOPE} [data-zen-pulse], ${SCOPE} [data-zen-typing] span { animation: none !important; transition: none !important; }
}
`

// ── Frosted glass behind the message box ────────────────────────────────
//
// Rosson 2026-10-16, from K2 Companion (`ZenFooterBlur`, 7cbf584 on main,
// after 4.0.0): "the area behind the text area is transparent so that
// messages can be visible below and above/slightly to the side of the ear
// as you scroll but blurred." The list runs under the whole bottom area
// (the column pads it by the bar's height, so the newest message still
// rests above the box). The glass covers the bar's slot from FOOTER_FADE_PX
// above whatever sits on top of the box (the activity drawer's tab while
// it is out, else the field, or the file chips) down to the bottom, full
// width: beside the tab and behind the field too. The field gets a
// see-through wash of its own fill so the blurred messages show through it
// while the text stays readable (stronger in light schemes). The tab keeps
// its own solid, darker fill (ZenActivityDrawer's ZEN_DRAWER_EAR_FILL;
// Rosson 2026-10-16), the same in every state of the box.
//
// A progressive blur (Rosson: "gentle at the top, heavy at the bottom"):
// four stacked backdrop-blur layers, light to heavy (1/4/10/20px), each
// masked to show from its own start down, each mask overlapping the one
// before so there are no bands. Over the glass's height: the top ~20% is
// barely blurred, the bottom ~40% fully heavy. A tint of the window's
// background (`--zen-canvas`) ramps the same way, nearly clear at the top.
// WebKit (the 2026-10-04 no-blur rule, suspended for this strip on the dev
// branch while Rosson judges it): no will-change, no animated blur radius,
// clicks pass through; only the glass's top moves with the tab. Prefixed
// and standard properties both. Reduced transparency, or no
// backdrop-filter: no blur, the field opaque, and an opaque-going
// gradient (px stops) behind them.

/** How far (px) the fade reaches above the topmost thing on the message
 *  box: the drawer's tab when it is out, else the field (Rosson: "like 4px
 *  above the status dog ear when it's out, or above the chat area", then
 *  "start the blur like 2px higher"). The blur and tint ramp from there
 *  down to the bottom (reduced transparency: its tint is near opaque by
 *  that element's top edge). */
export const FOOTER_FADE_PX = 6

/** What the strip's top follows, topmost first found wins by position:
 *  the file chips, the activity drawer's tab, the field. */
export const FOOTER_FADE_TARGETS = ['[data-zen-attachments]', '[data-zen-drawer-clip]', '[data-zen-compose-box]'] as const

/** The strip's `top` (px, relative to the bar's slot) from the slot's top
 *  and the tops of what sits on the box (viewport px): FOOTER_FADE_PX above
 *  the topmost. Null when there is nothing to follow. */
export function threadFadeTop(slotTop: number, targetTops: readonly number[]): number | null {
  if (targetTops.length === 0) return null
  return Math.round(Math.min(...targetTops) - slotTop - FOOTER_FADE_PX)
}

/** Follows the drawer sliding in and out (ZenActivityDrawer's 200ms). */
export const FOOTER_FADE_MOVE_MS = 200

const FOOTER_FADE_SCOPE = `[${THREAD_ZEN_SCOPE_ATTR}] .thread-footer-fade`

/** The frosted tint: nearly clear at the top, stronger toward the bottom
 *  ([% of the glass's height from its top, % of the background]). */
export const FOOTER_TINT_STOPS: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [20, 6],
  [45, 18],
  [70, 32],
  [100, 46],
]

/** Reduced transparency (no blur): the gradient goes nearly opaque within
 *  6px of the glass's top ([px from the top, % of the background]). */
export const FOOTER_TINT_STOPS_OPAQUE: ReadonlyArray<readonly [number, number]> = [
  [0, 0],
  [1, 20],
  [2, 44],
  [3, 66],
  [4, 84],
  [6, 95],
]

/** The blur layers, light to heavy: [blur px, mask clear until %, fully on
 *  at %] of the glass's height from its top. Each starts before the one
 *  above it is fully on, so the masks overlap. */
export const FOOTER_BLUR_LAYERS: ReadonlyArray<readonly [number, number, number]> = [
  [1, 0, 12],
  [4, 8, 35],
  [10, 28, 60],
  [20, 52, 80],
]

/** The field's see-through wash (% of its own fill), dark and light
 *  schemes; reduced transparency keeps it opaque. */
export const FOOTER_GLASS_WASH = { dark: 72, light: 86 } as const

const tintCss = (stops: ReadonlyArray<readonly [number, number]>, unit: '%' | 'px'): string =>
  `background: linear-gradient(to bottom, ${stops.map(
    ([at, pct]) => `color-mix(in srgb, var(--zen-canvas) ${pct}%, transparent) ${at}${unit}`,
  ).join(', ')});`

const blurLayerCss = ([px, from, to]: readonly [number, number, number], n: number): string => {
  const mask = `linear-gradient(to bottom, transparent ${from}%, #000 ${to}%)`
  return (
    `${FOOTER_FADE_SCOPE} > .thread-footer-blur-${n + 1} { -webkit-backdrop-filter: blur(${px}px); backdrop-filter: blur(${px}px); ` +
    `-webkit-mask-image: ${mask}; mask-image: ${mask}; }`
  )
}

/** The desktop bar's field (not Zen). Not the drawer's tab: it keeps its
 *  own fill, whatever the box does. */
const GLASS_TARGETS = [`[${THREAD_ZEN_SCOPE_ATTR}] [data-zen-compose] [data-zen-compose-box]`].join(', ')

/** A wash of the element's own fill at `pct`% (the square Style's chamfer
 *  layers read `--ch-fill`, the round ones `background`). */
const glassCss = (pct: number): string => {
  const fill = pct >= 100 ? 'var(--zen-surface-raised)' : `color-mix(in srgb, var(--zen-surface-raised) ${pct}%, transparent)`
  return `background: ${fill} !important; --ch-fill: ${fill} !important;`
}

export const THREAD_FOOTER_FADE_CSS = `
${FOOTER_FADE_SCOPE} { position: absolute; left: 0; right: 0; bottom: 0; top: -${FOOTER_FADE_PX}px; pointer-events: none; z-index: 0; transition: top ${FOOTER_FADE_MOVE_MS}ms cubic-bezier(0.2, 0.9, 0.3, 1); }
${FOOTER_FADE_SCOPE} > span { position: absolute; inset: 0; pointer-events: none; }
${FOOTER_BLUR_LAYERS.map(blurLayerCss).join('\n')}
${FOOTER_FADE_SCOPE} > .thread-footer-tint { ${tintCss(FOOTER_TINT_STOPS, '%')} }
${GLASS_TARGETS} { ${glassCss(FOOTER_GLASS_WASH.dark)} }
html[data-scheme="light"] ${GLASS_TARGETS.split(', ').join(', html[data-scheme="light"] ')} { ${glassCss(FOOTER_GLASS_WASH.light)} }
@media (prefers-reduced-motion: reduce) {
  ${FOOTER_FADE_SCOPE} { transition: none; }
}
@media (prefers-reduced-transparency: reduce) {
  ${FOOTER_FADE_SCOPE} > [class^="thread-footer-blur-"] { display: none; }
  ${FOOTER_FADE_SCOPE} > .thread-footer-tint { ${tintCss(FOOTER_TINT_STOPS_OPAQUE, 'px')} }
  ${GLASS_TARGETS}, html[data-scheme="light"] ${GLASS_TARGETS.split(', ').join(', html[data-scheme="light"] ')} { ${glassCss(100)} }
}
@supports not ((-webkit-backdrop-filter: blur(1px)) or (backdrop-filter: blur(1px))) {
  ${FOOTER_FADE_SCOPE} > [class^="thread-footer-blur-"] { display: none; }
  ${FOOTER_FADE_SCOPE} > .thread-footer-tint { ${tintCss(FOOTER_TINT_STOPS_OPAQUE, 'px')} }
  ${GLASS_TARGETS}, html[data-scheme="light"] ${GLASS_TARGETS.split(', ').join(', html[data-scheme="light"] ')} { ${glassCss(100)} }
}
`

/** The fade behind the message box: fills its (relative) box down to the
 *  bottom, from `top` (`threadFadeTop`; unmeasured, FOOTER_FADE_PX above
 *  the box's slot). Never takes clicks. */
export function ThreadFooterFade({ top }: { top?: number | null }): JSX.Element {
  return (
    <div
      className="thread-footer-fade"
      data-thread-footer-fade=""
      aria-hidden="true"
      style={top == null ? undefined : { top }}
    >
      {FOOTER_BLUR_LAYERS.map((_, i) => (
        <span key={i} className={`thread-footer-blur-${i + 1}`} />
      ))}
      <span className="thread-footer-tint" />
    </div>
  )
}

export function ThreadZenStyles(): JSX.Element {
  return <style data-thread-zen-styles="">{THREAD_ZEN_CSS + THREAD_CHAMFER_CSS + THREAD_FOOTER_FADE_CSS}</style>
}

/** The message list's padding and the space between messages (the
 *  desktop Thread, and the chats that wear its look: a ticket's discussion,
 *  the project chat). */
export const THREAD_LIST_PADDING = '14px var(--zen-gap) 8px'
export const THREAD_LIST_GAP = 10

/** The empty Thread's line (Zen and the desktop Thread). */
export const zenEmptyThreadText = (label: string): string => `Message ${label}. It reads this like a text.`

/** A bubble's text: the body, a choice card's prompt; a secret card shows
 *  the agent's prompt itself, as plain text (VS13). */
export function threadBubbleText(item: OverlayThreadItem): string {
  const d = item.doc
  // A permission card draws its own headline (the prompt is for old clients).
  if (d.kind === 'choice' && d.choice?.permission) return ''
  if (d.kind === 'choice') return d.choice?.prompt || d.body || ''
  if (d.kind === 'secret') return d.secret ? '' : d.body || ''
  // A handoff card draws itself; `body` is the plain line for old renderers.
  if (d.kind === 'handoff') return d.handoff ? '' : d.body || ''
  return d.body || ''
}

/** How a bubble's secret card draws. */
export interface ThreadBubbleSecret {
  /** Zen: the compact card with More options (prd-k2-vault-v1 KV9). */
  compact?: boolean
  onMoreOptions?: () => void
  workspace?: string
  server?: string
}

/** The bubble's body text style: the bubble's own colour and size. One
 *  object, so the memoized body (and its Markdown parse) skips re-renders. */
const BUBBLE_BODY_STYLE: CSSProperties = { color: 'inherit', fontSize: 'inherit' }

export interface ThreadBubbleProps {
  item: OverlayThreadItem
  /** The viewer's own post: right side, accent bubble. */
  mine: boolean
  /** "You", a person's name, the agent's address. */
  author: string
  timeLabel: string
  /** Who is looking: "answered by You" only on their own answers. */
  viewer: ThreadViewer | null
  nowSec?: number
  /** Resolves when the daemon took the answer; throws its refusal. */
  onAnswer?: (payload: ThreadAnswerPayload) => unknown
  onVoid?: () => void
  secret?: ThreadBubbleSecret
  /** A handoff card's decision (`/cli/vault/incoming/decide` on the
   *  card's own server: the room's, or a Garden row's feed). Absent: this
   *  view can't decide, and the card says to open the agent's server (no
   *  silent fallback to the window's server, accept cards from Home S2). */
  onHandoffDecide?: (body: HandoffDecideBody) => Promise<HandoffDecideResult | void>
  /** The room is on a server this window can't decide a handoff for (a
   *  view-only room): its name. The card then says "Open <host> directly
   *  to accept this" instead of Accept. A usable remote room decides on
   *  its own server (accept cards from Home, 0.46.6). */
  handoffRemoteHost?: string | null
  /** A line under "author · time", on the same side (the project chat's
   *  "delivered to …"). Absent (the Thread): nothing is drawn. */
  footer?: ReactNode
  /** Drawn again by the render window (useRenderWindow): no message-in
   *  animation, it was on screen before. */
  still?: boolean
}

function shallowSame(a: object | null | undefined, b: object | null | undefined): boolean {
  if (a === b) return true
  if (!a || !b) return false
  const ka = Object.keys(a)
  if (ka.length !== Object.keys(b).length) return false
  return ka.every((k) => Object.is((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]))
}

/** Two footers draw the same: one node, or two elements of one type with
 *  the same props (a parent re-render makes a new, equal element). */
export function sameChatNode(a: ReactNode, b: ReactNode): boolean {
  if (Object.is(a, b)) return true
  if (!isValidElement(a) || !isValidElement(b)) return false
  return a.type === b.type && a.key === b.key && shallowSame(a.props as object, b.props as object)
}

/** A bubble re-renders only when what it draws changed: the message by
 *  content (a re-read hands over new, equal objects), the rest by value;
 *  callbacks by identity (callers keep them stable). */
export function sameBubbleProps(a: ThreadBubbleProps, b: ThreadBubbleProps): boolean {
  return (
    sameThreadItem(a.item, b.item) &&
    a.mine === b.mine &&
    a.author === b.author &&
    a.timeLabel === b.timeLabel &&
    a.viewer === b.viewer &&
    a.nowSec === b.nowSec &&
    a.onAnswer === b.onAnswer &&
    a.onVoid === b.onVoid &&
    a.onHandoffDecide === b.onHandoffDecide &&
    a.handoffRemoteHost === b.handoffRemoteHost &&
    a.still === b.still &&
    shallowSame(a.secret, b.secret) &&
    sameChatNode(a.footer, b.footer)
  )
}

/** One message (memoized: `sameBubbleProps`). */
export const ThreadBubble = memo(function ThreadBubble({
  item,
  mine,
  author,
  timeLabel,
  viewer,
  onAnswer,
  onVoid,
  secret,
  nowSec,
  onHandoffDecide,
  handoffRemoteHost = null,
  footer,
  still = false,
}: ThreadBubbleProps): JSX.Element {
  const kind = item.doc.kind
  // A handoff doc as it arrived (a live frame, another server's row) is
  // coerced here like a snapshot row: no `hosts` must never crash a bubble.
  const handoff = useMemo(() => (kind === 'handoff' ? coerceHandoff(item.doc.handoff) : null), [kind, item.doc.handoff])
  const text = threadBubbleText(item)
  const files = item.doc.attachments ?? []
  // A post that is only files (`k2 thread <addr> --file …`) has no bubble.
  const bubble = !(files.length > 0 && !text && kind === 'text')
  // A narrow column (ThreadColumn): the whole column but a small inset.
  const narrow = useThreadColumnNarrow()
  const permissionActions = usePermissionActions(item, onAnswer)
  return (
    <div
      data-zen-message={item.id}
      data-testid="thread-item"
      data-kind={kind}
      data-seq={item.seq}
      data-status={item.doc.choice?.status || item.doc.secret?.status || item.doc.handoff?.status || ''}
      data-mine={mine ? '' : undefined}
      data-zen-message-still={still ? '' : undefined}
      className="flex flex-col"
      style={{ alignSelf: mine ? 'flex-end' : 'flex-start', maxWidth: threadBubbleMaxWidth(narrow), minWidth: 0, gap: 3 }}
    >
      {bubble && <div
        className="selectable-copy"
        data-zen-bubble={mine ? 'me' : 'agent'}
        data-chamfer=""
        style={{
          padding: '9px 14px',
          borderRadius: 'var(--zen-bubble-radius)',
          borderBottomRightRadius: mine ? 6 : 'var(--zen-bubble-radius)',
          borderBottomLeftRadius: mine ? 'var(--zen-bubble-radius)' : 6,
          background: mine ? 'var(--zen-bubble-me)' : 'var(--zen-bubble-agent)',
          color: mine ? 'var(--zen-bubble-me-text)' : 'var(--zen-bubble-agent-text)',
          minWidth: 0,
          overflowWrap: 'anywhere',
          // Markdown inside reads Styles variables; here they are this bubble's text.
          ...zenBubbleShield(mine ? 'me' : 'agent'),
          ...THREAD_CARD_TOKENS,
          // Square Style: cut corners; the tail corner stays sharp.
          ...chamferVars({
            cut: mine ? [4, 4, 0, 4] : [4, 4, 4, 0],
            fill: mine ? 'var(--zen-bubble-me)' : 'var(--zen-bubble-agent)',
          }),
        }}
      >
        {text && <ChatMessageBody text={text} style={BUBBLE_BODY_STYLE} />}
        {kind === 'choice' && item.doc.choice && (
          <ChoiceCard
            options={item.doc.choice.options}
            allowCustom={item.doc.choice.allow_custom}
            status={item.doc.choice.status}
            answer={item.doc.choice.answer}
            answeredBy={cardAnswererLabel(item.doc.choice.answeredBy, viewer)}
            expiresAt={item.doc.choice.expiresAt}
            nowSec={nowSec}
            resultMessage={item.doc.choice.resultMessage}
            permission={item.doc.choice.permission ?? null}
            permissionActions={permissionActions}
            from={item.doc.from || ''}
            onPick={(label) => {
              const warn = (err: unknown): void => console.warn('[thread] answer failed:', err)
              try {
                const sent = onAnswer?.({ answer: label })
                if (sent instanceof Promise) sent.catch(warn)
              } catch (err) {
                warn(err)
              }
            }}
          />
        )}
        {kind === 'secret' && item.doc.secret && (
          <div style={{ marginTop: 6 }}>
            <SecretCard
              compact={secret?.compact}
              secret={item.doc.secret}
              answeredBy={cardAnswererLabel(item.doc.secret.answeredBy, viewer)}
              agent={item.doc.from || ''}
              workspace={secret?.workspace}
              server={secret?.server}
              onSubmit={async (payload) => {
                await onAnswer?.(payload)
              }}
              onDismiss={() => onVoid?.()}
              onMoreOptions={secret?.onMoreOptions}
            />
          </div>
        )}
        {handoff && (
          <div style={{ marginTop: 6 }}>
            <HandoffCard
              handoff={handoff}
              answeredBy={cardAnswererLabel(handoff.answeredBy, viewer)}
              nowSec={nowSec}
              onDecide={onHandoffDecide ?? (() => Promise.reject(new Error('no server to decide on')))}
              remoteHost={onHandoffDecide ? handoffRemoteHost : (handoffRemoteHost ?? 'the agent’s server')}
            />
          </div>
        )}
      </div>}
      <ThreadFileObjects files={files} mine={mine} docId={item.id} />
      {/* thread-context-v1: a Garden widget's line that rode this post, marked as the widget's. */}
      {item.doc.context && <ThreadContextLine context={item.doc.context} mine={mine} />}
      <span
        data-zen-message-meta=""
        style={{
          alignSelf: mine ? 'flex-end' : 'flex-start',
          padding: '0 6px',
          fontSize: '0.72em',
          color: 'var(--zen-text-muted)',
        }}
      >
        <span data-thread-author="">{author}</span> · <span data-thread-time="">{timeLabel}</span>
      </span>
      {footer != null && (
        <span
          data-zen-message-footer=""
          style={{
            alignSelf: mine ? 'flex-end' : 'flex-start',
            padding: '0 6px',
            fontSize: '0.72em',
            color: 'var(--zen-text-muted)',
          }}
        >
          {footer}
        </span>
      )}
    </div>
  )
}, sameBubbleProps)

/** "Open in Settings" from a permission card. */
const openPermissionSettings = (section: string): void => {
  void openSettingsPage(section as SettingsSection).catch((err: unknown) => console.warn('[thread] open Settings failed:', err))
}

/**
 * A permission card's actions (prd-agent-settings-cards-v1 §8): on the
 * Thread's own server when the surface says which (the desktop Thread pane
 * and Zen's conversation: answer, note, undo, the confirm step, Open in
 * Settings); else through the surface's `onAnswer` (Allow / Deny only).
 * Null for every other card.
 */
function usePermissionActions(
  item: OverlayThreadItem,
  onAnswer: ThreadBubbleProps['onAnswer'],
): PermissionCardActions | null {
  const source = useThreadAttachmentSource()
  const isPermission = !!item.doc.choice?.permission
  return useMemo(() => {
    if (!isPermission) return null
    if (source?.addr) return permissionCardActions(source.scope, source.addr, item.id, openPermissionSettings)
    if (!onAnswer) return null
    return {
      answer: async (a) => {
        await onAnswer({
          answer: a.answer,
          ...(a.item ? { item: a.item } : {}),
          ...(a.reason ? { reason: a.reason } : {}),
          client: permissionClient(),
        })
      },
    }
  }, [isPermission, source, item.id, onAnswer])
}

/** Stands in for the rows the render window does not draw (useRenderWindow):
 *  their measured height. It takes back the list's `gap` with a negative
 *  margin, so the list is as tall as it was with the rows in it. */
export function ThreadWindowSpacer({ px, gap, edge }: { px: number; gap: number; edge: 'top' | 'bottom' }): JSX.Element | null {
  if (!(px > 0)) return null
  return (
    <div
      aria-hidden="true"
      data-win-spacer={edge}
      style={{ flex: 'none', height: px, ...(edge === 'top' ? { marginBottom: -gap } : { marginTop: -gap }) }}
    />
  )
}

/** Three dots under the last message while the agent works (no drawer). */
export function ThreadTypingIndicator(): JSX.Element {
  return (
    <div
      data-zen-typing=""
      aria-label="Working"
      className="flex items-center gap-1"
      style={{
        alignSelf: 'flex-start',
        padding: '10px 14px',
        borderRadius: 'var(--zen-bubble-radius)',
        background: 'var(--zen-bubble-agent)',
      }}
    >
      {[0, 1, 2].map((i) => (
        <span key={i} style={{ width: 6, height: 6, borderRadius: 999, background: 'var(--zen-working)', display: 'inline-block' }} />
      ))}
    </div>
  )
}

/** "Earlier messages", a soft pill at the top of the list. */
export function ThreadLoadOlder({
  loading,
  onClick,
  testId,
}: {
  loading: boolean
  onClick: () => void
  testId?: string
}): JSX.Element {
  return (
    <button
      className="cursor-pointer disabled:cursor-default"
      type="button"
      data-zen-load-older=""
      data-zen-soft-button=""
      {...ZEN_SOLID_PROPS}
      data-chamfer=""
      data-testid={testId}
      disabled={loading}
      onClick={onClick}
      style={{
        alignSelf: 'center',
        flex: 'none',
        padding: '3px 12px',
        borderRadius: 999,
        fontSize: '0.8em',
        color: 'var(--zen-text-muted)',
        // Square Style: no fill at rest (a hover fills it), cut like a 22px control.
        ...chamferVars({ cut: chamferCutFor(22), fill: 'transparent' }),
      }}
    >
      {loading ? 'Loading…' : 'Earlier messages'}
    </button>
  )
}

// ── The message box ─────────────────────────────────────────────────────

// ── Stacking past 3 lines ──
//
// Rosson 2026-10-16 ("really clean"): once the text wraps past 3 lines the
// box stacks: the text takes its own full-width line and the buttons sit
// on the line under it (Send on the right). Any box built from this kit
// can do it: pass `stacked` to ThreadComposeBox, give the textarea's slot
// `threadComposeTextSlotStyle(stacked)`, put `<ThreadComposeStackSpacer>`
// before Send, and update `stacked` with `threadComposeStackNext` whenever
// the textarea is re-measured. The textarea stays the same element (CSS
// order and wrap), so focus and the caret are kept.

/** Lines `el` wraps to: (scrollHeight - vertical padding) / line height. */
export function threadComposeLines(el: { value: string; scrollHeight: number }, lineHeightPx: number, padY: number): number {
  if (!el.value) return 1
  return Math.max(1, Math.round((el.scrollHeight - padY) / lineHeightPx))
}

/** Stack or not, from the lines at the textarea's current `width`.
 *  `rowChrome` is the width the one-row layout takes beside the textarea
 *  (the leading buttons, the box's gaps and Send). Up past 3 lines; down
 *  only when the text would take 2 lines or fewer back in the narrower
 *  one-row field, so it can't flicker (one row again means 3 lines or
 *  fewer there, which keeps it as it is). */
export function threadComposeStackNext(o: { stacked: boolean; lines: number; width: number; rowChrome: number }): boolean {
  if (!o.stacked) return o.lines > 3
  const narrow = o.width - o.rowChrome
  if (narrow <= 0) return true
  return Math.ceil((o.lines * o.width) / narrow) > 2
}

/** The textarea slot's extra style: stacked, first and full width. */
export function threadComposeTextSlotStyle(stacked: boolean): CSSProperties {
  return stacked ? { order: -1, flexBasis: '100%' } : {}
}

/** Before Send: stacked, pushes it to the right of the button line. */
export function ThreadComposeStackSpacer({ stacked }: { stacked: boolean }): JSX.Element | null {
  return stacked ? <div aria-hidden="true" data-thread-compose-spacer="" style={{ flex: 1 }} /> : null
}

/** The field's padding and border (px); a leading control's ink starts
 *  THREAD_COMPOSE_BOX_EDGE in from the field's outer edge plus its own. */
export const THREAD_COMPOSE_BOX_PAD = 6
export const THREAD_COMPOSE_BOX_BORDER = 1
export const THREAD_COMPOSE_BOX_EDGE = THREAD_COMPOSE_BOX_PAD + THREAD_COMPOSE_BOX_BORDER

/** The rounded field: leading buttons, the textarea, Send. A drawer (the
 *  activity drawer) is drawn on its top edge, chips above that. */
export function ThreadComposeBox({
  chips,
  drawer,
  children,
  error,
  stacked = false,
}: {
  chips?: ReactNode
  drawer?: ReactNode
  /** The leading buttons, the textarea and Send, in order. */
  children: ReactNode
  error?: ReactNode
  /** Wrap: the caller puts the textarea on its own first line (CSS order,
   *  no remount) and the buttons on the line under it. */
  stacked?: boolean
}): JSX.Element {
  return (
    <>
      {chips}
      {drawer}
      <div
        data-zen-compose-box=""
        {...ZEN_TILE_RAISED_PROPS}
        data-chamfer=""
        data-stacked={stacked ? 'true' : undefined}
        className="flex items-end gap-2"
        style={{
          position: 'relative',
          ...(stacked ? { flexWrap: 'wrap', rowGap: 4 } : {}),
          padding: THREAD_COMPOSE_BOX_PAD,
          borderRadius: 'calc(var(--zen-radius) + 4px)',
          background: 'var(--zen-surface-raised)',
          border: `${THREAD_COMPOSE_BOX_BORDER}px solid var(--zen-border)`,
          minWidth: 0,
          ...chamferVars({
            cut: CHAMFER_CUT.surface,
            fill: 'var(--zen-surface-raised)',
            edge: 'var(--zen-border)',
            border: THREAD_COMPOSE_BOX_BORDER,
          }),
        }}
      >
        {children}
      </div>
      {error}
    </>
  )
}

// ── The "+" lit while held or while its menu is open ──────────────────────
// K2 Companion `c29b6db` (Rosson 2026-10-18): the composer's "+" shows it's
// selected while pressed and while its attach menu is open, a soft glass
// fill with the glyph at full ink. The phone's fill is white at 20% and its
// glyph white, on a dark canvas. Here the same is the theme's text colour
// (`--zen-text`): white glass and a white glyph on dark Styles and Gardens,
// a soft dark wash and a dark glyph on light ones (no white on white).

/** The lit "+"'s fill: the text colour at 20% (white at 20% on dark). */
export const THREAD_PLUS_LIT_FILL = 'color-mix(in srgb, var(--zen-text) 20%, transparent)'
/** The lit "+"'s glyph: the text colour (white on dark), from muted. */
export const THREAD_PLUS_LIT_INK = 'var(--zen-text)'
/** The phone's 90ms ease-out (the hover fill keeps its own timing). */
export const THREAD_PLUS_LIT_TRANSITION = 'background-color 90ms ease-out, color 90ms ease-out, opacity 140ms ease'

/** Held between pointerdown and pointerup / cancel / leave (never while
 *  `disabled`). Mouse, pen and touch alike. */
export function usePointerHeld(disabled = false): {
  held: boolean
  handlers: {
    onPointerDown: () => void
    onPointerUp: () => void
    onPointerCancel: () => void
    onPointerLeave: () => void
  }
} {
  const [down, setDown] = useState(false)
  const release = useCallback(() => setDown(false), [])
  const press = useCallback(() => {
    if (!disabled) setDown(true)
  }, [disabled])
  return {
    held: down && !disabled,
    handlers: { onPointerDown: press, onPointerUp: release, onPointerCancel: release, onPointerLeave: release },
  }
}

/** A round soft button in the box ("+", the desktop's "/"). */
export function ThreadRoundButton({
  label,
  title,
  disabled,
  pressed,
  lightable = false,
  menuOpen = false,
  onClick,
  children,
  buttonRef,
  data,
  style,
}: {
  label: string
  title?: string
  disabled?: boolean
  /** On (a selected slash command): the accent fill. */
  pressed?: boolean
  /** The composer's "+": lit (THREAD_PLUS_LIT_*) while held, and while
   *  `menuOpen`. */
  lightable?: boolean
  /** Its menu is open (a "+" whose click opens one); lit while it is. */
  menuOpen?: boolean
  onClick: () => void
  children: ReactNode
  buttonRef?: Ref<HTMLButtonElement>
  /** Extra attributes (data-*, aria-*). */
  data?: Record<string, string | boolean | undefined>
  style?: CSSProperties
}): JSX.Element {
  const { held, handlers } = usePointerHeld(!!disabled || !lightable)
  const lit = lightable && !pressed && !disabled && (held || menuOpen)
  return (
    <button
      ref={buttonRef}
      type="button"
      aria-label={label}
      title={title ?? label}
      data-zen-soft-button={pressed ? undefined : ''}
      data-zen-lit={lit ? 'true' : undefined}
      disabled={disabled}
      onClick={onClick}
      {...(lightable ? handlers : {})}
      {...data}
      className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
      style={{
        width: 32,
        height: 32,
        borderRadius: 999,
        color: pressed ? 'var(--zen-accent-text)' : lit ? THREAD_PLUS_LIT_INK : 'var(--zen-text-muted)',
        background: pressed ? 'var(--zen-accent)' : lit ? THREAD_PLUS_LIT_FILL : undefined,
        ...(lightable ? { transition: THREAD_PLUS_LIT_TRANSITION } : {}),
        fontSize: 20,
        lineHeight: 1,
        ...style,
      }}
    >
      {children}
    </button>
  )
}

/** Send while pressed (K2 Companion `c29b6db`): a little brighter. */
export const THREAD_SEND_PRESSED_FILTER = 'brightness(1.5)'

/** The round Send with the up arrow; the accent while there is something to send. */
export function ThreadSendButton({
  canSend,
  onClick,
  label = 'Send',
  title = 'Send (Enter)',
  size = 32,
  testId,
}: {
  canSend: boolean
  onClick: () => void
  label?: string
  title?: string
  /** Its square side (px); the desktop Thread's box uses 26. */
  size?: number
  testId?: string
}): JSX.Element {
  // The square Style cut keeps the proportion of 5px at 32px (to 0.5px).
  const cut = chamferCutFor(size)
  // Pressed: brighter, only while it can send. Tracked in JS like the
  // phone's (a press that keeps the caret in the box skips `:active`).
  const { held, handlers } = usePointerHeld(!canSend)
  const lit = held && canSend
  return (
    <button
      type="button"
      aria-label={label}
      title={title}
      data-zen-send=""
      data-zen-pressed={lit ? 'true' : undefined}
      data-testid={testId}
      {...ZEN_TILE_RAISED_PROPS}
      data-chamfer=""
      disabled={!canSend}
      onClick={onClick}
      {...handlers}
      className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
      style={{
        width: size,
        height: size,
        borderRadius: 999,
        background: canSend ? 'var(--zen-accent)' : 'var(--zen-surface)',
        color: canSend ? 'var(--zen-accent-text)' : 'var(--zen-text-muted)',
        filter: lit ? THREAD_SEND_PRESSED_FILTER : undefined,
        transition: 'background-color 140ms ease, filter 90ms ease-out',
        ...chamferVars({ cut, fill: canSend ? 'var(--zen-accent)' : 'var(--zen-surface)' }),
      }}
    >
      <svg width="14" height="14" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
        <path d="M8 13V3" />
        <path d="M3.5 7.5 8 3l4.5 4.5" />
      </svg>
    </button>
  )
}

/** The chips row above the box. */
export function ThreadAttachmentChips({ children, testId }: { children: ReactNode; testId?: string }): JSX.Element {
  return (
    <div className="flex flex-wrap gap-1.5" style={{ marginBottom: 6 }} data-zen-attachments="" data-testid={testId}>
      {children}
    </div>
  )
}

/** One attached file: its name and ×. `leading` (a thumbnail) and `onOpen`
 *  (a click on the name) are the desktop's image preview. */
export function ThreadAttachmentChip({
  name,
  onRemove,
  leading,
  onOpen,
  title,
}: {
  name: string
  onRemove: () => void
  leading?: ReactNode
  onOpen?: () => void
  title?: string
}): JSX.Element {
  return (
    <span
      data-zen-attachment={name}
      {...ZEN_SOLID_PROPS}
      data-chamfer=""
      title={title}
      className="inline-flex max-w-[16rem] items-center gap-1.5"
      style={{
        fontSize: '0.8em',
        padding: leading ? '3px 4px 3px 4px' : '3px 4px 3px 10px',
        borderRadius: 999,
        background: 'var(--zen-surface-raised)',
        border: '1px solid var(--zen-border)',
        color: 'var(--zen-text)',
        ...chamferVars({ cut: chamferCutFor(24), fill: 'var(--zen-surface-raised)', edge: 'var(--zen-border)', border: 1 }),
      }}
    >
      {leading}
      {onOpen ? (
        <button
          type="button"
          aria-label={`View ${name}`}
          onClick={onOpen}
          className="min-w-0 truncate cursor-zoom-in"
          style={{ color: 'inherit', background: 'transparent', border: 'none', padding: 0, font: 'inherit' }}
        >
          {name}
        </button>
      ) : (
        <span className="truncate">{name}</span>
      )}
      <button
        type="button"
        aria-label={`Remove ${name}`}
        data-zen-soft-button=""
        onClick={(e) => {
          e.stopPropagation()
          onRemove()
        }}
        className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
        style={{ width: 18, height: 18, borderRadius: 999, color: 'var(--zen-text-muted)' }}
      >
        ×
      </button>
    </span>
  )
}
