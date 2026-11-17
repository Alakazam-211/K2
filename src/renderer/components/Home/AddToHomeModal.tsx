// The Add to Home dialog (K2 Design's Home nav mock): + in the nav's footer,
// + in the rail and the empty group's + open it on the current group.
// K2's dialog shape (DialogScrim, then DialogFrame's Surface: elevation 5,
// the box radius, cut in Terminal), centred, at most 560 × 540, kept 16px
// from the window's sides and 24px from its top and bottom; below 640px of
// room across it is a full-height sheet.
//
// Head: "Add to Home", then "Adds to <group>" in plain words (no group
// number or glyph in the line): it always adds to the
// current group. A tab bar between that line and the search appears only
// once there is more than one kind to add (`addToHomeSections`): Agents,
// and People and Channels once a server this window knows has Home chat
// (prd-home-people-channels-v1 S6; `useHomeChatOffered`). It is K2's
// segmented control, hugging its labels and left-aligned (the Groups
// dialog's Agents | Icon); ←/→ on it switches tabs. The last tab is kept
// per Home in this window (`rememberAddToHomeTab`): the next open lands there.
//
// People: everyone you share a server with (owner, admin, member and Voice
// & Chat only logins), never App users, disabled logins or agents. Channels:
// the channels you can see, or New channel. Their sections
// (`components/HomeChat/HomeChatPickers`) hand a picked row to `onAddRow`;
// the dialog adds it to the group and closes, as for an agent.
//
// Agents: one search ("Search agents...") over one list with a section per
// source: this computer (the connected server), then each saved server
// (its agents, Sign in, or Offline), from `useAddableAgents` (today's Add
// Agent picker's sources). A row shows the agent's picture, name and
// handle; on hover, "Add". One already in the group says "✓ In <group>" and
// can't be picked. Picking adds it and closes the dialog (add one, close);
// the opener gets focus back and the new row is scrolled into view with a
// brief wash.
//
// Keyboard: focus starts in the search. The list is one tab stop: ↓ from
// the search enters it, ↑/↓ move between the agents that can be added,
// Enter adds. Esc (after the search clears), a press outside, or the ×
// closes it.

import { useCallback, useEffect, useId, useMemo, useRef, useState, type JSX } from 'react'
import { createPortal } from 'react-dom'
import { CheckIcon } from 'lucide-react'
import { DialogFrame, DialogScrim } from '@/components/ui/Dialog'
import { SearchField } from '@/components/ui/SearchField'
import { DIALOG_EDGE_TINT, ScrollEdgeFrame } from '@/components/ui/ScrollEdgeFade'
import { Segmented } from '@/components/ui/Segmented'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { K2_ICON_STROKE } from '@/components/icons/K2Icons'
import { selectedHome, useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useHomeChatOffered } from '@/stores/home-chat'
import { isWebClient } from '@/lib/is-web'
import { AddChannelPanel, AddPersonPanel, NewChannelPanel } from '@/components/HomeChat/HomeChatPickers'
import { useHomeAddPickerStore } from './home-room'
import {
  AddSourceHead,
  SectionStatus,
  addableMatches,
  useAddableAgents,
  type AddableAgent,
  type HomeAvatarLookup,
} from './HomeAddPanels'

/** The kinds a Home holds. */
export type AddToHomeSection = 'agents' | 'people' | 'channels'

/** Every section in tab order. With one shown, there is no tab bar. */
export const ADD_TO_HOME_SECTIONS: ReadonlyArray<{ id: AddToHomeSection; label: string }> = [
  { id: 'agents', label: 'Agents' },
  { id: 'people', label: 'People' },
  { id: 'channels', label: 'Channels' },
]

/** The sections shown: People and Channels only where Home chat is on. */
export function addToHomeSections(chat: boolean): ReadonlyArray<{ id: AddToHomeSection; label: string }> {
  return chat ? ADD_TO_HOME_SECTIONS : ADD_TO_HOME_SECTIONS.filter((s) => s.id === 'agents')
}

