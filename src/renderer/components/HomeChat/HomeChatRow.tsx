// A person or channel row on Home (prd-home-people-channels-v1 §5.4, §15).
//
//   - Person: their initial, their name, a server chip when they're on
//     another server than this window's or on more than one of yours, the
//     unread badge (a DM's number, §15.1).
//   - Channel: `#` (a lock for invite-only), its name, the server chip, a dot
//     while unread (a number once Sew counts, `unread_count`).
//   - A server that can't be reached greys the row out with its last badge,
//     as agent rows do. A server without chat says so.
//
// Clicking opens the chat in the main area. The menu: Pin / Unpin (pins and
// "Most recent at the top" are shared with agent rows), "Also on" another
// server for a person (§6.3), and Remove from <Home> (a DM can't be left in
// Sew: removing hides the row, D10).

import { useEffect, useMemo, type JSX } from 'react'
import { showContextMenu } from '@/lib/context-menu'
import type { ContextMenuItemDef } from '@/stores/context-menu'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey } from '@/lib/home-address'
import { homeChatTarget, personAddress, type HomeChatTarget } from '@/lib/home-chat/address'
import { matchesElsewhere } from '@/lib/home-chat/people'
import type { HomeChatPerson } from '@/lib/home-chat/api'
import {
  homeChatRowBadge,
  refreshHomeChatPeople,
  useHomeChatHost,
  useHomeChatStore,
} from '@/stores/home-chat'
import { MAX_PINS, isPinned, pinnedRows } from '@/lib/home-row-order'
import { AgentRowButton } from '@/components/Sidebar/Sidebar'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { UnreadBadge } from '@/components/common/UnreadBadge'
import { MachineChip } from '@/components/Home/MachineChip'
import { HomeRowPinMark } from '@/components/Home/groups/HomeRowCaption'
import { serverLabelFor } from './serverLabel'

const MUTED = 'var(--color-text-muted)'

/** Pin / Unpin, as on an agent row (HomeSidebar's `pinMenuItem`): a tenth
 *  pin is offered but off, saying why. */
function pinMenuItem(home: Home, row: HomeRow): ContextMenuItemDef {
  if (isPinned(row)) return { id: 'home-unpin', label: 'Unpin' }
  const n = pinnedRows(home).length
  return n >= MAX_PINS
    ? { id: 'home-pin', label: 'Pin', enabled: false, hint: `Up to ${MAX_PINS} pins` }
    : { id: 'home-pin', label: 'Pin', shortcut: `⌘${n + 1}` }
}

/** Open a person or channel row's chat (this window's main area). */
export function openHomeChatRow(row: HomeRow): boolean {
  const t = homeChatTarget(row)
  if (!t) return false
  useHomeChatStore.getState().openChat(t)
  return true
}

/** Every saved server's cached people, for "Also on" and the chip. */
function usePeopleByHost(): Record<string, HomeChatPerson[] | undefined> {
  const byHost = useHomeChatStore((s) => s.byHost)
  return useMemo(() => {
    const out: Record<string, HomeChatPerson[] | undefined> = {}
    for (const [k, v] of Object.entries(byHost)) out[k] = v.people?.people
    return out
  }, [byHost])
}

async function homeChatRowMenu(
  e: React.MouseEvent,
  home: Home,
  row: HomeRow,
  target: HomeChatTarget,
  also: Array<{ hostKey: string; person: HomeChatPerson }>,
  hosts: ReturnType<typeof useConnectHostStore.getState>['hosts'],
): Promise<void> {
  e.preventDefault()
  const items: ContextMenuItemDef[] = []
  if (target.kind === 'person' && also.length > 0) {
    for (const [i, a] of also.entries()) {
      items.push({ id: `also-${i}`, label: `Also on ${serverLabelFor(a.hostKey, hosts)}: switch to it` })
    }
    items.push({ id: 'sep-also', label: '', type: 'separator' })
  }
  items.push(pinMenuItem(home, row))
  items.push({ id: 'home-remove', label: `Remove from ${home.name}` })
  const clicked = await showContextMenu(items)
  if (clicked === 'home-pin') useHomesStore.getState().pinRow(home.id, row.address)
  else if (clicked === 'home-unpin') useHomesStore.getState().unpinRow(home.id, row.address)
  else if (clicked === 'home-remove') {
    useHomesStore.getState().removeRow(home.id, row.address)
    if (useHomeChatStore.getState().open?.address === row.address) useHomeChatStore.getState().closeChat()
  } else if (clicked?.startsWith('also-')) {
    const pick = also[Number(clicked.slice(5))]
    const address = pick ? personAddress(pick.person.ext, pick.hostKey) : null
    if (address) {
      // Same place and pin on this Home; the other server's DM is its own
      // conversation (§6.3).
      useHomesStore.getState().replaceRow(home.id, row.address, { kind: 'person', address, workspaceId: null, label: pick.person.displayName })
      if (useHomeChatStore.getState().open?.address === row.address) openHomeChatRow({ kind: 'person', address, workspaceId: null, label: '' })
    }
  }
}

