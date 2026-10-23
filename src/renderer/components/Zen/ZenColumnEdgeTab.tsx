// prd-garden-column-collapse-and-agent-list-v1 A1 (CA15) — the folded
// column's edge tab and the chevron that folds it.
//
// A folded column is a tab about 28 px wide and the column's full height,
// on its outer side (a right column's hugs the right edge, a left column's
// the left; a middle one stays between its neighbours). The whole tab is one
// `<button>` (at least 28 × 44): the vertical label, an expand chevron, and
// the unread and waiting counts of the Agents or Conversation widgets inside
// (a fold never hides an agent that needs you). It is never a drag area; on a
// full canvas it is drawn above the floating top band and the frame's hit
// regions, starting below the band so it never covers a K2 control there.
//
// The chevron sits in the open column's top outer corner. Both carry
// `aria-expanded` and `aria-controls`; there is no keyboard shortcut (CA3):
// Tab to them, then Enter or Space.

import { useEffect, useMemo, useState } from 'react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { ZenBridgeError } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { ZEN_COLUMN_TAB_MIN_HEIGHT_PX, ZEN_COLUMN_TAB_PX } from '@/lib/zen/zen-overflow'
import { zenConversationRow } from './widgets/ZenConversationWidget'
import { ZenUnreadPills } from './widgets/ZenAgentsWidget'
import { TEXTING_BAR_HEIGHT_PX } from './ZenTemplateControls'

/** Which side of the row a column sits on: its tab hugs that edge. */
export type ZenColumnSide = 'left' | 'middle' | 'right'

export function zenColumnSide(index: number, ncols: number): ZenColumnSide {
  if (index === 0) return 'left'
  if (index === ncols - 1) return 'right'
  return 'middle'
}

/** On a full canvas the top band floats over the columns: K2's column
 *  controls start below it, so they never cover a control in the band. */
export const ZEN_FULL_CANVAS_BAND_CSS = `max(${TEXTING_BAR_HEIGHT_PX + 8}px, var(--zen-stoplight-safe-top, 0px))`

/** A widget in the column whose rows count toward the tab's badge. */
export interface ZenColumnBadgeSource {
  decl: ZenWidgetDecl
  bridge: ZenWidgetBridge
}

/** The Agents and Conversation widgets in a column that can read rows. */
export function zenColumnBadgeSources(
  widgets: readonly ZenWidgetDecl[],
  bridges: ReadonlyMap<string, ZenWidgetBridge>,
): ZenColumnBadgeSource[] {
  return widgets.flatMap((decl) => {
    if (decl.kind !== 'agents' && decl.kind !== 'conversation') return []
    const bridge = bridges.get(decl.id)
    if (!bridge || !bridge.caps.has('agents:read')) return []
    return [{ decl, bridge }]
  })
}

/** The unread and waiting counts on a column's rows: every row of an Agents
 *  widget, the row a Conversation shows; each agent once. Muted unread
 *  doesn't count; waiting always does. */
export function zenColumnUnread(rowsBySource: ReadonlyArray<readonly ZenAgentRow[]>): { unread: number; waiting: number } {
  const seen = new Map<string, ZenAgentRow>()
  for (const rows of rowsBySource) for (const r of rows) seen.set(r.address, r)
  let unread = 0
  let waiting = 0
  for (const r of seen.values()) {
    if (!r.unread) continue
    if (!r.unread.muted) unread += Math.max(0, r.unread.unread)
    waiting += Math.max(0, r.unread.waiting)
  }
  return { unread, waiting }
}

function rowsOf(source: ZenColumnBadgeSource, rows: readonly ZenAgentRow[]): ZenAgentRow[] {
  if (source.decl.kind !== 'conversation') return [...rows]
  const row = zenConversationRow(rows, source.decl.props)
  return row ? [row] : []
}

/** Live rows of each source (only while the tab is drawn). A source whose
 *  data verbs aren't installed (`verb_unavailable`) counts nothing.
 *  `sources` must be memoized by the caller (a new list resubscribes). */
function useColumnUnread(sources: readonly ZenColumnBadgeSource[]): { unread: number; waiting: number } {
  const [rows, setRows] = useState<ReadonlyArray<readonly ZenAgentRow[]>>(() => sources.map(() => []))
  useEffect(() => {
    let live = true
    const next: ZenAgentRow[][] = sources.map(() => [])
    const put = (i: number, r: readonly ZenAgentRow[]): void => {
      next[i] = rowsOf(sources[i], r)
      if (live) setRows([...next])
    }
    const offs: Array<() => void> = []
    sources.forEach((s, i) => {
      try {
        put(i, s.bridge.call('agents.list') as ZenAgentRow[])
        const off = s.bridge.call('agents.subscribe', (r: ZenAgentRow[]) => put(i, r))
        if (typeof off === 'function') offs.push(off as () => void)
      } catch (err) {
        if (err instanceof ZenBridgeError && err.code === 'verb_unavailable') return
        throw err
      }
    })
    return () => {
      live = false
      for (const off of offs) off()
    }
  }, [sources])
  return useMemo(() => zenColumnUnread(rows), [rows])
}

