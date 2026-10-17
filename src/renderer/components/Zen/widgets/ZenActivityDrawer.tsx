// Garden 1's activity drawer (0.45.6, Rosson 2026-10-15; ported from K2
// Companion's `ThreadWorkingStrip.tsx` + `threadDrawer.css`). A tile pulled
// out of the message box's top edge while the agent works on your message,
// so what it is doing stays in view while you read back. Collapsed it is one
// line (pulse · step · icon counters · clock); a click grows it upward to
// the full detail in words, another click shrinks it back. The choice is
// remembered on this computer. Nothing else is stored.
//
// It is the desktop Thread working strip's words (`SessionView/threadStrip`,
// shipped 0.45.0: the daemon decides, the renderer only labels), drawn the
// phone's way. It counts its own clock (1 Hz, from the daemon's `startedAt`
// / `phaseSince`, corrected by `serverNow`), so the conversation around it
// doesn't re-render every second. The Conversation widget draws it with its
// `activity-drawer` prop (on in Garden 1); off, the three dots stay.
//
// No Stop in a Garden: it can't type into the agent's session. The desktop
// Thread draws this same drawer on its message box (2026-10-16, "match
// mobile/the zen garden") and passes `onStop`: the Thread working strip's
// Stop, which sends Esc to the agent's PTY, shows while the turn can be
// stopped (`threadStripView(...).stoppable`, the strip's own rule).

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import type { ThreadTurn } from '@/components/SessionView/overlayThread'
import { CHAMFER_CUT, THREAD_ZEN_SCOPE_ATTR, chamferCutFor, chamferVars } from '@/components/Thread/chamfer'
import { WorkingGrid } from '@/components/AgentStatus/StatusGlyphs'
import { ZEN_TILE_RAISED_PROPS } from '@/lib/zen/zen-materials'
import {
  COUNTER_CHECK_PX,
  COUNTER_GAP_PX,
  COUNTER_GROUP_GAP_PX,
  COUNTER_ICON_GAP_PX,
  COUNTER_ICON_PX,
  COUNTER_SPINNER_COLS,
  COUNTER_SPINNER_EM,
  COUNTER_SPINNER_GAP_PX,
  COUNTER_SPINNER_LEAD_PX,
  COUNTER_SPINNER_SVG_EM,
  STOPPED_LINGER_MS,
  counterSpoken,
  counterWidthPx,
  estimateStripTextPx,
  fitStripCounters,
  threadDrawerLine,
  threadStripView,
  type StripCounter,
  type StripCounterKey,
  type ThreadDrawerLine,
  type ThreadStripStep,
  type ThreadStripTone,
} from '@/components/SessionView/threadStrip'

const TONE: Record<ThreadStripTone, string> = {
  working: 'var(--zen-working)',
  monitoring: 'var(--zen-accent)',
  'needs-you': 'var(--zen-needs-you)',
  stopped: 'var(--zen-text-muted)',
  unverifiable: 'var(--zen-text-muted)',
}

/** Slide / fade length (ms); the CSS uses the same. */
export const ZEN_DRAWER_MS = 200
/** Collapsed tile height (px) above the box. */
export const ZEN_DRAWER_TILE_PX = 32
/** The desktop's Stop on the tile: its height and the room it keeps. */
const ZEN_DRAWER_STOP_H_PX = 20
export const ZEN_DRAWER_STOP_ROOM_PX = 48
/** Room the step keeps before counters start dropping. */
const STEP_MIN_PX = 72
const COUNTER_FONT_PX = 12
/** Each counter is a borderless tinted tag: side padding (px) and height. */
export const ZEN_DRAWER_TAG_PAD_X_PX = 5
const TAG_H_PX = 18

/** A counter's tag width (px): its glyphs plus the tag's side padding. */
export function zenCounterTagWidthPx(c: StripCounter, fontPx = COUNTER_FONT_PX): number {
  return counterWidthPx(c, fontPx) + 2 * ZEN_DRAWER_TAG_PAD_X_PX
}

