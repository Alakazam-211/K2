// prd-zen-mode-v1 Z38, Z39, Z32 (answers 4, 5, 9) and prd-zen-gardens-v1
// G27, G38 (Rosson's 2026-10-04 answer 5) — the Agents widget: a Home's
// agents like a list of chats.
//
// It shows ONE Home: its own pick from the Home picker in its header
// ("Home: [Work ▾]", with the `home-picker` prop), else its `home` prop,
// else the Garden's seed Home, else the window's selected Home when it
// first showed. Picking a Home here never moves the Home page. Two modes,
// by props: the whole Home (default), or one agent filtered from it
// (`agent`: a row address, handle or name).
//
// Display props (the daemon sends every prop, defaults filled):
// `server-tag`, `preview` and `status` (which live statuses show).
//
// One row per Home row, in Home order (⌘1–9 selects row N of the page's
// first Agents widget). Each row: avatar (the agent's picture when it has
// one — `row.avatarUrl` from the bridge — else its initials),
// name, the server when it isn't this computer, the agent's live status
// (working, monitoring, needs you, no update, idle — the daemon's display,
// prd-daemon-activity-and-thread-working-v1 Q13; no status when its server
// can't say), and the
// start of its last message. No unread dots (answer 9). A row that can't be
// messaged says why ("Offline", "Sign in", "No access", "Update <server>").
// Selecting a row opens its conversation in place (`conversation.open`):
// never a window server switch. ⌘↑ / ⌘↓ move between conversations.
//
// The list ends with an "Add agent" row in its bottom-left corner (Rosson
// 2026-10-04: part of the list, not a floating page control). It opens K2's
// Add agent picker for THIS widget's Home through the bridge (`agents.add`,
// cap `agents:add`) above the row. It shows in whole-Home mode when the
// widget holds the cap; a one-agent widget has nothing to add to.
//
// The Agents view (Rosson 2026-10-04, the nav rail's Agents in Garden 1):
// the same widget with `source: "workspaces"` lists THIS server's
// workspaces the way the app's Agents page does. In place of the Home
// picker it shows the app's focus-group dropdown (`FocusGroupDropdown`)
// when focus groups are on; picking a group lists that group's agents
// (`focusGroups.set`, never a workspace switch). No Add agent row there:
// it adds to a Home.
//
// Everything comes through the bridge (`agents:read`, `agents:add`).
//
// 0.45.6 Garden 1 (Rosson 2026-10-15, ported from K2 Companion):
//   - `pins`: pinned agents (their workspace pin on their own server) as a
//     grid of pictures at the top, three across, more rows as you pin more
//     (no paging); the list below leaves them out (`zen-pin-grid.ts`).
//   - `settings`: a cog in the header opens the Garden's own settings
//     (`ZenGardenSettings`), not the app's Settings window.
//   - "Recent agents first" (that settings view, this computer only, off by
//     default): pinned agents stay first, then the agents you last messaged
//     from a Garden, newest first (`zen-recent-first.ts`). ⌘↑ / ⌘↓ follow
//     what's drawn; ⌘1–9 keep the Home's own order.
// 0.46.0 (Garden 1's template sets both; an older daemon sends neither):
//   - `pin-limit`: the grid holds the first N pinned agents (Garden 1: 9,
//     3 × 3); pinned agents past it head the list.
//   - `recent-first`: the Garden's default for "Recent agents first" until
//     the person picks on this computer (Garden 1: on).
//   The cog's settings also pick this Garden's theme.
// Only Gardens read any of this; the app's Agents page doesn't change.

