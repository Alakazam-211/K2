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

const IDENTITY_FLAGS = new Set(['--session-id', '--resume', '-r', '--session'])
const FROM_BRANCH_SUFFIX = / \(from [^)]+\)$/

export interface ChatSessionTabHit {
  sessionId: string
  provider: string | null
}

export interface ChatRenamePayload {
  provider: string
  session_id: string
  custom_name: string
}

export interface CopyableAddress {
  /** Segment only (`reviewer` / `1` / workspace handle). Never `sales/…`. */
  label: string
  /** Typeable `k2 msg` target (`sales/reviewer` or `sales`). */
  clipboard: string
}

export interface DaemonHandleRow {
  agentName?: string
  sessionId?: string
  kind?: string
  handle?: string
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

function isUuidShape(s: string): boolean {
  return /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(s)
}

/** Provider conversation id from resume / premint argv. Not the PTY id. */
export function conversationIdFromArgs(command?: string, args?: string[]): string | null {
  if (!args?.length) return null
  for (let i = 0; i < args.length; i++) {
    if (IDENTITY_FLAGS.has(args[i]) && args[i + 1] && !args[i + 1].startsWith('-')) {
      return args[i + 1]
    }
  }
  if (args[0] === 'resume' && args[1] && !args[1].startsWith('-')) {
    return args[1]
  }
  for (const arg of args) {
    if (isUuidShape(arg)) return arg
  }
  void command
  return null
}

export function conversationIdFromTerminal(td: TerminalItemData): string | null {
  const stamped = td.conversationId?.trim()
  if (stamped) return stamped
  return conversationIdFromArgs(td.command, td.args)
}

/** Copy daemon conversationId onto the item. Never write the PTY sessionId. */
export function pickConversationId(
  current: string | undefined,
  daemonConversationId: string | undefined,
  daemonPtySessionId: string | undefined,
): string | undefined {
  const incoming = daemonConversationId?.trim()
  if (incoming && incoming !== daemonPtySessionId) return incoming
  return current
}

/** Cross-worktree suffix stays tab-only (N7). */
export function restampTitle(currentTitle: string, displayName: string): string {
  const m = currentTitle.match(FROM_BRANCH_SUFFIX)
  return m ? `${displayName}${m[0]}` : displayName
}

/** Session tab: conversationId first, then exact args uuid.
 *  Ignores `TerminalItemData.sessionId` (Kessel PTY id after reconcile). */
export function findChatSessionInTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  sessionIds?: Set<string>,
): ChatSessionTabHit | null {
  if (tab.isSystemAgent) return null
  for (const [, pg] of tab.paneGroups) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const td = item.data as TerminalItemData
      if (isExcludedTerminal(td)) continue
      const provider = providerFromCommand(td.command)
      const stamped = td.conversationId?.trim()
      if (stamped && (!sessionIds || sessionIds.has(stamped))) {
        return { sessionId: stamped, provider }
      }
      for (const arg of td.args ?? []) {
        if (sessionIds ? sessionIds.has(arg) : isUuidShape(arg)) {
          return { sessionId: arg, provider }
        }
      }
      if (!sessionIds) {
        const fromArgs = conversationIdFromArgs(td.command, td.args)
        if (fromArgs) return { sessionId: fromArgs, provider }
      }
    }
  }
  return null
}

/** True when a tab looks like a harness session (N5). File / heartbeat / API stay false. */
export function tabLooksLikeChatSession(tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>): boolean {
  if (tab.isSystemAgent) return false
  for (const [, pg] of tab.paneGroups) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const td = item.data as TerminalItemData
      if (isExcludedTerminal(td)) continue
      if (providerFromCommand(td.command)) return true
    }
  }
  return false
}