/** Counter budget (px) for a collapsed tile `width` wide showing `clock`. */
export function zenDrawerCounterBudget(width: number, clock: string): number {
  if (!Number.isFinite(width) || width <= 0) return Infinity
  // 12px padding both sides, the 8px pulse, three 8px gaps, the clock, the step's floor.
  return width - 24 - 8 - 24 - estimateStripTextPx(clock, COUNTER_FONT_PX) - STEP_MIN_PX
}

/** Expanded or not, on this computer. */
export const ZEN_DRAWER_EXPANDED_KEY = 'k2.zen.activityDrawerExpanded'

export function readZenDrawerExpanded(): boolean {
  try {
    return localStorage.getItem(ZEN_DRAWER_EXPANDED_KEY) === '1'
  } catch {
    return false
  }
}

function writeZenDrawerExpanded(on: boolean): void {
  try {
    localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, on ? '1' : '0')
  } catch {
    /* the choice lasts this window */
  }
}

/** A Zen root, or the desktop Thread's box (it draws this drawer too). */
const DRAWER_SCOPE = `:is([data-zen-root], [${THREAD_ZEN_SCOPE_ATTR}])`

// Tags tint by kind: the fill is the hue at ~16%, the ink the hue mixed
// toward the text colour. Zen tokens, so every theme keeps its look.
export const ZEN_DRAWER_CSS = `
@keyframes zen-drawer-in { from { transform: translateY(calc(100% + 8px)); } to { transform: none; } }
@keyframes zen-drawer-out { from { transform: none; } to { transform: translateY(calc(100% + 8px)); } }
@keyframes zen-drawer-fade-in { from { opacity: 0; } }
@keyframes zen-drawer-fade-out { to { opacity: 0; } }
@keyframes zen-drawer-shimmer { 0% { opacity: 1; } 50% { opacity: 0.55; } 100% { opacity: 1; } }
${DRAWER_SCOPE} [data-zen-drawer][data-phase="in"] { animation: zen-drawer-in ${ZEN_DRAWER_MS}ms cubic-bezier(0.2, 0.9, 0.3, 1.12) both; }
${DRAWER_SCOPE} [data-zen-drawer][data-phase="out"] { animation: zen-drawer-out ${ZEN_DRAWER_MS}ms cubic-bezier(0.4, 0, 0.8, 0.4) both; }
${DRAWER_SCOPE} [data-zen-drawer-tile] { transition: background-color 140ms ease; }
${DRAWER_SCOPE} [data-zen-drawer-tile]:hover { background: var(--zen-surface); }
${DRAWER_SCOPE} .zen-drawer-shimmer { animation: zen-drawer-shimmer 1.6s ease-in-out infinite; }
${DRAWER_SCOPE} .zen-drawer-tag { border-radius: 9999px; background: color-mix(in srgb, var(--zen-tag-hue) 16%, transparent); color: color-mix(in srgb, var(--zen-tag-hue) 70%, var(--zen-text)); }
${DRAWER_SCOPE} .zen-drawer-count { display: inline-flex; align-items: center; flex: none; white-space: nowrap; line-height: 18px; }
${DRAWER_SCOPE} .zen-drawer-code { --zen-tag-hue: var(--zen-text-muted); color: inherit; padding: 1px 5px; -webkit-box-decoration-break: clone; box-decoration-break: clone; }
${DRAWER_SCOPE} .zen-drawer-tag-subagents { --zen-tag-hue: var(--zen-accent); }
${DRAWER_SCOPE} .zen-drawer-tag-cmd { --zen-tag-hue: var(--zen-working); }
${DRAWER_SCOPE} .zen-drawer-tag-edit { --zen-tag-hue: var(--zen-needs-you); }
${DRAWER_SCOPE} .zen-drawer-tag-read { --zen-tag-hue: #2dd4bf; }
${DRAWER_SCOPE} .zen-drawer-tag-search { --zen-tag-hue: #a855f7; }
${DRAWER_SCOPE} .zen-drawer-tag-background { --zen-tag-hue: var(--zen-text-muted); }
@media (prefers-reduced-motion: reduce) {
  ${DRAWER_SCOPE} [data-zen-drawer][data-phase="in"] { animation: zen-drawer-fade-in 160ms ease-out both; }
  ${DRAWER_SCOPE} [data-zen-drawer][data-phase="out"] { animation: zen-drawer-fade-out 160ms ease-in both; }
  ${DRAWER_SCOPE} .zen-drawer-shimmer, ${DRAWER_SCOPE} [data-zen-pulse] { animation: none !important; }
}
`