/** "Expand Chat", "Expand Chat, 2 unread", "Expand Chat, 1 waiting on you, 2 unread". */
export function zenColumnTabName(label: string, counts: { unread: number; waiting: number }): string {
  const parts = [`Expand ${label}`]
  if (counts.waiting > 0) parts.push(`${counts.waiting} waiting on you`)
  if (counts.unread > 0) parts.push(`${counts.unread} unread`)
  return parts.join(', ')
}

function Chevron({ points }: { points: 'left' | 'right' }): React.JSX.Element {
  return (
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden focusable="false">
      <path
        d={points === 'left' ? 'M7.5 2.5 4 6l3.5 3.5' : 'M4.5 2.5 8 6 4.5 9.5'}
        fill="none"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}

export function ZenColumnEdgeTab({
  index,
  label,
  side,
  controls,
  sources,
  full,
  onExpand,
  buttonRef,
}: {
  index: number
  label: string
  side: ZenColumnSide
  /** The id of the column's (hidden) body. */
  controls: string
  sources: readonly ZenColumnBadgeSource[]
  /** A full-canvas page: drawn above the band and the frame's hit regions. */
  full: boolean
  onExpand(): void
  buttonRef?: (el: HTMLButtonElement | null) => void
}): React.JSX.Element {
  const counts = useColumnUnread(sources)
  const name = zenColumnTabName(label, counts)
  const badge = counts.unread > 0 || counts.waiting > 0 ? { unread: counts.unread, waiting: counts.waiting, muted: false } : null
  return (
    <button
      ref={buttonRef}
      type="button"
      data-zen-column-tab={index}
      data-zen-column-side={side}
      aria-expanded={false}
      aria-controls={controls}
      aria-label={name}
      title={name}
      onClick={onExpand}
      className="flex flex-col items-center"
      style={{
        position: 'relative',
        zIndex: full ? 4 : undefined,
        width: ZEN_COLUMN_TAB_PX,
        minWidth: ZEN_COLUMN_TAB_PX,
        minHeight: ZEN_COLUMN_TAB_MIN_HEIGHT_PX,
        flex: '1 1 auto',
        marginTop: full ? ZEN_FULL_CANVAS_BAND_CSS : undefined,
        gap: 8,
        padding: '10px 0',
        border: '1px solid var(--zen-border)',
        borderRadius: 'var(--zen-radius)',
        background: 'var(--zen-surface)',
        color: 'var(--zen-text-muted)',
        cursor: 'pointer',
        boxSizing: 'border-box',
      }}
    >
      <Chevron points={side === 'left' ? 'right' : 'left'} />
      {badge && (
        <span data-zen-column-tab-badge="" style={{ display: 'flex', flexDirection: 'column', alignItems: 'center' }}>
          <ZenUnreadPills unread={badge} />
        </span>
      )}
      <span
        data-zen-column-tab-label=""
        style={{
          writingMode: 'vertical-rl',
          transform: side === 'left' ? 'rotate(180deg)' : undefined,
          fontSize: 12,
          fontWeight: 600,
          whiteSpace: 'nowrap',
          overflow: 'hidden',
          textOverflow: 'ellipsis',
          minHeight: 0,
          color: 'var(--zen-text)',
        }}
      >
        {label}
      </span>
    </button>
  )
}

export function ZenColumnChevron({
  index,
  label,
  side,
  controls,
  disabled,
  full,
  onFold,
}: {
  index: number
  label: string
  side: ZenColumnSide
  controls: string
  /** The last open column: it can't fold (CA10). */
  disabled: boolean
  full: boolean
  onFold(): void
}): React.JSX.Element {
  const name = disabled ? `${label} is the only open column` : `Collapse ${label}`
  return (
    <button
      type="button"
      data-zen-column-chevron={index}
      aria-expanded
      aria-controls={controls}
      aria-label={name}
      title={name}
      disabled={disabled}
      onClick={onFold}
      className="flex items-center justify-center"
      style={{
        position: 'absolute',
        top: full ? `calc(${ZEN_FULL_CANVAS_BAND_CSS} + 4px)` : 4,
        ...(side === 'left' ? { left: 4 } : { right: 4 }),
        zIndex: full ? 4 : 2,
        width: 22,
        height: 22,
        padding: 0,
        border: '1px solid var(--zen-border)',
        borderRadius: 999,
        background: 'var(--zen-surface-raised)',
        color: 'var(--zen-text-muted)',
        cursor: disabled ? 'default' : 'pointer',
        opacity: disabled ? 0.4 : 1,
      }}
    >
      <Chevron points={side === 'left' ? 'left' : 'right'} />
    </button>
  )
}