import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import { zenCountsText } from '@/lib/zen/zen-counts'
import type { ZenHomeSummary, ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenWidgetProps } from '../zen-registry'
import { ZenAgentAvatar, ZenWidgetStyles, shortAge, useNowSec, useZenRows } from './zen-widget-kit'
import { useZenAddAgentClick } from '../ZenTemplateControls'
import { requestZenComposeFocus } from '@/lib/zen/zen-compose-focus'
import { useZenAddAgentStore } from '@/lib/zen/zen-add-agent'
import { zenGardenHomeKey } from '@/lib/zen/zen-garden-homes'
import { zenAgentsSource } from '@/lib/zen/zen-rail-views'
import type { ZenFocusGroups } from '@/lib/zen/zen-data'
import FocusGroupDropdown from '@/components/Sidebar/FocusGroupDropdown'
import { useZenRecentFirst, useZenRecentFirstStore, zenRecentFirstProp } from '@/lib/zen/zen-recent-first'
import {
  ZEN_PIN_COL_GAP,
  ZEN_PIN_GRID_BOTTOM_PX,
  ZEN_PIN_ROW_GAP,
  zenAgentsDisplay,
  zenPinAvatarPx,
  zenPinLimit,
  zenPinGridCentred,
  zenPinGridColumns,
} from '@/lib/zen/zen-pin-grid'
import { ZenGardenSettingsButton } from './ZenGardenSettings'

export const ZEN_EMPTY_HOME = 'This Home has no agents yet. Use Add agent to add some.'
export const ZEN_EMPTY_SERVER = 'No agents on this server yet.'
export const ZEN_EMPTY_FOCUS_GROUP = 'No agents in this focus group.'
export const zenAgentNotOnHome = (agent: string, home: string): string => `${agent} isn’t on ${home}.`