export function chatRenamePayloadForTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  sessions: Array<{ sessionId: string; provider: string }>,
  customName: string,
): ChatRenamePayload | null {
  const name = customName.trim()
  if (!name) return null
  const hit = findChatSessionInTab(tab)
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
  tabs: Array<Pick<Tab, 'id' | 'title' | 'isSystemAgent' | 'paneGroups'>>,
  sessionId: string,
  title: string,
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void,
): void {
  const ids = new Set([sessionId])
  for (const tab of tabs) {
    const hit = findChatSessionInTab(tab, ids)
    if (hit && hit.sessionId === sessionId) {
      setTabTitle(tab.id, restampTitle(tab.title ?? '', title), { locked: true })
    }
  }
}

/** Persist a user tab rename into chat_session_names when the tab is a session tab.
 *  Does not require the uuid to already be on chat/list (fresh premint). */
export async function persistChatRenameIfSessionTab(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  customName: string,
  projectPath: string,
): Promise<boolean> {
  const name = customName.trim()
  if (!name || !projectPath) return false
  const hit = findChatSessionInTab(tab)
  if (!hit?.sessionId) return false
  const provider = hit.provider
  if (!provider) return false
  await daemonCliPost('chat/rename', {
    provider,
    session_id: hit.sessionId,
    custom_name: name,
  })
  return true
}

export function copyableAddressFromDaemonRow(row: DaemonHandleRow): CopyableAddress | null {
  const handle = row.handle?.trim()
  if (!handle) return null
  if (row.kind === 'api' || row.kind === 'other') return null
  if (row.kind === 'canonical') {
    const slash = handle.indexOf('/')
    const segment = slash >= 0 ? handle.slice(0, slash) : handle
    if (!segment) return null
    return { label: segment, clipboard: segment }
  }
  if (row.kind === 'sidecar') {
    const slash = handle.lastIndexOf('/')
    const segment = slash >= 0 ? handle.slice(slash + 1) : handle
    if (!segment) return null
    return { label: segment, clipboard: handle }
  }
  return null
}

export function copyableAddressForWorkspaceHandle(handle: string): CopyableAddress | null {
  const h = handle.trim()
  if (!h) return null
  return { label: h, clipboard: h }
}

export function daemonRowForTab(
  tab: Pick<Tab, 'paneGroups'>,
  rows: DaemonHandleRow[],
): DaemonHandleRow | null {
  for (const [pgId] of tab.paneGroups) {
    const name = `tab-${pgId}`
    const row = rows.find((r) => r.agentName === name)
    if (row) return row
  }
  return null
}

export function daemonRowForCanonicalChat(
  rows: DaemonHandleRow[],
  projectId?: string | null,
): DaemonHandleRow | null {
  const byKind = rows.find((r) => r.kind === 'canonical')
  if (byKind) return byKind
  if (projectId) {
    const byName = rows.find((r) => r.agentName === projectId)
    if (byName) return byName
  }
  return null
}

export async function resolveSessionTabCopyableAddress(
  tab: Pick<Tab, 'isSystemAgent' | 'paneGroups'>,
  projectPath: string,
): Promise<CopyableAddress | null> {
  if (tab.isSystemAgent || !projectPath) return null
  try {
    const rows = await daemonCliGet<DaemonHandleRow[]>('sessions/list-for-workspace', {
      path: projectPath,
    })
    const row = daemonRowForTab(tab, Array.isArray(rows) ? rows : [])
    return row ? copyableAddressFromDaemonRow(row) : null
  } catch {
    return null
  }
}

export async function resolvePinnedChatCopyableAddress(
  projectPath: string,
  projectId?: string | null,
): Promise<CopyableAddress | null> {
  if (!projectPath) return null
  try {
    const rows = await daemonCliGet<DaemonHandleRow[]>('sessions/list-for-workspace', {
      path: projectPath,
    })
    const row = daemonRowForCanonicalChat(Array.isArray(rows) ? rows : [], projectId)
    const fromRow = row ? copyableAddressFromDaemonRow(row) : null
    if (fromRow) return fromRow
  } catch {
    /* fall through to workspace/handle */
  }
  try {
    const r = await daemonCliGet<{ handle?: string }>('workspace/handle', {
      project: projectPath,
    })
    return copyableAddressForWorkspaceHandle(r?.handle ?? '')
  } catch {
    return null
  }
}
