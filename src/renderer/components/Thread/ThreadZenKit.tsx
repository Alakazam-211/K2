// The Thread's look, shared by the Zen Thread widget (Gardens, from K2
// Companion) and the desktop Thread beside an agent (Rosson 2026-10-16:
// "update the thread UI in K2 to match what we have in mobile/the zen
// garden"). One set of pieces, so the two stay in sync:
//
//   - `ThreadBubble`: a message as a text bubble (mine right, others left),
//     the choice and secret cards inside it. Where the Thread knows its runs
//     (threadRuns: the desktop Thread, the project chat, a ticket's chat),
//     as on the phone: "name • time" over the first bubble of another
//     sender's run, and under the last of yours one status line (the time,
//     Sending…, Not sent, or the receipt). Elsewhere (Gardens): "author ·
//     time" under each.
//   - `ThreadTypingIndicator`, `ThreadLoadOlder`, `ThreadEmptyNote`.
//   - The message box parts (`ThreadComposeBox`, `ThreadRoundButton`,
//     `ThreadSendButton`, `ThreadAttachmentChip`).
//   - `THREAD_ZEN_CSS`: motion and hover rules for all of the above.
//
// Everything paints with `--zen-*` tokens. Inside Zen the theme engine sets
// them; the desktop Thread sets them from the app's Style
// (`THREAD_ZEN_DESKTOP_TOKENS` on a `[data-k2-thread-zen]` box), so the
// shapes are Zen's and the colours are the window's.

import { isValidElement, memo, useCallback, useMemo, useRef, useState, type CSSProperties, type JSX, type ReactNode, type Ref } from 'react'
import { ChatMessageBody } from '@/components/common/ChatMessage'
import { ChoiceCard } from '@/components/SessionView/ChoiceCard'
import { SecretCard } from '@/components/SessionView/SecretCard'
import { HandoffCard } from '@/components/SessionView/HandoffCard'
import { ChangesCard } from '@/components/SessionView/ChangesCard'
import type { HandoffDecideBody, HandoffDecideResult } from '@/components/Settings/sections/handoff-api'
import { coerceChanges, coerceHandoff, sameThreadItem, type OverlayStashOffer, type OverlayThreadItem, type ThreadAnswerPayload } from '@/components/SessionView/overlayThread'
import { cardAnswererLabel, type ThreadViewer } from '@/lib/thread-viewer'
import { daemonErrorText } from '@/lib/daemon-error'
import { zenBubbleShield } from '@/lib/zen/zen-style-shield'
import { ZEN_SOLID_PROPS, ZEN_TILE_RAISED_PROPS } from '@/lib/zen/zen-materials'
import { ThreadContextLine } from './ThreadContextChip'
import { ThreadFileObjects } from './ThreadFileObjects'
import { THREAD_SENDER_CSS, ThreadSenderFrame, sameSenderSlot, threadSenderRootProps, type ThreadSenderSlot } from './threadSenders'
import { useThreadAttachmentSource } from './ThreadImageObjects'
import { RECEIPT_LABEL, type ThreadReceipt } from './threadReceipt'
import {
  permissionCardActions,
  permissionClient,
  type PermissionCardActions,
} from '@/components/SessionView/permissionCardView'
import { openSettingsPage } from '@/lib/open-settings-page'
import type { SettingsSection } from '@/stores/settings'
import { threadBubbleMaxWidth, useThreadColumnNarrow } from './ThreadColumn'
import { StashOfferCard } from '@/components/PromptStash/StashFromThread'
import { stashOfferView } from '@/lib/prompt-stash/record'
import { parseThreadReply, threadReplyKey } from '@/lib/thread-reply'
import {
  ThreadMessageMoreButton,
  ThreadReplyQuote,
  openThreadMessageMenu,
  useThreadMessageGestures,
  type ThreadMessageMenuArgs,
} from './ThreadMessageMenu'

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
  '--zen-surface-raised': 'var(--color-thread-raised, var(--color-bg-elevated))',
  '--zen-border': 'var(--color-border)',
  '--zen-text': 'var(--color-text-primary)',
  '--zen-text-muted': 'var(--color-text-muted)',
  '--zen-accent': 'var(--color-accent)',
  '--zen-accent-text': 'var(--color-on-accent)',
  '--zen-bubble-me': 'var(--color-accent)',
  '--zen-bubble-me-text': 'var(--color-on-accent)',
  '--zen-bubble-agent': 'var(--color-bubble-agent, var(--color-bg-elevated))',
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