/** `index` is the row's ⌘ number slot (unused: chat rows show no agent
 *  status cluster; ⌘N still opens them through the Home shortcuts). */
export function HomeChatHomeRow({ home, row }: { home: Home; row: HomeRow; index?: number }): JSX.Element | null {
  const target = homeChatTarget(row)
  const hostKey = target?.hostKey ?? null
  const host = useHomeChatHost(hostKey)
  const open = useHomeChatStore((s) => s.open)
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const hosts = useConnectHostStore((s) => s.hosts)
  const peopleByHost = usePeopleByHost()
  useEffect(() => {
    if (hostKey && target?.kind === 'person' && host?.status?.on) void refreshHomeChatPeople(hostKey)
  }, [hostKey, target?.kind, host?.status?.on])
  if (!target || !hostKey) return null

  const person = target.kind === 'person' ? (host?.people?.people.find((p) => p.ext === target.ext) ?? null) : null
  const name = person?.displayName ?? row.label
  const also = person ? matchesElsewhere(person, hostKey, peopleByHost) : []
  const badge = homeChatRowBadge(host, target)
  const unreachable = !!host?.statusError
  const noChat = host?.status !== null && host?.status !== undefined && !host.status.on
  const greyed = unreachable || noChat || host?.summary?.chat === 'unavailable'
  // The chip: another server than this window's, or (a person) on more than
  // one of your servers.
  const showChip = hostKey !== connectedKey || also.length > 0
  const place = serverLabelFor(hostKey, hosts)
  const isActive = open?.address === row.address
  const title = noChat
    ? `No chat on ${place}`
    : unreachable
      ? `${place} can’t be reached`
      : target.kind === 'person'
        ? `Message ${name} on ${place}`
        : `Open #${name} on ${place}`

  return (
    <div className="no-drag" data-home-chat-row={target.kind} data-testid="home-chat-row">
      <AgentRowButton
        isActive={isActive}
        color={MUTED}
        dimmed={greyed}
        onClick={() => {
          if (!noChat) openHomeChatRow(row)
        }}
        onContextMenu={(e) => void homeChatRowMenu(e, home, row, target, also, hosts)}
        title={title}
        avatar={
          target.kind === 'person' ? (
            <ProjectAvatar
              projectPath={`home-chat:${row.address}`}
              projectName={name}
              projectColor={MUTED}
              iconUrl={person?.picture ?? null}
              size={32}
              fetchIcon={false}
            />
          ) : (
            <span
              aria-hidden="true"
              className="inline-flex h-8 w-8 flex-shrink-0 items-center justify-center border border-[var(--color-border)] text-[14px] text-[var(--color-text-muted)]"
              data-channel-glyph=""
            >
              #
            </span>
          )
        }
        name={name}
        nameMark={isPinned(row) ? <HomeRowPinMark /> : undefined}
        nameAside={
          badge.kind === 'none' ? undefined : (
            <span className="mr-1 inline-flex items-center" data-home-chat-badge={badge.kind}>
              <UnreadBadge
                unread={badge.kind === 'count' ? badge.n : 1}
                variant={badge.kind === 'dot' ? 'dot' : 'count'}
                label={target.kind === 'person' ? 'messages from this person' : 'channel messages'}
              />
            </span>
          )
        }
        subline={
          <div className="flex min-w-0 flex-1 items-center gap-0.5 overflow-hidden">
            {showChip && <MachineChip place={place} reach={unreachable ? 'off' : undefined} />}
            {noChat && <span className="text-[10px] text-[var(--color-text-muted)]">No chat on this server</span>}
          </div>
        }
      />
    </div>
  )
}
