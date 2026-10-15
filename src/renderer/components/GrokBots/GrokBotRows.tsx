// Grok Bot rows (0.45.7): the Agents sidebar's "Grok Bots" group for the
// connected server, and a Home row for a Grok Bot on any saved server.
//
// Both paint through the shared `AgentRowButton` with the "Grok Bot" tag.
// Clicking opens the chat in the main area (no terminal, no session). The
// right-click menu offers only what applies: Add to Home on Agents, and
// Remove from <Home> on Home — never workspace items (settings, wiki,
// Finder, worktrees, clone, pin).

import { showContextMenu } from '@/lib/context-menu'
import { addAgentToHomeFromMenu } from '@/lib/add-to-home'
import { ADD_TO_HOME_MENU_ID, addToHomeMenuItem } from '@/stores/add-to-home-dialog'
import { useHomesStore, type Home, type HomeRow } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { activeHomeHostKey, parseHomeAddress } from '@/lib/home-address'
import { isWebClient } from '@/lib/is-web'
import {
  grokAgentState,
  grokBotHomeHandle,
  grokBotIdFromAddress,
  GROK_STATE_LABEL,
  type GrokAgentState,
  type GrokBotAgent,
  type GrokBotStatus,
} from '@/lib/grokbots'
import {
  grokAgentOn,
  grokSupported,
  scopeForGrokHost,
  useGrokBotsStore,
  useGrokBotsSync,
} from '@/stores/grokbots'
import { primaryScope } from '@/kessel/server-scope'
import { AgentRowButton, ShortcutIndexBadge } from '@/components/Sidebar/Sidebar'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { useHomeRowAvatar } from '@/lib/home-avatars'
import { GrokBotTag } from './GrokBotTag'

const STATE_TEXT: Record<GrokAgentState, string> = {
  working: 'text-[var(--color-status-working)]',
  'needs-you': 'text-[var(--color-status-warn-amber)]',
  idle: 'text-[var(--color-text-muted)]',
  'not-running': 'text-[var(--color-text-muted)]',
}

const FALLBACK_COLOR = 'var(--color-text-muted)'

function StateText({ state }: { state: GrokAgentState }): React.JSX.Element {
  return (
    <span data-testid="grokbot-state" data-state={state} className={`text-[10px] ${STATE_TEXT[state]}`}>
      {GROK_STATE_LABEL[state]}
    </span>
  )
}

function GrokAvatar({ id, name, color, avatar }: { id: string; name: string; color: string | null; avatar: string | null }): React.JSX.Element {
  return (
    <ProjectAvatar
      projectPath={`grokbot:${id}`}
      projectName={name}
      projectColor={color ?? FALLBACK_COLOR}
      iconUrl={avatar}
      size={32}
      fetchIcon={false}
    />
  )
}

/** A row's hover text. The latest-message line is Grok Bot's own preview
 *  (current even when the bot's chat lives elsewhere), never the last line
 *  of this computer's copy. */
export function rowTitle(name: string, agent: GrokBotAgent): string {
  const head = agent.title ? `${name} — ${agent.title}` : name
  return agent.lastMessagePreview ? `${head}\n${agent.lastMessagePreview}` : head
}

/** The menu on a Grok Bot row in the Agents sidebar. */
export async function grokBotAgentsMenu(e: React.MouseEvent, hostKey: string, agent: GrokBotAgent): Promise<void> {
  e.preventDefault()
  const clicked = await showContextMenu([addToHomeMenuItem()])
  if (clicked === ADD_TO_HOME_MENU_ID) {
    // The existing Add to Home flow: one Home adds at once, several open
    // the picker. The row is `grokbot-<id>::<host>`.
    addAgentToHomeFromMenu(hostKey, {
      id: agent.id,
      name: agent.name,
      handle: grokBotHomeHandle(agent.id),
      iconUrl: agent.avatar,
    })
  }
}

export function GrokBotAgentRow({
  hostKey,
  agent,
  status,
}: {
  hostKey: string
  agent: GrokBotAgent
  status: GrokBotStatus
}): React.JSX.Element {
  const open = useGrokBotsStore((s) => s.open)
  const isActive = open?.hostKey === hostKey && open.agentId.toLowerCase() === agent.id.toLowerCase()
  const state = grokAgentState(agent, status)
  return (
    <div className="no-drag" data-grokbot-row={agent.id}>
      <AgentRowButton
        isActive={isActive}
        color={agent.color ?? FALLBACK_COLOR}
        dimmed={state === 'not-running'}
        onClick={() => useGrokBotsStore.getState().openChat({ hostKey, agentId: agent.id })}
        onContextMenu={(e) => void grokBotAgentsMenu(e, hostKey, agent)}
        title={rowTitle(agent.name, agent)}
        avatar={<GrokAvatar id={agent.id} name={agent.name} color={agent.color} avatar={agent.avatar} />}
        name={agent.name}
        nameAside={<GrokBotTag />}
        subline={<StateText state={state} />}
      />
    </div>
  )
}