const TAB_KEY = 'k2.addToHome.tab.'

/** The tab Add to Home last showed on `homeId` in this window (a per-viewer
 *  convenience, so local storage); null when none or storage is off. */
export function rememberedAddToHomeTab(homeId: string): AddToHomeSection | null {
  try {
    const v = globalThis.localStorage?.getItem(TAB_KEY + homeId)
    return v === 'agents' || v === 'people' || v === 'channels' ? v : null
  } catch {
    return null
  }
}

/** Keep `tab` as `homeId`'s last Add to Home tab. */
export function rememberAddToHomeTab(homeId: string, tab: AddToHomeSection): void {
  try {
    globalThis.localStorage?.setItem(TAB_KEY + homeId, tab)
  } catch {
    // Storage off (a private window): the dialog opens on Agents next time.
  }
}

/** The dialog's most, and its margins from the window (the Groups dialog's). */
export const ADD_TO_HOME_MAX = { width: 560, height: 540 } as const
export const ADD_TO_HOME_MARGIN = { x: 16, y: 24 } as const
/** Below this much room across, the dialog is a full-height sheet. */
export const ADD_TO_HOME_SHEET_BELOW_PX = 640

/** Width and height: the most, clamped to the window less the margins;
 *  under `ADD_TO_HOME_SHEET_BELOW_PX` of room across (a window under 672px),
 *  a sheet: the window's full width and height less the margins. Literal classes, so Tailwind sees them; a test keeps
 *  them in step with the numbers above. */
export const ADD_TO_HOME_SIZE_CLASS =
  'w-[min(560px,calc(var(--k2-vw)-32px))] h-[min(540px,calc(var(--k2-vh)-48px))] max-[671px]:w-[calc(var(--k2-vw)-32px)] max-[671px]:h-[calc(var(--k2-vh)-48px)]'

export interface AddToHomeModalProps {
  home: Home
  /** The section it opens on (default: the last one shown on this Home,
   *  else Agents). */
  section?: AddToHomeSection
  onClose: () => void
  /** A row was added (its address). The dialog closes right after. */
  onAdded?: (address: string) => void
  /** A person or channel row picked (default: add it to `home`). True when
   *  it went on the group. */
  onAddRow?: (row: HomeRow) => boolean
  avatarFor?: HomeAvatarLookup
}

const ROW_CLASS =
  'group/add k2-focus no-drag flex w-full items-center gap-2.5 px-2 py-1.5 text-left outline-none rounded-[var(--radius-selector)]'

function AgentRow({
  agent,
  group,
  tabbable,
  onAdd,
}: {
  agent: AddableAgent
  group: string
  tabbable: boolean
  onAdd: (a: AddableAgent) => void
}): JSX.Element {
  const taken = agent.inGroup
  return (
    <button
      type="button"
      role="option"
      aria-selected={false}
      aria-disabled={taken ? true : undefined}
      aria-label={taken ? `${agent.name}, already in ${group}` : `${agent.name}, add to ${group}`}
      title={taken ? `Already in ${group}` : `Add ${agent.name} to ${group}`}
      tabIndex={tabbable ? 0 : -1}
      data-add-agent={agent.value}
      data-taken={taken ? '' : undefined}
      className={`${ROW_CLASS} ${
        taken
          ? 'cursor-default text-[var(--color-text-secondary)]'
          : 'cursor-pointer text-[var(--color-text-secondary)] hover:bg-[var(--color-row-hover)] focus-visible:bg-[var(--color-row-hover)]'
      }`}
      onClick={() => {
        if (!taken) onAdd(agent)
      }}
    >
      <ProjectAvatar
        projectPath={agent.avatar.kind === 'workspace' ? agent.avatar.path : agent.value}
        projectName={agent.name}
        projectColor={agent.avatar.kind === 'workspace' ? agent.avatar.color : 'var(--color-text-muted)'}
        projectId={agent.avatar.kind === 'workspace' ? agent.avatar.id : undefined}
        iconUrl={agent.avatar.kind === 'workspace' ? agent.avatar.iconUrl : null}
        size={32}
        fetchIcon={agent.avatar.kind === 'workspace' ? agent.avatar.fetchIcon : false}
      />
      <span className="flex min-w-0 flex-1 flex-col gap-px">
        <span className={`truncate type-name font-semibold ${taken ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`}>
          {agent.name}
        </span>
        <span className="truncate type-hint text-[var(--color-text-muted)] group-hover/add:text-[var(--color-text-secondary)]">{agent.handle}</span>
      </span>
      {taken ? (
        <span className="flex flex-shrink-0 items-center gap-1 type-hint text-[var(--color-text-secondary)]" data-add-in-group="">
          <CheckIcon size={14} strokeWidth={K2_ICON_STROKE} aria-hidden="true" />
          In {group}
        </span>
      ) : (
        <span aria-hidden="true" className="invisible flex-shrink-0 type-hint text-[var(--color-text-secondary)] group-hover/add:visible group-focus-visible/add:visible" data-add-go="">
          Add
        </span>
      )}
    </button>
  )
}

