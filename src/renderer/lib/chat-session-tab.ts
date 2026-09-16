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

/**
 * Overlay Thread|Terminal chrome (C9): pinned Chat is handled separately.
 * Extra terminal items get chrome when they are an **agent PTY**
 * (harness command / sidecar / heartbeat surface), not empty bash,
 * file viewer, inbox, browser, or `/v1` API cells.
 */
export function isAgentPtyTerminalItem(item: { type: string; data: unknown }): boolean {
  if (item.type !== 'terminal') return false
  const td = item.data as TerminalItemData
  if (td.fromApi) return false
  if (td.attachAgentName?.startsWith('api-')) return false
  if (providerFromCommand(td.command) || providerFromCommand(td.commandHint)) return true
  if (td.conversationId?.trim()) return true
  return false
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

export function findTabById<T extends { id: string }>(
  tabs: Iterable<T>,
  tabId: string | undefined,
): T | undefined {
  if (!tabId) return undefined
  for (const tab of tabs) {
    if (tab.id === tabId) return tab
  }
  return undefined
}

/** Same leading OSC/idle glyph class as AlacrittyTerminalView title strip (~541). */
const OSC_IDLE_GLYPH_PREFIX = /^[\u2800-\u28FF*✱✲✳✴✵✶✷✸✹⚹⁎∗※·•●◦‣⏺]\s*/

export function stripOscIdleGlyphs(label: string): string {
  return label.replace(OSC_IDLE_GLYPH_PREFIX, '').trim()
}

/** PTY/OSC harness names that must not replace a named chat. */
export function isHarnessTabLabel(label: string): boolean {
  const n = stripOscIdleGlyphs(label).toLowerCase()
  if (!n) return false
  if (n === 'claude code' || n === 'cursor agent') return true
  const base = n.split(/[\s/\\]/)[0] ?? n
  return base in COMMAND_TO_PROVIDER
}

export function conversationIdFromTab(
  tab: Pick<Tab, 'paneGroups'> | undefined,
): string | undefined {
  if (!tab?.paneGroups) return undefined
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const id = conversationIdFromTerminal(item.data as TerminalItemData)?.trim()
      if (id) return id
    }
  }
  return undefined
}

function tabHasConversationId(tab: Pick<Tab, 'paneGroups'> | undefined): boolean {
  return Boolean(conversationIdFromTab(tab))
}

function terminalLookupKeys(tab: Pick<Tab, 'paneGroups'> | undefined): string[] {
  if (!tab?.paneGroups) return []
  const keys: string[] = []
  for (const pg of tab.paneGroups.values()) {
    for (const item of pg.items) {
      if (item.type !== 'terminal') continue
      const d = item.data as TerminalItemData
      const cid = conversationIdFromTerminal(d)?.trim()
      if (cid) keys.push(cid)
      const hint = d.commandHint?.trim()
      if (hint && isUuidShape(hint)) keys.push(hint)
      const sid = d.sessionId?.trim()
      if (sid && isUuidShape(sid)) keys.push(sid)
    }
  }
  return keys
}

export interface TabTitleAdoptState {
  title?: string
  locked?: boolean
  conversationId?: string | null
}

/**
 * Single chokepoint for title writes onto an existing tab (T1).
 * Harness incoming never replaces a real live name (T2). Omitted
 * `locked` keeps live.locked (T3). Keeping a named title with a
 * conversationId forces lock true (T14). Result `locked` is always
 * boolean — never undefined.
 */
export function adoptTabTitle(
  live: TabTitleAdoptState,
  incoming: TabTitleAdoptState,
): { title: string; locked: boolean } {
  const liveTitle = (live.title ?? '').trim()
  const incomingTitle = (incoming.title ?? '').trim()
  const hasConversation = Boolean(
    (live.conversationId ?? incoming.conversationId)?.trim(),
  )
  const keepLiveName =
    incomingTitle.length > 0 &&
    isHarnessTabLabel(incomingTitle) &&
    liveTitle.length > 0 &&
    !isHarnessTabLabel(liveTitle)

  const title = keepLiveName ? liveTitle : (incomingTitle || liveTitle)

  let locked: boolean
  if (typeof incoming.locked === 'boolean') {
    locked = incoming.locked
  } else {
    locked = live.locked === true
  }
  if (keepLiveName && hasConversation) locked = true

  return { title, locked: locked === true }
}

const customNameByKey = new Map<string, string>()
const tabTitleById = new Map<string, { title: string; locked?: boolean }>()

export function rememberChatCustomName(sessionId: string, name: string): void {
  const id = sessionId.trim()
  const n = name.trim()
  if (!id || !n || isHarnessTabLabel(n)) return
  customNameByKey.set(id, n)
}

export function rememberChatCustomNames(map: Record<string, string> | null | undefined): void {
  if (!map) return
  for (const [key, name] of Object.entries(map)) {
    rememberChatCustomName(key, name)
    const colon = key.lastIndexOf(':')
    if (colon > 0) rememberChatCustomName(key.slice(colon + 1), name)
  }
}