/** The Agents sidebar's Grok Bots group for the connected server. Shows
 *  nothing unless the server has the feature and Show is on there. */
export function GrokBotSidebarSection(): React.JSX.Element | null {
  const scope = primaryScope()
  const hostKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connected = useConnectHostStore((s) => s.connectionStatus === 'connected')
  const supported = connected && grokSupported(scope)
  useGrokBotsSync(supported ? scope : null)
  const state = useGrokBotsStore((s) => s.byHost[hostKey])
  const list = state?.list
  if (!supported || !list || !list.enabled || list.agents.length === 0) return null
  return (
    <div data-testid="grokbot-section" className="mt-1 border-t border-[var(--color-border)] pt-1">
      <div className="px-3 pb-0.5 pt-1 text-[10px] font-medium uppercase tracking-wide text-[var(--color-text-muted)]">
        Grok Bots
      </div>
      {list.agents.map((a) => (
        <GrokBotAgentRow key={a.id} hostKey={hostKey} agent={a} status={list.status} />
      ))}
    </div>
  )
}

/** A Grok Bot row on a Home (any saved server). */
export function GrokBotHomeRow({ home, row, index }: { home: Home; row: HomeRow; index: number }): React.JSX.Element {
  const parsed = parseHomeAddress(row.address)
  const hostKey = parsed?.host ?? ''
  const agentId = grokBotIdFromAddress(row.address) ?? ''
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const onConnected = hostKey === connectedKey
  const scope = hostKey ? scopeForGrokHost(hostKey) : null
  // The window's server gets events; other servers are polled while shown.
  useGrokBotsSync(scope, onConnected ? 0 : 15000, !(isWebClient() && !onConnected))
  const hostState = useGrokBotsStore((s) => s.byHost[hostKey])
  const open = useGrokBotsStore((s) => s.open)
  const cachedAvatar = useHomeRowAvatar(row.address)
  const agent = grokAgentOn(hostState, agentId)
  const list = hostState?.list ?? null
  const state = agent && list ? grokAgentState(agent, list.status) : null
  const unavailable =
    isWebClient() && !onConnected
      ? 'Open this Grok Bot from the desktop app'
      : hostState?.error
        ? `Can't reach Grok Bots on that server (${hostState.error})`
        : list && !list.enabled
          ? 'Local Grok Bots are off on that server'
          : list && !agent
            ? `${row.label} is not on that server any more`
            : null
  const isActive = open?.hostKey === hostKey && open.agentId.toLowerCase() === agentId
  return (
    <div className="no-drag" data-grokbot-home-row={row.address}>
      <AgentRowButton
        isActive={isActive}
        color={agent?.color ?? FALLBACK_COLOR}
        dimmed={unavailable !== null || state === 'not-running'}
        onClick={() => {
          if (unavailable === null && agentId) useGrokBotsStore.getState().openChat({ hostKey, agentId: agent?.id ?? agentId })
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          void showContextMenu([{ id: 'home-remove', label: `Remove from ${home.name}` }]).then((clicked) => {
            if (clicked === 'home-remove') useHomesStore.getState().removeRow(home.id, row.address)
          })
        }}
        title={unavailable ?? (agent ? rowTitle(row.label, agent) : `Open ${row.label}`)}
        avatar={
          <GrokAvatar
            id={agentId}
            name={agent?.name ?? row.label}
            color={agent?.color ?? null}
            avatar={agent?.avatar ?? cachedAvatar}
          />
        }
        name={agent?.name ?? row.label}
        nameAside={<GrokBotTag />}
        subline={
          <>
            <div className="flex min-w-0 flex-1 items-center">
              {state ? (
                <StateText state={state} />
              ) : (
                <span className="text-[10px] text-[var(--color-text-muted)]">
                  {unavailable ? 'Unavailable' : 'Checking…'}
                </span>
              )}
            </div>
            <ShortcutIndexBadge index={index} />
          </>
        }
      />
    </div>
  )
}