// ── Icons (24×24 strokes, `currentColor`; the phone's) ──────────────────

function Svg({ size, children, stroke = 2 }: { size: number; children: React.ReactNode; stroke?: number }): React.JSX.Element {
  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={stroke}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden
      style={{ display: 'inline-block', verticalAlign: '-0.2em', marginRight: COUNTER_ICON_GAP_PX }}
    >
      {children}
    </svg>
  )
}

const ICON: Record<StripCounterKey | 'check', (size: number) => React.JSX.Element> = {
  subagents: (s) => (
    <Svg size={s}>
      <rect x="4" y="7" width="16" height="12" rx="3" />
      <path d="M12 3v4" />
      <circle cx="9" cy="13" r="1" />
      <circle cx="15" cy="13" r="1" />
      <path d="M2 12v3M22 12v3" />
    </Svg>
  ),
  background: (s) => (
    <Svg size={s}>
      <circle cx="12" cy="12" r="3" />
      <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 1 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06A1.65 1.65 0 0 0 4.68 15a1.65 1.65 0 0 0-1.51-1H3a2 2 0 1 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06A1.65 1.65 0 0 0 9 4.68a1.65 1.65 0 0 0 1-1.51V3a2 2 0 1 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06A1.65 1.65 0 0 0 19.4 9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 1 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" />
    </Svg>
  ),
  read: (s) => (
    <Svg size={s}>
      <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
      <polyline points="14 2 14 8 20 8" />
    </Svg>
  ),
  search: (s) => (
    <Svg size={s}>
      <circle cx="11" cy="11" r="8" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
    </Svg>
  ),
  cmd: (s) => (
    <Svg size={s}>
      <polyline points="4 17 10 11 4 5" />
      <line x1="12" y1="19" x2="20" y2="19" />
    </Svg>
  ),
  edit: (s) => (
    <Svg size={s}>
      <path d="M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" />
    </Svg>
  ),
  check: (s) => (
    <Svg size={s} stroke={2.4}>
      <path d="M20 6 9 17l-5-5" />
    </Svg>
  ),
}

/** Running subagents: the app's working glyph (plan-agent-status-unify-v1:
 *  the cell spinner replaces the braille everywhere), as the narrow 2×3
 *  grid (a tight spot), in the theme's working colour. It stills itself for
 *  reduced motion.
 *
 *  Sized in em of the count beside it, so it follows the text size: the
 *  grid's ink is COUNTER_SPINNER_INK_EM tall (0.8em − 2px at 12px). The
 *  layout box is the ink's width (COUNTER_SPINNER_EM; the counter width
 *  estimate uses it) × 1em, plus 1px more after the agent icon than its
 *  usual gap, and a 2px gap before the count; the svg's
 *  empty margin overflows it, centred. The box sits 0.14em below the
 *  baseline so the grid is centred on the digits. */