/** The Agents section: search, then every source's agents. */
export function AddAgentsPanel({
  home,
  onAdded,
  onCancel,
  avatarFor,
  autoFocus = true,
}: {
  home: Home
  onAdded: (address: string) => void
  /** Esc in the list (a listbox keeps Esc from the dialog's own handler). */
  onCancel: () => void
  avatarFor?: HomeAvatarLookup
  /** Focus the search on mount (not when ←/→ on the tab bar showed it). */
  autoFocus?: boolean
}): JSX.Element {
  const { sources, add } = useAddableAgents(home, avatarFor)
  const [query, setQuery] = useState('')
  const searchRef = useRef<HTMLInputElement>(null)
  const listRef = useRef<HTMLDivElement>(null)
  const listId = useId()

  useEffect(() => {
    if (!autoFocus) return
    const raf = requestAnimationFrame(() => searchRef.current?.focus())
    return () => cancelAnimationFrame(raf)
  }, [autoFocus])

  const shown = useMemo(() => {
    const q = query.trim()
    return sources
      .map((s) => ({ ...s, agents: q ? s.agents.filter((a) => addableMatches(a, q)) : s.agents }))
      // A source with no matches while searching goes; with no search it
      // keeps its line (Sign in, Offline, Loading…).
      .filter((s) => s.agents.length > 0 || (!q && s.listing.kind !== 'ok'))
  }, [sources, query])
  const firstFree = shown.flatMap((s) => s.agents).find((a) => !a.inGroup)?.value

  const pick = useCallback(
    (a: AddableAgent) => {
      const address = add(a.value)
      if (address) onAdded(address)
    },
    [add, onAdded],
  )

  /** ↑/↓ between the agents that can be added (the taken ones are skipped). */
  const move = (from: HTMLElement | null, dir: 1 | -1): void => {
    const list = listRef.current
    if (!list) return
    const rows = Array.from(list.querySelectorAll<HTMLElement>('[data-add-agent]:not([data-taken])'))
    if (rows.length === 0) return
    const i = from ? rows.indexOf(from) : -1
    const next = i < 0 ? (dir > 0 ? rows[0] : rows[rows.length - 1]) : rows[Math.min(rows.length - 1, Math.max(0, i + dir))]
    next.focus()
    next.scrollIntoView({ block: 'nearest' })
  }

  return (
    <>
      <div className="flex-shrink-0 px-5 pt-0.5 relative z-[2]" data-edge-header="">
        <SearchField
          ref={searchRef}
          type="text"
          aria-label="Search agents"
          aria-controls={listId}
          placeholder="Search agents..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown') {
              e.preventDefault()
              move(null, 1)
            }
          }}
          spellCheck={false}
          autoComplete="off"
          className="w-full"
          data-add-search=""
        />
      </div>
      <ScrollEdgeFrame className="min-h-0 flex-1" scrollClassName="overflow-y-auto overflow-x-hidden px-3 pt-3.5 pb-3" tint={DIALOG_EDGE_TINT} heavy testId="add-to-home-list">
        <div
          ref={listRef}
          id={listId}
          role="listbox"
          aria-label="Agents"
          data-add-list=""
          onKeyDown={(e) => {
            if (e.key === 'Escape' && !e.defaultPrevented) {
              e.preventDefault()
              onCancel()
              return
            }
            if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp') return
            if (!(e.target instanceof HTMLElement) || !e.target.hasAttribute('data-add-agent')) return
            e.preventDefault()
            move(e.target, e.key === 'ArrowDown' ? 1 : -1)
          }}
        >
          {shown.map((s) => (
            <div key={s.key} role="group" aria-label={s.label} data-add-source={s.key}>
              <AddSourceHead label={s.label} sub={s.sub} sourceKey={s.key} />
              {s.listing.kind !== 'ok' && (
                <div data-add-source-status={s.listing.kind}>
                  <SectionStatus listing={s.listing} label={s.label} onRetry={s.retry} />
                </div>
              )}
              {s.agents.map((a) => (
                <AgentRow key={a.value} agent={a} group={home.name} tabbable={a.value === firstFree} onAdd={pick} />
              ))}
            </div>
          ))}
          {shown.length === 0 && (
            <p role="status" className="m-0 px-3 py-6 text-center type-hint text-[var(--color-text-muted)]" data-add-empty="">
              {query.trim() ? `No agents match “${query.trim()}”` : 'No agents to add'}
            </p>
          )}
        </div>
      </ScrollEdgeFrame>
    </>
  )
}

