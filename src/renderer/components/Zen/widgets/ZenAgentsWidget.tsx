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

import { useEffect, useRef, useState } from 'react'
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
  const rowsRef = useRef(rows)
  rowsRef.current = rows
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
        <ul
          role="listbox"
          aria-label="Agents"
          className="flex min-h-0 flex-1 flex-col overflow-y-auto"
          style={{ padding: '2px 8px 10px', gap: 2 }}
        >
          {rows.map((row) => (
            <AgentRow key={row.address} row={displayRow(row, decl.props)} nowSec={nowSec} onOpen={pick} />
          ))}
        </ul>
      )}
      {!agent && !workspaces && bridge.caps.has('agents:add') && <AddAgentRow bridge={bridge} />}
    </div>
  )
}