function propString(props: Record<string, unknown>, name: string): string | null {
  const v = props[name]
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

function homePickerOn(props: Record<string, unknown>): boolean {
  return props['home-picker'] === true || props.homePicker === true || props.home_picker === true
}

function propOn(props: Record<string, unknown>, name: string): boolean {
  return props[name] !== false
}

/** Which `status` prop value shows an activity. The prop keeps its three
 *  values (`working`, `needs-you`, `idle`): `monitoring` shows with
 *  `working` and `unverifiable` with `idle` (Q13). */
function statusGroup(activity: NonNullable<ZenAgentRow['activity']>): string {
  if (activity === 'monitoring') return 'working'
  if (activity === 'unverifiable') return 'idle'
  return activity
}

/** The row as this widget's display props show it. */
function displayRow(row: ZenAgentRow, props: Record<string, unknown>): ZenAgentRow {
  const statuses = Array.isArray(props.status) ? props.status.filter((s): s is string => typeof s === 'string') : null
  const activity =
    row.activity !== null && statuses && !statuses.includes(statusGroup(row.activity)) ? null : row.activity
  return {
    ...row,
    server: propOn(props, 'server-tag') ? row.server : null,
    preview: propOn(props, 'preview') ? row.preview : null,
    activity,
  }
}

/** The widget's own Home picker (G27): "Home: [Work ▾]". */
function HomePicker({
  homes,
  current,
  onPick,
}: {
  homes: ZenHomeSummary[]
  current: ZenHomeSummary | null
  onPick(id: string): void
}): React.JSX.Element {
  const [open, setOpen] = useState(false)
  // Portalled into the Zen root and fixed to the button: the column card's
  // `overflow: hidden` (ZenPage) can't cut it off.
  const menu = useAnchoredMenu<HTMLButtonElement>({
    open,
    onClose: () => setOpen(false),
    gap: 4,
    width: 'min',
    minWidth: 180,
  })
  return (
    <div className="relative flex min-w-0 items-center gap-1.5" data-zen-home-picker="">
      <span style={{ color: 'var(--zen-text-muted)' }}>Home:</span>
      <button
        ref={menu.anchorRef}
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        title="Show another Home here"
        data-zen-home-picker-button=""
        data-zen-soft-button=""
        onClick={() => setOpen((v) => !v)}
        className="flex min-w-0 items-center gap-1 cursor-pointer"
        style={{
          padding: '1px 8px',
          borderRadius: 999,
          border: '1px solid var(--zen-border)',
          color: 'var(--zen-text)',
          textTransform: 'none',
          letterSpacing: 0,
          fontWeight: 600,
        }}
      >
        <span className="max-w-[10rem] truncate">{current?.name ?? 'Pick a Home'}</span>
        <svg width="9" height="9" viewBox="0 0 10 10" aria-hidden style={{ color: 'var(--zen-text-muted)' }}>
          <path d="M2 3.5 5 6.5 8 3.5" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
        </svg>
      </button>
      {open && menu.portal(
        <div
          ref={menu.menuRef}
          role="menu"
          data-zen-home-picker-menu=""
          data-placement={menu.placement}
          className="no-drag flex flex-col"
          style={{
            ...menu.style,
            padding: 4,
            gap: 2,
            background: 'var(--zen-surface-raised)',
            border: '1px solid var(--zen-border)',
            borderRadius: 'var(--zen-radius)',
            boxShadow: '0 10px 30px rgba(0, 0, 0, 0.14)',
            textTransform: 'none',
            letterSpacing: 0,
            fontWeight: 400,
          }}
        >
          {homes.map((h) => (
            <button
              key={h.id}
              type="button"
              role="menuitemradio"
              aria-checked={h.id === current?.id}
              data-zen-home-choice={h.id}
              onClick={() => {
                setOpen(false)
                onPick(h.id)
              }}
              className="flex w-full items-center text-left cursor-pointer"
              style={{
                minHeight: 28,
                padding: '4px 10px',
                borderRadius: 'calc(var(--zen-radius) - 4px)',
                color: 'var(--zen-text)',
                background: h.id === current?.id ? 'var(--zen-surface)' : 'transparent',
                fontWeight: h.id === current?.id ? 600 : 400,
              }}
            >
              <span className="truncate">{h.name}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  )
}

/** The app's focus groups (`focusGroups.get` + `focusGroups.subscribe`),
 *  for the Agents view; null in a Home view. */
function useFocusGroups(bridge: ZenWidgetBridge, on: boolean): ZenFocusGroups | null {
  const [groups, setGroups] = useState<ZenFocusGroups | null>(() =>
    on ? (bridge.call('focusGroups.get') as ZenFocusGroups) : null,
  )
  useEffect(() => {
    if (!on) {
      setGroups(null)
      return
    }
    setGroups(bridge.call('focusGroups.get') as ZenFocusGroups)
    const off = bridge.call('focusGroups.subscribe', (next: ZenFocusGroups) => setGroups(next))
    if (typeof off !== 'function') throw new Error('zen: focusGroups.subscribe returned no unsubscribe')
    return off as () => void
  }, [bridge, on])
  return groups
}

/** The Agents view's focus-group dropdown: the app's own component. */
function FocusGroupPicker({ bridge, groups }: { bridge: ZenWidgetBridge; groups: ZenFocusGroups }): React.JSX.Element {
  return (
    <div
      data-zen-focus-group-picker=""
      className="min-w-0 flex items-center gap-1.5"
      style={{ width: 240 }}
    >
      <span data-zen-focus-group-label="" className="flex-shrink-0 text-[11px] font-semibold uppercase tracking-wide opacity-70">
        Group:
      </span>
      <div className="min-w-0 flex-1" style={{ textTransform: 'none', letterSpacing: 0, fontWeight: 400 }}>
      <FocusGroupDropdown
        options={groups.groups.map((g) => ({ id: g.id, name: g.name, color: g.color }))}
        value={groups.active}
        menuAttributes={{ 'data-zen-focus-group-menu': '' }}
        onChange={(id) => {
          if (id === null) return
          try {
            bridge.call('focusGroups.set', id)
          } catch (err) {
            console.warn('[zen] picking a focus group failed:', err)
          }
        }}
      />
      </div>
    </div>
  )
}

/** The widget's Home (`agents.home`), re-read as its rows change. */
function useWidgetHome(bridge: ZenWidgetBridge, rows: ZenAgentRow[]): [string | null, (id: string) => void] {
  const [homeId, setHomeId] = useState<string | null>(() => bridge.call('agents.home') as string | null)
  useEffect(() => {
    setHomeId(bridge.call('agents.home') as string | null)
  }, [bridge, rows])
  const pick = (id: string): void => {
    bridge.call('agents.setHome', id)
    setHomeId(id)
  }
  return [homeId, pick]
}

const ACTIVITY_TEXT = {
  working: 'working',
  monitoring: 'monitoring',
  'needs-you': 'needs you',
  unverifiable: 'no update',
  idle: 'idle',
} as const

/** The status mark on the avatar, and its word. Monitoring is a hollow
 *  accent ring, no update a dashed one (the daemon's displays, Q13). */
export function ZenStatusDot({ row, size = 10 }: { row: ZenAgentRow; size?: number }): React.JSX.Element | null {
  if (row.activity === null) return null
  const color =
    row.activity === 'working' ? 'var(--zen-working)' : row.activity === 'needs-you' ? 'var(--zen-needs-you)' : 'transparent'
  const border =
    row.activity === 'idle'
      ? '1.5px solid var(--zen-text-muted)'
      : row.activity === 'monitoring'
        ? '2px solid var(--zen-accent)'
        : row.activity === 'unverifiable'
          ? '1.5px dashed var(--zen-text-muted)'
          : '2px solid var(--zen-surface)'
  return (
    <span
      aria-hidden
      data-zen-status-dot={row.activity}
      data-zen-pulse={row.activity === 'working' ? '' : undefined}
      style={{
        display: 'inline-block',
        width: size,
        height: size,
        borderRadius: 999,
        background: color,
        border,
        boxSizing: 'border-box',
      }}
    />
  )
}

function AgentRow({
  row,
  nowSec,
  onOpen,
}: {
  row: ZenAgentRow
  nowSec: number
  onOpen(address: string): void
}): React.JSX.Element {
  const second = row.state !== 'ok'
    ? row.stateLabel
    : row.preview
      ? `${row.preview.mine ? 'You: ' : ''}${row.preview.text}`
      : null
  const muted = row.state !== 'ok'
  // Small, beside the word, only while the agent is busy (the finished
  // turn's numbers stay on an idle row; they aren't news).
  const busy = row.activity === 'working' || row.activity === 'monitoring' || row.activity === 'needs-you'
  const countsText = row.state === 'ok' && busy ? zenCountsText(row.counts) : null
  return (
    <li role="presentation">
      <button
        type="button"
        role="option"
        aria-selected={row.selected}
        title={row.detail ?? (row.server ? `${row.label} on ${row.server}` : row.label)}
        data-zen-agent-row={row.address}
        data-selected={row.selected ? '' : undefined}
        data-activity={row.activity ?? 'unknown'}
        data-state={row.state}
        onClick={() => onOpen(row.address)}
        className="flex w-full items-center gap-3 text-left"
        style={{
          padding: '10px 12px',
          borderRadius: 'var(--zen-radius)',
          color: 'var(--zen-text)',
          background: 'transparent',
          opacity: muted ? 0.72 : 1,
          cursor: 'default',
        }}
      >
        <span className="relative flex-shrink-0" style={{ width: 40, height: 40 }}>
          <ZenAgentAvatar
            label={row.label}
            url={row.avatarUrl}
            size={40}
            agentColor={row.selected ? null : row.color}
            background={row.selected ? 'var(--zen-accent)' : 'var(--zen-bubble-agent)'}
            color={row.selected ? 'var(--zen-accent-text)' : 'var(--zen-bubble-agent-text)'}
            fontSize="0.95em"
          />
          <span className="absolute" style={{ right: -1, bottom: -1, lineHeight: 0 }}>
            <ZenStatusDot row={row} size={12} />
          </span>
        </span>
        <span className="flex min-w-0 flex-1 flex-col" style={{ gap: 2 }}>
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="truncate" style={{ fontWeight: 600 }} data-zen-agent-name="">
              {row.label}
            </span>
            {row.server && (
              <span
                className="flex-shrink-0 truncate"
                data-zen-server-tag=""
                style={{
                  maxWidth: '9rem',
                  fontSize: '0.72em',
                  padding: '1px 7px',
                  borderRadius: 999,
                  border: '1px solid var(--zen-border)',
                  color: 'var(--zen-text-muted)',
                }}
              >
                {row.server}
              </span>
            )}
            <span className="ml-auto flex-shrink-0" style={{ fontSize: '0.75em', color: 'var(--zen-text-muted)' }}>
              {row.state === 'ok' && row.preview ? shortAge(row.preview.at, nowSec) : ''}
            </span>
          </span>
          <span className="flex min-w-0 items-center gap-2" style={{ fontSize: '0.86em' }}>
            <span
              className="min-w-0 flex-1 truncate"
              data-zen-preview={row.state === 'ok' && row.preview ? '' : undefined}
              style={{ color: 'var(--zen-text-muted)' }}
            >
              {second ?? ' '}
            </span>
            {countsText && (
              <span
                className="flex-shrink-0"
                data-zen-activity-counts=""
                style={{ fontSize: '0.78em', color: 'var(--zen-text-muted)' }}
              >
                {countsText}
              </span>
            )}
            {row.activity !== null && row.state === 'ok' && (
              <span
                className="flex-shrink-0"
                data-zen-activity={row.activity}
                style={{
                  fontSize: '0.85em',
                  color:
                    row.activity === 'working'
                      ? 'var(--zen-working)'
                      : row.activity === 'needs-you'
                        ? 'var(--zen-needs-you)'
                        : row.activity === 'monitoring'
                          ? 'var(--zen-accent)'
                          : 'var(--zen-text-muted)',
                  fontWeight: row.activity === 'idle' || row.activity === 'unverifiable' ? 400 : 600,
                }}
              >
                {ACTIVITY_TEXT[row.activity]}
              </span>
            )}
          </span>
        </span>
      </button>
    </li>
  )
}

/** One pinned agent in the grid: its picture with the status mark, its
 *  name, and while it's busy the status word under it. */
function PinCell({ row, avatarPx, onOpen }: { row: ZenAgentRow; avatarPx: number; onOpen(address: string): void }): React.JSX.Element {
  const busy = row.state === 'ok' && (row.activity === 'working' || row.activity === 'monitoring' || row.activity === 'needs-you')
  const tone =
    row.activity === 'working' ? 'var(--zen-working)' : row.activity === 'needs-you' ? 'var(--zen-needs-you)' : 'var(--zen-accent)'
  const dotPx = Math.max(10, Math.round(avatarPx * 0.24))
  const spoken = `Pinned ${row.label}${busy && row.activity ? `, ${ACTIVITY_TEXT[row.activity]}` : row.state !== 'ok' && row.stateLabel ? `, ${row.stateLabel}` : ''}`
  return (
    <li role="presentation" className="min-w-0">
      <button
        type="button"
        role="option"
        aria-selected={row.selected}
        aria-label={spoken}
        title={row.detail ?? (row.server ? `${row.label} on ${row.server}` : row.label)}
        data-zen-pin={row.address}
        data-selected={row.selected ? '' : undefined}
        data-activity={row.activity ?? 'unknown'}
        data-state={row.state}
        onClick={() => onOpen(row.address)}
        className="flex w-full min-w-0 flex-col items-center text-center cursor-pointer"
        style={{ padding: 0, background: 'transparent', color: 'var(--zen-text)', opacity: row.state !== 'ok' ? 0.72 : 1 }}
      >
        <span
          className="relative inline-flex"
          data-zen-pin-avatar=""
          style={{
            marginBottom: 6,
            borderRadius: 999,
            boxShadow: row.selected ? '0 0 0 2px var(--zen-surface), 0 0 0 4px var(--zen-accent)' : 'none',
          }}
        >
          <ZenAgentAvatar
            label={row.label}
            url={row.avatarUrl}
            size={avatarPx}
            agentColor={row.color}
            background="var(--zen-bubble-agent)"
            color="var(--zen-bubble-agent-text)"
            fontSize={`${Math.max(12, Math.round(avatarPx * 0.35))}px`}
          />
          <span className="absolute" style={{ right: 1, bottom: 1, lineHeight: 0 }}>
            <ZenStatusDot row={row} size={dotPx} />
          </span>
        </span>
        <span className="block w-full truncate" data-zen-pin-name="" style={{ fontSize: '0.8em', maxWidth: 110 }}>
          {row.label}
        </span>
        {busy && row.activity && (
          <span className="block w-full truncate" data-zen-pin-activity={row.activity} style={{ fontSize: '0.72em', color: tone, maxWidth: 110 }}>
            {ACTIVITY_TEXT[row.activity]}
          </span>
        )}
      </button>
    </li>
  )
}

/** Garden 1's pin grid (`pins`): three across, more rows as you pin more;
 *  one or two pins centred, a partial last row left-aligned. */
function PinGrid({ rows, onOpen }: { rows: ZenAgentRow[]; onOpen(address: string): void }): React.JSX.Element {
  const ref = useRef<HTMLUListElement | null>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const measure = (): void => {
      const cs = typeof getComputedStyle === 'function' ? getComputedStyle(el) : null
      const pad = cs ? (parseFloat(cs.paddingLeft) || 0) + (parseFloat(cs.paddingRight) || 0) : 0
      setWidth(el.clientWidth - pad)
    }
    measure()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  const avatarPx = zenPinAvatarPx(width)
  return (
    <ul
      ref={ref}
      role="listbox"
      aria-label="Pinned agents"
      data-zen-pins=""
      data-zen-pin-count={rows.length}
      className="grid flex-shrink-0"
      style={{
        gridTemplateColumns: zenPinGridColumns(rows.length),
        justifyContent: zenPinGridCentred(rows.length) ? 'center' : undefined,
        gap: `${ZEN_PIN_ROW_GAP}px ${ZEN_PIN_COL_GAP}px`,
        padding: `6px 18px ${ZEN_PIN_GRID_BOTTOM_PX}px`,
        margin: 0,
      }}
    >
      {rows.map((row) => (
        <PinCell key={row.address} row={row} avatarPx={avatarPx} onOpen={onOpen} />
      ))}
    </ul>
  )
}

/** The list's last row: Add agent (bottom left of the widget). */
function AddAgentRow({ bridge }: { bridge: ZenWidgetBridge }): React.JSX.Element {
  const { ref, onClick } = useZenAddAgentClick(bridge)
  return (
    <div className="flex flex-shrink-0 items-center" data-zen-agents-footer="" style={{ padding: '0 8px 10px' }}>
      <button
        ref={ref}
        type="button"
        aria-haspopup="dialog"
        title="Add an agent to this Home"
        onClick={onClick}
        data-zen-add-agent=""
        data-zen-soft-button=""
        className="no-drag flex items-center gap-3 text-left cursor-pointer"
        style={{
          padding: '8px 12px',
          borderRadius: 'var(--zen-radius)',
          color: 'var(--zen-text-muted)',
          background: 'transparent',
        }}
      >
        <span
          aria-hidden
          className="flex flex-shrink-0 items-center justify-center"
          style={{ width: 28, height: 28, borderRadius: 999, border: '1.5px dashed var(--zen-border)' }}
        >
          <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden>
            <path d="M6 1.5v9M1.5 6h9" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          </svg>
        </span>
        <span style={{ fontWeight: 600, fontSize: '0.9em' }}>Add agent</span>
      </button>
    </div>
  )
}

export function ZenAgentsWidget({ bridge, decl }: ZenWidgetProps): React.JSX.Element {
  const rows = useZenRows(bridge)
  const nowSec = useNowSec()
  const [homeId, pickHome] = useWidgetHome(bridge, rows)
  const workspaces = zenAgentsSource(decl.props) === 'workspaces'
  const groups = useFocusGroups(bridge, workspaces)
  const picker = !workspaces && homePickerOn(decl.props)
  // `mode: "home"` shows the whole Home even when `agent` is set.
  const agent = decl.props.mode === 'home' ? null : propString(decl.props, 'agent')
  // Home names only when they show (the picker, or the single-agent note).
  const homes = picker || (agent && rows.length === 0) ? bridge.homes.list() : []
  const home = homes.find((h) => h.id === homeId) ?? null
  // 0.45.6 Garden 1: the pin grid (`pins`, not in one-agent mode) and
  // "Recent agents first" (the Garden's own setting, this computer).
  // 0.46.0: `recent-first` is the Garden's default (the person's own
  // choice on this computer wins); `pin-limit` caps the grid. Both unset on
  // an older daemon: today's behaviour.
  const recentFirstDefault = zenRecentFirstProp(decl.props)
  const recentFirst = useZenRecentFirst(recentFirstDefault)
  const sent = useZenRecentFirstStore((s) => s.sent)
  const pinsOn = decl.props.pins === true && !agent
  const pinLimit = zenPinLimit(decl.props)
  const { pins, list } = zenAgentsDisplay(rows, { grid: pinsOn, recentFirst, sent, limit: pinLimit })
  const settingsOn = decl.props.settings === true
  // ⌘↑ / ⌘↓ walk what's drawn: the grid, then the list.
  const rowsRef = useRef(rows)
  rowsRef.current = [...pins, ...list]
  const open = (address: string): void => {
    void Promise.resolve()
      .then(() => bridge.call('conversation.open', address))
      .catch((err: unknown) => console.warn('[zen] open conversation failed:', err))
  }
  const openRef = useRef(open)
  openRef.current = open
  // A row the person clicked: open it, and put the caret in its box.
  const pick = (address: string): void => {
    requestZenComposeFocus(address)
    open(address)
  }

  // An agent added through this widget's Add agent: open it the same way.
  const gardenId = bridge.gardens.current()?.id ?? ''
  const added = useZenAddAgentStore((s) => s.added)
  const handledAdd = useRef(added?.seq ?? 0)
  useEffect(() => {
    if (!added || added.seq === handledAdd.current) return
    handledAdd.current = added.seq
    if (added.viewKey !== zenGardenHomeKey(gardenId, decl.id)) return
    requestZenComposeFocus(added.address)
    openRef.current(added.address)
  }, [added, gardenId, decl.id])

  // Z32: ⌘↑ / ⌘↓ move between conversations.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (!e.metaKey || e.shiftKey || e.altKey || e.ctrlKey) return
      if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return
      const list = rowsRef.current
      if (list.length === 0) return
      e.preventDefault()
      const at = list.findIndex((r) => r.selected)
      const next =
        at < 0 ? (e.key === 'ArrowDown' ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, at + (e.key === 'ArrowDown' ? 1 : -1)))
      if (next !== at) openRef.current(list[next].address)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

  return (
    <div
      className="flex h-full min-h-0 w-full flex-col"
      data-zen-widget="agents"
      data-zen-widget-id={decl.id}
      data-zen-agents-mode={agent ? 'agent' : 'home'}
      data-zen-agents-source={workspaces ? 'workspaces' : 'home'}
      data-zen-agents-home={homeId ?? undefined}
    >
      <ZenWidgetStyles />
      <div
        className="flex flex-shrink-0 items-center justify-between gap-3"
        style={{
          padding: '14px 18px 6px',
          fontSize: '0.78em',
          fontWeight: 600,
          letterSpacing: '0.04em',
          textTransform: 'uppercase',
          color: 'var(--zen-text-muted)',
        }}
      >
        <span>{agent ? 'Agent' : 'Agents'}</span>
        {picker && <HomePicker homes={homes} current={home} onPick={pickHome} />}
        {groups?.enabled && <FocusGroupPicker bridge={bridge} groups={groups} />}
        {settingsOn && (
          <ZenGardenSettingsButton
            gardenId={gardenId || null}
            pinsOn={pinsOn}
            pinLimit={pinLimit}
            recentFirstDefault={recentFirstDefault}
          />
        )}
      </div>
      {rows.length === 0 ? (
        <div
          className="flex flex-1 items-center justify-center text-center"
          data-zen-agents-empty=""
          style={{ padding: 24, color: 'var(--zen-text-muted)' }}
        >
          {workspaces
            ? groups?.enabled
              ? ZEN_EMPTY_FOCUS_GROUP
              : ZEN_EMPTY_SERVER
            : agent
              ? zenAgentNotOnHome(agent, home?.name ?? 'this Home')
              : ZEN_EMPTY_HOME}
        </div>
      ) : (
        // One scroller: the pin grid scrolls away with the list.
        <div data-zen-agents-scroll="" className="flex min-h-0 flex-1 flex-col overflow-y-auto">
          {pins.length > 0 && <PinGrid rows={pins.map((row) => displayRow(row, decl.props))} onOpen={pick} />}
          <ul
            role="listbox"
            aria-label="Agents"
            data-zen-agents-order={recentFirst ? 'recent-first' : 'home'}
            className="flex flex-shrink-0 flex-col"
            style={{ padding: '2px 8px 10px', gap: 2, margin: 0 }}
          >
            {list.map((row) => (
              <AgentRow key={row.address} row={displayRow(row, decl.props)} nowSec={nowSec} onOpen={pick} />
            ))}
          </ul>
        </div>
      )}
      {!agent && !workspaces && bridge.caps.has('agents:add') && <AddAgentRow bridge={bridge} />}
    </div>
  )
}