export function rememberTabTitleSnapshot(tabId: string, title: string, locked?: boolean): void {
  const id = tabId.trim()
  const n = title.trim()
  if (!id || !n || isHarnessTabLabel(n)) return
  tabTitleById.set(id, { title: n, ...(typeof locked === 'boolean' ? { locked } : {}) })
}

export function lookupNamedChatTitle(...keys: Array<string | null | undefined>): string | undefined {
  for (const key of keys) {
    const k = key?.trim()
    if (!k) continue
    const custom = customNameByKey.get(k)
    if (custom) return custom
    const snap = tabTitleById.get(k)
    if (snap?.title) return snap.title
  }
  return undefined
}

export function lookupTabTitleSnapshot(tabId: string | undefined): { title: string; locked?: boolean } | undefined {
  const id = tabId?.trim()
  if (!id) return undefined
  return tabTitleById.get(id)
}

export function rememberLiveNamedChatTitles(
  tabs: Iterable<Pick<Tab, 'id' | 'title' | 'locked' | 'paneGroups'>>,
): void {
  for (const tab of tabs) {
    const title = tab.title?.trim()
    if (!title || isHarnessTabLabel(title)) continue
    rememberTabTitleSnapshot(tab.id, title, tab.locked)
    const cid = conversationIdFromTab(tab)
    if (cid) rememberChatCustomName(cid, title)
  }
}

/** Sync restamp input for restoreLayout (T4/T10/T11). Never awaits. */
export function adoptRestoredTab(
  built: Pick<Tab, 'id' | 'title' | 'locked' | 'paneGroups'>,
  live?: Pick<Tab, 'id' | 'title' | 'locked' | 'paneGroups'>,
): { title: string; locked: boolean } {
  const conversationId = conversationIdFromTab(built) ?? conversationIdFromTab(live)
  const mapped = lookupNamedChatTitle(
    conversationId,
    built.id,
    live?.id,
    ...terminalLookupKeys(built),
    ...terminalLookupKeys(live),
  )
  const snap = lookupTabTitleSnapshot(built.id) ?? lookupTabTitleSnapshot(live?.id)
  const incomingTitle = mapped ?? built.title
  const incomingLocked = mapped
    ? (typeof snap?.locked === 'boolean' ? snap.locked : true)
    : (typeof built.locked === 'boolean' ? built.locked : snap?.locked)
  return adoptTabTitle(
    {
      title: live?.title ?? built.title,
      locked: live?.locked ?? built.locked,
      conversationId,
    },
    {
      title: incomingTitle,
      locked: incomingLocked,
      conversationId,
    },
  )
}

export function __resetNamedChatTitleCachesForTests(): void {
  customNameByKey.clear()
  tabTitleById.clear()
}

/** label_initial / label_changed / OSC: never unlocked-write a locked tab.
 *  Also never stamp a harness basename onto a named-chat tab (Grok OSC
 *  "grok" was still winning the first paint on Cortana). */
export function applyUnlockedTabLabel(
  tab: (Pick<Tab, 'id' | 'locked'> & Partial<Pick<Tab, 'paneGroups'>>) | undefined,
  label: string,
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void,
): void {
  const next = label.trim()
  if (!tab || !next) return
  if (tab.locked) return
  if (isHarnessTabLabel(next) && tabHasConversationId(tab)) return
  setTabTitle(tab.id, next)
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
      // chat/list often has title=grok and empty customName; that restamp
      // was locking the harness name over "Hi Test". Never replace a
      // real name with a harness basename.
      if (isHarnessTabLabel(title) && !isHarnessTabLabel(tab.title ?? '')) continue
      setTabTitle(tab.id, restampTitle(tab.title ?? '', title), { locked: true })
    }
  }
}

export function restampListedChatTabs(
  tabs: Array<Pick<Tab, 'id' | 'title' | 'isSystemAgent' | 'paneGroups'>>,
  sessions: Array<{ sessionId: string; customName?: string | null; title?: string }>,
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void,
): void {
  for (const s of sessions) {
    if (!s.sessionId) continue
    if (s.customName?.trim()) rememberChatCustomName(s.sessionId, s.customName)
    restampSessionTabs(
      tabs,
      s.sessionId,
      chatDisplayName({ customName: s.customName, title: s.title ?? '' }),
      setTabTitle,
    )
  }
}

/** Restore path: once conversationId is on the layout, restamp extras from custom_name. */
export async function restampSessionTabsFromChatList(
  projectPath: string,
  tabs: Array<Pick<Tab, 'id' | 'title' | 'isSystemAgent' | 'paneGroups'>>,
  setTabTitle: (tabId: string, title: string, opts?: { locked?: boolean }) => void,
): Promise<void> {
  if (!projectPath) return
  try {
    const rows = await daemonCliGet<Array<{
      sessionId: string
      customName?: string | null
      title?: string
    }>>('chat/list', { project_path: projectPath })
    restampListedChatTabs(tabs, Array.isArray(rows) ? rows : [], setTabTitle)
  } catch {
    /* layout titles stay */
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
  rememberChatCustomName(hit.sessionId, name)
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