function WorkingMark(): React.JSX.Element {
  const svg = `${COUNTER_SPINNER_SVG_EM}em`
  return (
    <span
      data-zen-drawer-spinner=""
      style={{
        display: 'inline-block',
        position: 'relative',
        width: `${COUNTER_SPINNER_EM}em`,
        height: '1em',
        marginLeft: COUNTER_SPINNER_LEAD_PX,
        marginRight: COUNTER_SPINNER_GAP_PX,
        verticalAlign: '-0.14em',
        color: 'var(--zen-working)',
      }}
    >
      <span
        data-zen-drawer-spinner-grid=""
        style={{ position: 'absolute', left: '50%', top: '50%', display: 'flex', transform: 'translate(-50%, -50%)' }}
      >
        <WorkingGrid size={svg} cols={COUNTER_SPINNER_COLS} />
      </span>
    </span>
  )
}

/** One collapsed counter's visible part (aria-hidden; the words are beside it). */
function CounterGlyphs({ c }: { c: StripCounter }): React.JSX.Element {
  const icon = ICON[c.key](COUNTER_ICON_PX)
  if (c.key !== 'subagents') {
    return (
      <span aria-hidden="true">
        {icon}
        {c.count}
      </span>
    )
  }
  return (
    <span aria-hidden="true">
      {icon}
      {c.count > 0 ? (
        <span data-zen-drawer-running="">
          <WorkingMark />
          {c.count}
        </span>
      ) : null}
      {c.done > 0 ? (
        <span data-zen-drawer-done="" style={{ marginLeft: c.count > 0 ? COUNTER_GROUP_GAP_PX : 0 }}>
          {ICON.check(COUNTER_CHECK_PX)}
          {c.done}
        </span>
      ) : null}
    </span>
  )
}

/** A tool line: backtick spans become code tags. */
function StepBody({ step }: { step: ThreadStripStep }): React.JSX.Element {
  if (step.kind !== 'line') return <>{step.text}</>
  const parts = step.text.split('`')
  return (
    <>
      {parts.map((p, i) =>
        i % 2 === 1 && i < parts.length - 1 ? (
          <code key={i} data-zen-drawer-code="" className="zen-drawer-tag zen-drawer-code" style={{ fontFamily: 'ui-monospace, SFMono-Regular, Menlo, monospace', fontSize: '0.92em' }}>
            {p}
          </code>
        ) : (
          <span key={i}>{i % 2 === 1 ? `\`${p}` : p}</span>
        ),
      )}
    </>
  )
}

/** A message sent while the agent works ends the open turn `superseded`
 *  at once (the daemon's `Tracker::start`, thread_activity.rs), but the new
 *  turn's first frame only comes once the message is delivered and the
 *  agent is on it (the inject's own timed sleeps are ~0.5s). Until then the
 *  drawer keeps showing the turn it had, so it doesn't slide out and back
 *  in (Rosson 2026-10-16: "if the agent is still working we don't need to
 *  collapse the activity dog ear and then re-open it"). Past this hold with
 *  no new turn, it goes as any ended turn does. */
export const SUPERSEDE_HOLD_MS = 2_000

/** The turn the drawer draws: `turn`, or while a `superseded` end is
 *  within its hold, the last live state of that same turn (`lastLive`). */
export function zenDrawerHeldTurn(turn: ThreadTurn | null, lastLive: ThreadTurn | null, now: number): ThreadTurn | null {
  if (!turn?.end || turn.end.reason !== 'superseded') return turn
  if (!lastLive || lastLive.turnId !== turn.turnId || lastLive.end !== null) return turn
  return now + turn.skewMs - turn.end.at < SUPERSEDE_HOLD_MS ? lastLive : turn
}

/** This client's clock, re-read every second while the turn runs (or is
 *  held after a supersede), and once more when a "Stopped" linger or the
 *  supersede hold runs out. */