/** Where the bubble tail applies: the desktop Thread box under Glass. Never
 *  under Terminal (its bubbles are chamfered, CHAMFER_SCOPE) nor in a Garden
 *  (`[data-zen-root]` themes itself). */
export const THREAD_TAIL_SCOPE = `html[data-style="glass"] [${THREAD_ZEN_SCOPE_ATTR}]`

/** The tail's shape (K2 Companion, chatBubbles.css): a 20×20 box, the
 *  bubble's 14px corner block on the left, the curl out to the right. */
const TAIL_PATH = 'M0 0H14V8C14 13 16 17 20 19.6C15.5 20.2 11.5 18.6 9 16.2C7 18.5 3.5 20 0 20Z'
const TAIL_MASK = `url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 20 20'%3E%3Cpath d='${TAIL_PATH}'/%3E%3C/svg%3E") center / 100% 100% no-repeat`

/**
 * Glass's message tail (K2 Companion's iMessage tail, 4.2.2), on the last
 * bubble of each run (`ThreadBubble`'s `tail`, threadRuns):
 *
 *   - A run's other bubbles round all four corners (as on the phone).
 *   - The tail is the bubble's own `::after`, filled with its colour
 *     (`--zen-tail-fill`), shaped by a vector mask. Inside the bubble's
 *     box, so the "Sending…" dim (the row's opacity) covers bubble and
 *     tail as one, and a `drop-shadow` ring would trace both.
 *   - The bubble is its own stacking layer and the tail sits at z -1: over
 *     the bubble's fill, under its words (the tail's corner block is inside
 *     the bubble and would cover the last line's first letter).
 *   - The corner under the tail rounds at 14px, the tail's base, so no
 *     sliver of the page shows where the two meet.
 *   - The bubble never clips (the tail hangs 6px outside it).
 */
export const THREAD_TAIL_CSS = `
${THREAD_TAIL_SCOPE} [data-zen-tail] { border-bottom-left-radius: var(--zen-bubble-radius) !important; border-bottom-right-radius: var(--zen-bubble-radius) !important; }
${THREAD_TAIL_SCOPE} :is([data-zen-tail="mine"], [data-zen-tail="theirs"]) { position: relative; isolation: isolate; }
${THREAD_TAIL_SCOPE} [data-zen-tail="mine"] { --zen-tail-fill: var(--zen-bubble-me); border-bottom-right-radius: 14px !important; }
${THREAD_TAIL_SCOPE} [data-zen-tail="theirs"] { --zen-tail-fill: var(--zen-bubble-agent); border-bottom-left-radius: 14px !important; }
${THREAD_TAIL_SCOPE} :is([data-zen-tail="mine"], [data-zen-tail="theirs"])::after { content: ''; position: absolute; bottom: 0; width: 20px; height: 20px; z-index: -1; background: var(--zen-tail-fill); pointer-events: none; -webkit-mask: ${TAIL_MASK}; mask: ${TAIL_MASK}; }
${THREAD_TAIL_SCOPE} [data-zen-tail="mine"]::after { right: -6px; }
${THREAD_TAIL_SCOPE} [data-zen-tail="theirs"]::after { left: -6px; transform: scaleX(-1); }
`