export function AddToHomeModal({ home, section, onClose, onAdded, onAddRow, avatarFor }: AddToHomeModalProps): JSX.Element {
  const titleId = useId()
  const sections = addToHomeSections(useHomeChatOffered() && !isWebClient())
  const [picked, setPicked] = useState<AddToHomeSection>(() => section ?? rememberedAddToHomeTab(home.id) ?? 'agents')
  const tab = sections.some((s) => s.id === picked) ? picked : 'agents'
  // ←/→ on the tab bar keeps focus there (so the next arrow moves on); a
  // click, or the first open, puts it in the panel's search.
  const viaKeys = useRef(false)
  const [panelFocus, setPanelFocus] = useState(true)
  const setTab = useCallback(
    (next: AddToHomeSection) => {
      setPanelFocus(!viaKeys.current)
      viaKeys.current = false
      setPicked(next)
      rememberAddToHomeTab(home.id, next)
    },
    [home.id],
  )
  const [channelMode, setChannelMode] = useState<'pick' | 'new'>('pick')
  const added = useCallback(
    (address: string) => {
      onAdded?.(address)
      onClose()
    },
    [onAdded, onClose],
  )
  // A person or channel: add one, then close (as for an agent).
  const addRow = useCallback(
    (row: HomeRow): boolean => {
      const ok = onAddRow ? onAddRow(row) : useHomesStore.getState().addRow(home.id, row)
      if (ok) added(row.address)
      return ok
    },
    [onAddRow, home.id, added],
  )
  return createPortal(
    <>
      <DialogScrim data-add-to-home-scrim="" />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-add-to-home-modal=""
        data-section={tab}
        onCancel={onClose}
        className={`flex flex-col overflow-hidden ${ADD_TO_HOME_SIZE_CLASS}`}
      >
        <div className="flex-shrink-0 pt-4 pr-12 pb-2.5 pl-5" data-add-to-home-head="">
          <h2 id={titleId} className="m-0 text-[length:var(--type-xl)] font-semibold text-[var(--color-text-primary)]">
            Add to Home
          </h2>
          <p className="m-0 mt-1 flex min-w-0 items-center gap-1.5 type-hint text-[var(--color-text-secondary)]" data-add-to="">
            Adds to <b className="min-w-0 truncate font-semibold text-[var(--color-text-primary)]">{home.name}</b>
          </p>
        </div>
        {sections.length > 1 && (
          // Hugs its labels, left-aligned (the Groups dialog's Agents | Icon).
          <div
            className="flex flex-shrink-0 justify-start px-5 pb-2.5"
            data-add-to-home-tabs=""
            onKeyDownCapture={(e) => {
              viaKeys.current = e.key.startsWith('Arrow')
            }}
            onPointerDownCapture={() => {
              viaKeys.current = false
            }}
          >
            <Segmented
              aria-label="Add to Home: show"
              value={tab}
              onChange={setTab}
              options={sections.map((s) => ({
                value: s.id,
                label: s.label,
                buttonProps: { 'data-testid': `add-to-home-tab-${s.id}`, 'data-add-to-home-tab': s.id },
              }))}
            />
          </div>
        )}
        <div className="flex min-h-0 flex-1 flex-col" data-add-to-home-panel={tab}>
          {tab === 'agents' && <AddAgentsPanel home={home} onAdded={added} onCancel={onClose} avatarFor={avatarFor} autoFocus={panelFocus} />}
          {tab === 'people' && <AddPersonPanel home={home} onAddRow={addRow} autoFocus={panelFocus} />}
          {tab === 'channels' && (
            <>
              <div className="flex flex-shrink-0 justify-start px-5 pb-2" data-add-channel-mode={channelMode}>
                <Segmented
                  aria-label="Channels: add or make"
                  value={channelMode}
                  onChange={setChannelMode}
                  options={[
                    { value: 'pick', label: 'Add a channel', buttonProps: { 'data-testid': 'add-to-home-channels-pick' } },
                    { value: 'new', label: 'New channel', buttonProps: { 'data-testid': 'add-to-home-channels-new' } },
                  ]}
                />
              </div>
              {channelMode === 'pick' ? (
                <AddChannelPanel home={home} onAddRow={addRow} autoFocus={panelFocus} />
              ) : (
                <NewChannelPanel home={home} onAddRow={addRow} autoFocus={panelFocus} />
              )}
            </>
          )}
        </div>
      </DialogFrame>
    </>,
    document.body,
  )
}