function useDrawerNow(turn: ThreadTurn | null): number {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    setNow(Date.now())
    if (!turn) return
    if (turn.end === null) {
      const id = setInterval(() => setNow(Date.now()), 1_000)
      return () => clearInterval(id)
    }
    if (turn.end.reason === 'superseded') {
      const left = turn.end.at + SUPERSEDE_HOLD_MS - (Date.now() + turn.skewMs)
      if (left <= 0) return
      const tick = setInterval(() => setNow(Date.now()), 1_000)
      const done = setTimeout(() => setNow(Date.now()), left + 50)
      return () => {
        clearInterval(tick)
        clearTimeout(done)
      }
    }
    if (turn.end.reason === 'interrupted') {
      const left = turn.end.at + STOPPED_LINGER_MS - (Date.now() + turn.skewMs)
      if (left <= 0) return
      const id = setTimeout(() => setNow(Date.now()), left + 50)
      return () => clearTimeout(id)
    }
  }, [turn])
  return now
}

type Phase = 'hidden' | 'in' | 'out'

/** Whether the drawer has anything to show for `turn` now (the
 *  conversation hides its three dots while it does). */
export function zenDrawerShows(turn: ThreadTurn | null | undefined, now = Date.now()): boolean {
  return !!turn && threadDrawerLine(turn, now) !== null
}