/** Motion and hover rules for the shared pieces (Zen and the desktop Thread). */
export const THREAD_ZEN_CSS = `
${SCOPE} [data-zen-message] { animation: zen-message-in ${anim('messageIn', 220)} both; }
${SCOPE} [data-zen-message][data-zen-message-still] { animation: none; }
${SCOPE} [data-zen-pulse] { animation: zen-pulse var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; will-change: opacity, transform; }
${SCOPE} [data-zen-typing] span { animation: zen-typing var(--zen-anim-workingPulse-duration, 1400ms) var(--zen-anim-workingPulse-ease, ease-in-out) infinite; }
${SCOPE} [data-zen-typing] span:nth-child(2) { animation-delay: 160ms; }
${SCOPE} [data-zen-typing] span:nth-child(3) { animation-delay: 320ms; }
${SCOPE} [data-zen-soft-button] { transition: background-color ${anim('rowMove', 140)}, opacity ${anim('rowMove', 140)}; }
${SCOPE} [data-zen-soft-button]:hover:not(:disabled) { background: var(--zen-surface-raised); }
${SCOPE} [data-zen-soft-button]:disabled { opacity: 0.45; }
${SCOPE} [data-zen-compose-input]::placeholder { color: var(--zen-text-muted); opacity: 1; }
${SCOPE} [data-zen-compose-input]:focus { outline: none; }
${SCOPE} [data-zen-message-more] { opacity: 0; transition: opacity 120ms ease; }
${SCOPE} [data-zen-message]:hover [data-zen-message-more], ${SCOPE} [data-zen-message-more]:focus-visible { opacity: 1; }
${SCOPE} [data-thread-hover-status] { opacity: 0; pointer-events: none; transition: opacity 120ms ease; }
${SCOPE} [data-zen-message]:hover [data-thread-hover-status], ${SCOPE} [data-thread-hover-status]:focus-within { opacity: 1; pointer-events: auto; }
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
  ${SCOPE} [data-thread-hover-status] { transition: none !important; }
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
 *  the file chips, the activity drawer's tab, the stash ear (and the open
 *  stash) on the box's top edge, the field. */
export const FOOTER_FADE_TARGETS = ['[data-zen-attachments]', '[data-zen-drawer-clip]', '[data-stash-tab]', '[data-stash-drawer]', '[data-zen-compose-box]'] as const

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
  return <style data-thread-zen-styles="">{THREAD_ZEN_CSS + THREAD_CHAMFER_CSS + THREAD_TAIL_CSS + THREAD_FOOTER_FADE_CSS + THREAD_SENDER_CSS}</style>
}

/** The message list's padding and the space between messages (the
 *  desktop Thread, and the chats that wear its look: a ticket's discussion,
 *  the project chat). */
export const THREAD_LIST_PADDING = '14px var(--zen-gap) 8px'
export const THREAD_LIST_GAP = 10
/** The space between two bubbles of one run (the phone's: 4px in a run,
 *  10px between runs). A run's later rows take the difference back. */
export const THREAD_RUN_GAP = 4

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
  // A stash offer from an agent draws its own card; `body` is the fallback
  // for old clients, and what a person's (refused) offer shows.
  if (d.kind === 'stash.offer') return threadStashOffer(item) ? '' : d.body || ''
  // A changes card (workbench-v1 B7) draws itself; `body` is for old renderers.
  if (d.kind === 'changes') return coerceChanges(d.changes) ? '' : d.body || ''
  return d.body || ''
}

/** A `stash.offer` card's payload, only when an agent sent it (a sender
 *  address, no person's author stamp, not composed by a person; K2
 *  Companion's rule). Anything else draws as a plain message, no button. */
export function threadStashOffer(item: OverlayThreadItem): OverlayStashOffer | null {
  const d = item.doc
  if (d.kind !== 'stash.offer' || !d.stash) return null
  if (!d.from?.trim() || !isAgentPost(item)) return null
  return d.stash
}

/** An agent's post (not a person's: no daemon author stamp, not compose). */
function isAgentPost(item: OverlayThreadItem): boolean {
  return !item.doc.author && item.doc.via !== 'compose'
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
   *  "delivered to …"; a failed send's Retry · Discard). With runs, on
   *  your own message it joins the status line. Absent: nothing is drawn. */
  footer?: ReactNode
  /** Drawn again by the render window (useRenderWindow): no message-in
   *  animation, it was on screen before. */
  still?: boolean
  /** This window's send the server hasn't confirmed (remote Thread load
   *  Fix 3): `sending` draws a little faded, `failed` as usual (its footer
   *  says what to do). No message menu on either. */
  pending?: 'sending' | 'failed'
  /** The sender column (threadSenders, more than two people in the
   *  Thread): another sender's picture on the left. Absent: none. */
  sender?: ThreadSenderSlot | null
  /** Where the Thread knows its runs (threadRuns, the desktop Thread):
   *  true on the last bubble of a run, which draws a tail in Glass
   *  (THREAD_TAIL_CSS); false on the others, which round every corner.
   *  Absent (Gardens, the chats without runs): the bubble as before. */
  tail?: boolean
  /** Where the Thread knows its runs (threadRuns): true on the first
   *  bubble of a run. Given, the details go where the phone puts them:
   *  another sender's name and time over the first of their run, your
   *  status line under the last of yours (`tail`) or a pending send; none
   *  under the bubbles between. Absent (Gardens): "author · time" under
   *  every bubble, as before. */
  runStart?: boolean
  /** The Thread's one receipt (threadReceipt), on your newest confirmed
   *  message: its status line says it instead of the time. */
  receipt?: ThreadReceipt | null
}

/** The name line over another sender's run, and your status line under
 *  yours (K2 Companion's: 11px, muted; a 16px status line). */
const META_TYPE: CSSProperties = {
  fontSize: 'var(--type-sm)',
  lineHeight: '16px',
  color: 'var(--zen-text-muted)',
}
const META_NAME_STYLE: CSSProperties = {
  ...META_TYPE,
  display: 'flex',
  alignItems: 'baseline',
  gap: 6,
  minWidth: 0,
  maxWidth: '100%',
  // The bubble's text edge (its 14px padding), as the phone lines them up.
  padding: '0 14px',
}
const META_NAME_TEXT: CSSProperties = { minWidth: 0, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }
const META_STATUS_STYLE: CSSProperties = {
  ...META_TYPE,
  position: 'relative',
  alignSelf: 'flex-end',
  minHeight: 16,
  padding: '0 10px',
  textAlign: 'right',
}
/** The hover status line's corners (the Thread's small radius). */
const THREAD_HOVER_STATUS_RADIUS = 'calc(var(--zen-radius) / 2)'
/** Yours: the menu's button at the status line's inner end (before the
 *  time or receipt, pointing back into the Thread; Rosson 2026-11-09).
 *  Out of the flow, so the words never move when it shows. */
const MORE_STATUS_SLOT: CSSProperties = {
  position: 'absolute',
  right: 'calc(100% - 8px)',
  top: 0,
  height: 16,
  display: 'flex',
  alignItems: 'center',
}
const HOVER_MORE_SLOT: CSSProperties = { display: 'flex', alignItems: 'center', marginRight: 2 }
/** A bubble of yours with no status line (inside a run): hovering it (or
 *  the menu's button taking focus) shows one with its time and the
 *  button, laid over the space below, so nothing below moves. The
 *  canvas behind it keeps it readable over the next bubble. */
const HOVER_STATUS_STYLE: CSSProperties = {
  ...META_STATUS_STYLE,
  // As short as the words, so it covers as little of the next bubble's
  // top padding as it can.
  lineHeight: '14px',
  minHeight: 14,
  display: 'flex',
  alignItems: 'center',
  padding: '0 10px 0 2px',
  position: 'absolute',
  top: '100%',
  right: 0,
  zIndex: 2,
  whiteSpace: 'nowrap',
  background: 'var(--zen-canvas)',
  borderRadius: THREAD_HOVER_STATUS_RADIUS,
  userSelect: 'none',
}
/** Another sender's message menu button beside the bubble (runs: no line
 *  under each bubble to hold it): on the open side, level with the
 *  bubble's last line, shown on hover (THREAD_ZEN_CSS). */
const MORE_SLOT_BASE: CSSProperties = {
  position: 'absolute',
  bottom: 0,
  height: 'calc(var(--zen-line-height) * 1em + 18px)',
  display: 'flex',
  alignItems: 'center',
  color: 'var(--zen-text-muted)',
  fontSize: 'var(--type-sm)',
}
const MORE_SLOT_THEIRS: CSSProperties = { ...MORE_SLOT_BASE, left: '100%' }
/** Holds the bubble with runs: the picture and the menu's button sit
 *  against the bubble itself, not the name line over it. */
const BUBBLE_ROW_STYLE: CSSProperties = { position: 'relative', display: 'flex', flexDirection: 'column', minWidth: 0 }

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
    a.pending === b.pending &&
    a.tail === b.tail &&
    a.runStart === b.runStart &&
    (a.receipt ?? null) === (b.receipt ?? null) &&
    sameSenderSlot(a.sender, b.sender) &&
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
  pending,
  sender = null,
  tail,
  runStart,
  receipt = null,
}: ThreadBubbleProps): JSX.Element {
  const kind = item.doc.kind
  // A handoff doc as it arrived (a live frame, another server's row) is
  // coerced here like a snapshot row: no `hosts` must never crash a bubble.
  const handoff = useMemo(() => (kind === 'handoff' ? coerceHandoff(item.doc.handoff) : null), [kind, item.doc.handoff])
  const changes = useMemo(() => (kind === 'changes' ? coerceChanges(item.doc.changes) : null), [kind, item.doc.changes])
  const text = threadBubbleText(item)
  const files = item.doc.attachments ?? []
  // A post that is only files (`k2 thread <addr> --file …`) has no bubble.
  const bubble = !(files.length > 0 && !text && kind === 'text')
  // A narrow column (ThreadColumn): the whole column but a small inset.
  const narrow = useThreadColumnNarrow()
  const permissionActions = usePermissionActions(item, onAnswer)
  // Why the last tap on this choice card didn't count (shown on the card).
  const [pickError, setPickError] = useState<string | null>(null)
  // A sent reply (prd-thread-message-menu-v1): its quote draws as a small
  // line above the person's words.
  const reply = useMemo(() => (kind === 'text' ? parseThreadReply(text) : null), [kind, text])
  const bodyText = reply ? reply.rest : text
  // The message menu (prd-thread-message-menu-v1: Reply, Reply to
  // selection, Copy, Save to stash), where the Thread knows its server (the
  // desktop Thread, a Zen conversation). Never on a secret card.
  const source = useThreadAttachmentSource()
  const offer = threadStashOffer(item)
  const menuText = offer ? stashOfferView(offer).saved : bodyText
  const menuOn = !pending && !!source && kind !== 'secret' && (menuText.trim().length > 0 || !!reply)
  const fromLabel = mine ? item.doc.from?.trim() || 'you' : author.split(' · via ')[0]
  const replyAddr = source?.addr ?? ''
  const menuArgs = useMemo<ThreadMessageMenuArgs>(
    () => ({
      scope: source?.scope ?? null,
      text: menuText,
      // Stash source: who sent it; on your own message, the Thread's agent.
      agent: (mine ? replyAddr : item.doc.from) || null,
      agentName: mine ? null : author,
      quote: reply ? { text: reply.quote, from: reply.from } : null,
      reply:
        source && replyAddr
          ? {
              key: threadReplyKey(source.scope, replyAddr),
              target: { id: item.id, seq: item.seq, from: fromLabel, addr: replyAddr, text: menuText, part: false },
            }
          : null,
    }),
    [source, replyAddr, menuText, mine, item.doc.from, item.id, item.seq, author, fromLabel, reply],
  )
  const bubbleRef = useRef<HTMLDivElement | null>(null)
  const openMenu = useCallback(
    (x: number, y: number, selection: string | null) => void openThreadMessageMenu(x, y, menuArgs, selection),
    [menuArgs],
  )
  const gestures = useThreadMessageGestures(bubbleRef, menuOn ? openMenu : null)
  const maxWidth = threadBubbleMaxWidth(narrow)
  const column = threadSenderRootProps(mine ? null : sender, maxWidth, narrow)
  // Runs (the phone's layout): which details this bubble carries.
  const runs = runStart !== undefined
  const nameAbove = runs && !mine && runStart
  const statusBelow = runs && mine && (!!tail || pending != null || footer != null)
  const hoverStatus = runs && mine && !statusBelow
  // "You · via Notes": the widget part stays on your status line.
  const via = mine ? author.split(' · via ')[1] : undefined
  const shownReceipt = statusBelow && !pending && receipt ? receipt : null
  // (A post that is only files has no words to act on: no menu, no bubble.)
  const more = menuOn ? <ThreadMessageMoreButton onOpen={(x, y) => openMenu(x, y, null)} /> : null
  const bubbleNode = bubble ? (
    <div
      ref={bubbleRef}
      className="selectable-copy"
      {...(menuOn ? gestures : {})}
      data-zen-bubble={mine ? 'me' : 'agent'}
      data-zen-tail={tail === undefined ? undefined : tail ? (mine ? 'mine' : 'theirs') : 'none'}
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
        // Terminal Style: cut corners; the tail corner stays sharp.
        ...chamferVars({
          cut: mine ? [CHAMFER_CUT.surface, CHAMFER_CUT.surface, 0, CHAMFER_CUT.surface] : [CHAMFER_CUT.surface, CHAMFER_CUT.surface, CHAMFER_CUT.surface, 0],
          fill: mine ? 'var(--zen-bubble-me)' : 'var(--zen-bubble-agent)',
        }),
      }}
    >
      {reply && <ThreadReplyQuote reply={reply} />}
      {bodyText && <ChatMessageBody text={bodyText} style={BUBBLE_BODY_STYLE} />}
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
          openPinForm={item.doc.choice.openPinForm}
          pinScope={permissionActions?.confirmScope}
          permission={item.doc.choice.permission ?? null}
          permissionActions={permissionActions}
          from={item.doc.from || ''}
          error={pickError}
          onPick={(label) => {
            // A refusal (not the owner, already answered, the folder
            // changed) is shown on the card in the daemon's words: a tap
            // that does nothing visible reads as broken (2026-11-05).
            const shown = (err: unknown): void => {
              console.warn('[thread] answer failed:', err)
              setPickError(daemonErrorText(err) || 'K2 couldn’t record that answer.')
            }
            setPickError(null)
            try {
              const sent = onAnswer?.({ answer: label })
              if (sent instanceof Promise) sent.catch(shown)
              else if (!onAnswer) setPickError('This view can’t answer cards.')
            } catch (err) {
              shown(err)
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
      {offer && (
        <StashOfferCard docId={item.id} offer={offer} scope={source?.scope ?? null} agent={item.doc.from || null} agentName={author} />
      )}
      {changes && <ChangesCard changes={changes} />}
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
    </div>
  ) : null
  return (
    <div
      data-zen-message={item.id}
      data-testid="thread-item"
      data-kind={kind}
      data-seq={item.seq}
      data-status={item.doc.choice?.status || item.doc.secret?.status || item.doc.handoff?.status || ''}
      data-mine={mine ? '' : undefined}
      data-zen-message-still={still ? '' : undefined}
      data-pending={pending}
      {...column.attrs}
      className={column.className ? `flex flex-col ${column.className}` : 'flex flex-col'}
      style={{
        alignSelf: mine ? 'flex-end' : 'flex-start',
        maxWidth,
        minWidth: 0,
        gap: 3,
        ...(runs && !runStart ? { marginTop: THREAD_RUN_GAP - THREAD_LIST_GAP } : {}),
        ...(hoverStatus ? { position: 'relative' } : {}),
        ...(pending === 'sending' ? { opacity: 0.6 } : {}),
        ...column.style,
      }}
      data-run={runs ? (runStart ? (tail ? 'solo' : 'first') : tail ? 'last' : 'middle') : undefined}
    >
      {/* Top-left, level with the name line (the phone's place). */}
      {!mine && sender ? <ThreadSenderFrame slot={sender} /> : null}
      {nameAbove && (
        <span data-zen-message-meta="" data-thread-meta="name" style={META_NAME_STYLE}>
          <span data-thread-author="" style={META_NAME_TEXT}>
            {author}
          </span>
          <span aria-hidden="true">•</span>
          <span data-thread-time="" style={{ flexShrink: 0 }}>
            {timeLabel}
          </span>
        </span>
      )}
      {bubble && (runs && !mine ? (
        <div data-zen-bubble-row="" style={BUBBLE_ROW_STYLE}>
          {bubbleNode}
          {more && <span data-zen-message-more-slot="" style={MORE_SLOT_THEIRS}>{more}</span>}
        </div>
      ) : (
        bubbleNode
      ))}
      <ThreadFileObjects files={files} mine={mine} docId={item.id} />
      {/* thread-context-v1: a Garden widget's line that rode this post, marked as the widget's. */}
      {item.doc.context && <ThreadContextLine context={item.doc.context} mine={mine} />}
      {statusBelow && (
        <span
          data-zen-message-meta=""
          data-thread-meta="status"
          data-send-status={pending ?? 'sent'}
          data-receipt={shownReceipt ?? undefined}
          style={META_STATUS_STYLE}
        >
          {more && <span data-zen-message-more-slot="" style={MORE_STATUS_SLOT}>{more}</span>}
          {via ? <>via {via} · </> : null}
          {shownReceipt ? (
            <span data-thread-receipt="" title={timeLabel}>
              {RECEIPT_LABEL[shownReceipt]}
            </span>
          ) : (
            <span data-thread-time="">{timeLabel}</span>
          )}
          {footer != null && (
            <>
              {' · '}
              <span data-zen-message-footer="">{footer}</span>
            </>
          )}
        </span>
      )}
      {hoverStatus && more && (
        <span data-thread-meta="status-hover" data-thread-hover-status="" style={HOVER_STATUS_STYLE}>
          {/* Inside the line here (it has its own background). */}
          <span data-zen-message-more-slot="" style={HOVER_MORE_SLOT}>{more}</span>
          {via ? <>via {via} · </> : null}
          <span data-thread-time="">{timeLabel}</span>
        </span>
      )}
      {!runs && (
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
          {more}
        </span>
      )}
      {footer != null && !(runs && mine) && (
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
        // Terminal Style: no fill at rest (a hover fills it), cut like a 22px control.
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
/** The message box's tool buttons' ("+", "/", the stash) corner radius
 *  outside the square Style: a rounded square, the cards' radius. */
export const THREAD_TOOL_RADIUS = 'calc(var(--zen-radius) / 2)'

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
      // Its backdrop (hover, held, on) is a square (Rosson 2026-10-31: never
      // a circle or an oval): the square Style's chamfer; elsewhere a
      // rounded square, the cards' radius.
      data-chamfer=""
      className="flex flex-shrink-0 items-center justify-center cursor-pointer disabled:cursor-default"
      style={{
        width: 32,
        height: 32,
        borderRadius: THREAD_TOOL_RADIUS,
        color: pressed ? 'var(--zen-accent-text)' : lit ? THREAD_PLUS_LIT_INK : 'var(--zen-text-muted)',
        background: pressed ? 'var(--zen-accent)' : lit ? THREAD_PLUS_LIT_FILL : undefined,
        ...chamferVars({
          cut: CHAMFER_CUT.button,
          fill: pressed ? 'var(--zen-accent)' : lit ? THREAD_PLUS_LIT_FILL : 'transparent',
        }),
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
  // The square Style cut for a control this size.
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
