import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { Tab, TerminalItemData } from '@/stores/tabs'

/** SSOT display: non-empty trimmed customName, else provider title. */
export function chatDisplayName(s: { customName?: string | null; title: string }): string {
  return s.customName?.trim() || s.title
}

const COMMAND_TO_PROVIDER: Record<string, string> = {
  claude: 'claude',
  'cursor-agent': 'cursor',
  cursor: 'cursor',
  grok: 'grok',
  gemini: 'gemini',
  pi: 'pi',
  codex: 'codex',
  hermes: 'hermes',
}

export interface ChatSessionTabHit {
  sessionId: string
  provider: string | null
}

export interface ChatRenamePayload {
  provider: string
  session_id: string
  custom_name: string
}

function commandBase(command?: string): string {
  if (!command) return ''
  return command.split(/[/\\]/).pop()?.split(/\s+/)[0] ?? ''
}

function providerFromCommand(command?: string): string | null {
  const base = commandBase(command)
  return COMMAND_TO_PROVIDER[base] ?? null
}

function isExcludedTerminal(data: TerminalItemData): boolean {
  if (data.heartbeatName) return true
  if (data.fromApi) return true
  if (data.attachAgentName?.startsWith('api-')) return true
  return false
}

/** Session tab: non-system tab whose terminal args contain an exact listed session id.
 *  Ignores `TerminalItemData.sessionId` (Kessel PTY id after reconcile). */
export function findChatSessionInTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  sessionIds: Set<string>,
): ChatSessionTabHit | null {
  if (tab.isSystemAgent) return null
  if (sessionIds.size === 0) return null
  for (const [, pg] of tab.paneGroups) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const td = item.data as TerminalItemData
      if (isExcludedTerminal(td)) continue
      for (const arg of td.args ?? []) {
        if (sessionIds.has(arg)) {
          return { sessionId: arg, provider: providerFromCommand(td.command) }
        }
      }
    }
  }
  return null
}

export function chatRenamePayloadForTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  sessions: Array<{ sessionId: string; provider: string }>,
  customName: string,
): ChatRenamePayload | null {
  const name = customName.trim()
  if (!name) return null
  const ids = new Set(sessions.map((s) => s.sessionId))
  const hit = findChatSessionInTab(tab, ids)
  if (!hit) return null
  const row = sessions.find((s) => s.sessionId === hit.sessionId)
  const provider = hit.provider ?? row?.provider
  if (!provider) return null
  return { provider, session_id: hit.sessionId, custom_name: name }
}

export function findTabByPaneGroupId<T extends { paneGroups: Map<string, unknown> }>(
  tabs: Iterable<T>,
  pgId: string,
): T | undefined {
  for (const tab of tabs) {
    if (tab.paneGroups.has(pgId)) return tab
  }
  return undefined
}

export function collectStoreTabs<T>(store: {
  tabs: T[]
  extraGroups: Array<{ tabs: T[] }>
}): T[] {
  return [...store.tabs, ...store.extraGroups.flatMap((g) => g.tabs)]
}

export function restampSessionTabs(
  tabs: Array<Pick<Tab, 'id' | 'isSystemAgent' | 'paneGroups'>>,
  sessionId: string,
  title: string,
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void,
): void {
  const ids = new Set([sessionId])
  for (const tab of tabs) {
    const hit = findChatSessionInTab(tab, ids)
    if (hit && hit.sessionId === sessionId) {
      setTabTitle(tab.id, title, { locked: true })
    }
  }
}

/** Persist a user tab rename into chat_session_names when the tab is a session tab. */
export async function persistChatRenameIfSessionTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  customName: string,
  projectPath: string,
): Promise<boolean> {
  const name = customName.trim()
  if (!name || !projectPath) return false
  const sessions = await daemonCliGet<Array<{ sessionId: string; provider: string }>>(
    'chat/list',
    { project_path: projectPath },
  )
  const payload = chatRenamePayloadForTab(tab, Array.isArray(sessions) ? sessions : [], name)
  if (!payload) return false
  await daemonCliPost('chat/rename', payload)
  return true
}