export function ZenActivityDrawer({
  turn,
  onStop,
  attached = true,
}: {
  turn: ThreadTurn | null
  /** Sends Esc to the agent's session (the desktop Thread). Absent: no Stop. */
  onStop?: () => void
  /** On a message box's top edge (default). False: a free tile (a Thread
   *  with no message box, e.g. a view-only room). */
  attached?: boolean
}): React.JSX.Element | null {
  const now = useDrawerNow(turn)
  // The last live state, for the supersede hold (a message sent mid-turn).
  const lastLive = useRef<ThreadTurn | null>(null)
  if (turn && turn.end === null) lastLive.current = turn
  const drawn = zenDrawerHeldTurn(turn, lastLive.current, now)
  const line = drawn ? threadDrawerLine(drawn, now) : null
  const stoppable = !!onStop && !!drawn && threadStripView(drawn, now)?.stoppable === true
  const shown = line !== null
  const last = useRef<ThreadDrawerLine | null>(line)
  if (line) last.current = line

  // hidden → in (slide up) → out (slide down) → hidden.
  const [phase, setPhase] = useState<Phase>(shown ? 'in' : 'hidden')
  if (shown && phase !== 'in') setPhase('in')
  if (!shown && phase === 'in') setPhase('out')
  useEffect(() => {
    if (phase !== 'out') return
    const t = setTimeout(() => setPhase('hidden'), ZEN_DRAWER_MS + 60)
    return () => clearTimeout(t)
  }, [phase])

  const [expanded, setExpanded] = useState(readZenDrawerExpanded)
  const toggle = (): void => {
    setExpanded((on) => {
      writeZenDrawerExpanded(!on)
      return !on
    })
  }

  const tileRef = useRef<HTMLButtonElement | null>(null)
  const [width, setWidth] = useState(0)
  const display = line ?? (phase === 'out' ? last.current : null)
  const mounted = display !== null
  useLayoutEffect(() => {
    if (!mounted) return
    const measure = (): void => setWidth(tileRef.current?.clientWidth ?? 0)
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    if (tileRef.current) ro.observe(tileRef.current)
    return () => ro.disconnect()
  }, [mounted])

  if (!display) return null
  const color = TONE[display.tone]
  const counters = fitStripCounters(
    display.counters,
    zenDrawerCounterBudget(width, display.clock) - (stoppable ? ZEN_DRAWER_STOP_ROOM_PX : 0),
    (c) => zenCounterTagWidthPx(c),
  )
  const padRight = stoppable ? 12 + ZEN_DRAWER_STOP_ROOM_PX : 12
  const hollow = display.tone === 'monitoring' || display.tone === 'stopped' || display.tone === 'unverifiable'
  const dot = (
    <span
      aria-hidden
      data-zen-drawer-dot=""
      data-zen-pulse={display.tone === 'working' ? '' : undefined}
      style={{
        display: 'inline-block',
        flex: 'none',
        width: 8,
        height: 8,
        borderRadius: 999,
        boxSizing: 'border-box',
        ...(hollow ? { border: `2px solid ${color}` } : { background: color }),
      }}
    />
  )
  const d = display.detail
  const muted = display.tone === 'stopped' || display.tone === 'unverifiable'

  return (
    // The clip box: the tile slides up out of the message box's top edge.
    <div data-zen-drawer-clip="" style={{ overflow: 'hidden', margin: attached ? '0 14px -1px' : '0 var(--zen-gap, 12px) 8px' }}>
      <style data-zen-drawer-styles="">{ZEN_DRAWER_CSS}</style>
      <div
        role="status"
        data-zen-drawer-attached={attached ? 'true' : 'false'}
        style={{ position: 'relative' }}
        data-zen-drawer=""
        data-phase={phase}
        data-tone={display.tone}
        data-turn-phase={drawn?.phase}
        data-expanded={expanded ? 'true' : 'false'}
        onAnimationEnd={(e) => {
          if (e.target === e.currentTarget && phase === 'out') setPhase('hidden')
        }}
      >
        <button
          ref={tileRef}
          type="button"
          data-zen-drawer-tile=""
          // Companion's `k2-tile k2-tile-sheet`: a tile; on the box, a sheet
          // with no bottom rim (Glass, `zen-materials.ts`).
          {...ZEN_TILE_RAISED_PROPS}
          data-zen-tile-sheet={attached ? '' : undefined}
          // K2's square Style (desktop Thread): chamfered, top corners only
          // when it sits on the message box.
          data-chamfer=""
          aria-expanded={expanded}
          aria-label={display.ariaLabel}
          title={expanded ? 'Show less' : 'Show the detail'}
          // Keep the caret in the message box: a click doesn't steal focus.
          onMouseDown={(e) => e.preventDefault()}
          onClick={toggle}
          className="block w-full min-w-0 text-left cursor-pointer"
          style={{
            overflow: 'hidden',
            color: 'var(--zen-text)',
            background: 'var(--zen-surface-raised)',
            border: '1px solid var(--zen-border)',
            borderStyle: display.tone === 'unverifiable' ? 'dashed' : 'solid',
            ...(attached
              ? { borderBottom: 'none', borderRadius: 'calc(var(--zen-radius) + 2px) calc(var(--zen-radius) + 2px) 0 0' }
              : { borderRadius: 'calc(var(--zen-radius) + 2px)' }),
            ...chamferVars({
              cut: attached ? [CHAMFER_CUT.drawer, CHAMFER_CUT.drawer, 0, 0] : CHAMFER_CUT.drawer,
              fill: 'var(--zen-surface-raised)',
              edge: 'var(--zen-border)',
              border: 1,
              outset: attached ? '-1px -1px 0 -1px' : undefined,
            }),
          }}
        >
          {expanded ? (
            <span data-zen-drawer-detail="" className="flex min-w-0 flex-col" style={{ gap: 1, padding: `6px ${padRight}px 6px 12px` }}>
              <span className="flex min-w-0 items-center" style={{ gap: 8, fontSize: 13, lineHeight: '18px' }}>
                {dot}
                <span data-zen-drawer-head="" className="min-w-0 break-words" style={{ color, fontWeight: 600 }}>
                  {d.head}
                </span>
              </span>
              {d.step ? (
                <span data-zen-drawer-step-full="" className="flex min-w-0 items-baseline" style={{ gap: 8, fontSize: 13, lineHeight: '19px' }}>
                  <span
                    className={`min-w-0 flex-1 break-words ${d.step.kind === 'thinking' ? 'zen-drawer-shimmer' : ''}`}
                    style={d.step.kind === 'needs' ? { color: 'var(--zen-needs-you)' } : undefined}
                  >
                    <StepBody step={d.step} />
                  </span>
                  {d.step.elapsed ? (
                    <span data-zen-drawer-elapsed="" className="flex-none tabular-nums" style={{ fontSize: 12, color: 'var(--zen-text-muted)' }}>
                      {d.step.elapsed}
                    </span>
                  ) : null}
                </span>
              ) : null}
              {d.tally ? (
                <span data-zen-drawer-tally="" className="min-w-0 break-words" style={{ fontSize: 12, lineHeight: '18px', color: 'var(--zen-text-muted)' }}>
                  {d.tally}
                </span>
              ) : null}
              {d.children ? (
                <span data-zen-drawer-children="" className="min-w-0 break-words" style={{ fontSize: 12, lineHeight: '18px', color: 'var(--zen-text-muted)' }}>
                  {d.children}
                </span>
              ) : null}
            </span>
          ) : (
            <span
              data-zen-drawer-line=""
              className="flex min-w-0 items-center whitespace-nowrap"
              style={{ gap: 8, padding: `0 ${padRight}px 0 12px`, height: ZEN_DRAWER_TILE_PX - 1 }}
            >
              {dot}
              <span
                data-zen-drawer-step=""
                className={`min-w-0 flex-1 overflow-hidden text-ellipsis whitespace-nowrap ${display.step.kind === 'thinking' ? 'zen-drawer-shimmer' : ''}`}
                style={{
                  fontSize: 13,
                  lineHeight: '18px',
                  color: display.step.kind === 'needs' ? 'var(--zen-needs-you)' : muted ? 'var(--zen-text-muted)' : 'var(--zen-text)',
                }}
              >
                <StepBody step={display.step} />
              </span>
              {counters.length ? (
                <span data-zen-drawer-counters="" className="flex flex-none items-center tabular-nums" style={{ gap: COUNTER_GAP_PX, fontSize: 12, lineHeight: '18px' }}>
                  {counters.map((c) => (
                    <span
                      key={c.key}
                      data-zen-drawer-count={c.key}
                      className={`zen-drawer-tag zen-drawer-count zen-drawer-tag-${c.key}`}
                      style={{ height: TAG_H_PX, paddingLeft: ZEN_DRAWER_TAG_PAD_X_PX, paddingRight: ZEN_DRAWER_TAG_PAD_X_PX }}
                    >
                      <CounterGlyphs c={c} />
                      <span className="sr-only">{counterSpoken(c)}</span>
                    </span>
                  ))}
                </span>
              ) : null}
              <span data-zen-drawer-clock="" className="flex-none tabular-nums" style={{ fontSize: 12, lineHeight: '18px', color: 'var(--zen-text-muted)' }}>
                {display.clock}
              </span>
            </span>
          )}
        </button>
        {stoppable && (
          <button
            type="button"
            data-testid="thread-strip-stop"
            data-zen-drawer-stop=""
            data-chamfer=""
            title="Sends Esc to the agent"
            // Keep the caret in the message box.
            onMouseDown={(e) => e.preventDefault()}
            onClick={onStop}
            className="cursor-pointer"
            style={{
              position: 'absolute',
              top: 6,
              right: 8,
              height: ZEN_DRAWER_STOP_H_PX,
              padding: '0 9px',
              borderRadius: 999,
              fontSize: 11,
              lineHeight: `${ZEN_DRAWER_STOP_H_PX - 2}px`,
              color: 'var(--zen-text)',
              background: 'var(--zen-surface)',
              border: '1px solid var(--zen-border)',
              // K2's square Style: chamfered like Send, scaled to its height.
              ...chamferVars({ cut: chamferCutFor(ZEN_DRAWER_STOP_H_PX), fill: 'var(--zen-surface)', edge: 'var(--zen-border)', border: 1 }),
            }}
          >
            Stop
          </button>
        )}
      </div>
    </div>
  )
}