/** How long a just-added row keeps its wash in the list. */
export const JUST_ADDED_WASH_MS = 1600

/** Close Add to Home and put focus back on what opened it (else the nav's
 *  +, else the rail's). */
export function closeAddToHome(): void {
  const st = useHomeAddPickerStore.getState()
  const opener = st.opener
  st.setOpen(false)
  const back =
    opener && opener.isConnected
      ? opener
      : document.querySelector<HTMLElement>('[data-home-add-open="footer"], [data-home-add-open="rail"]')
  back?.focus()
}

/** Add to Home while it is open, on the current group. The nav and the
 *  rail each mount one (only one of them is on screen). */
export function AddToHomeHost(): JSX.Element | null {
  const open = useHomeAddPickerStore((s) => s.open)
  const home = useHomesStore(selectedHome)
  if (!open) return null
  return (
    <AddToHomeModal
      home={home}
      onClose={closeAddToHome}
      onAdded={(address) => {
        const st = useHomeAddPickerStore.getState()
        st.setJustAdded(address)
        // The new row, in view once it has drawn.
        requestAnimationFrame(() => {
          document.querySelector(`[data-home-row="${CSS.escape(address)}"]`)?.scrollIntoView({ block: 'nearest' })
        })
        setTimeout(() => {
          if (useHomeAddPickerStore.getState().justAdded === address) useHomeAddPickerStore.getState().setJustAdded(null)
        }, JUST_ADDED_WASH_MS)
      }}
    />
  )
}
