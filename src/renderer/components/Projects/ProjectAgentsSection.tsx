// The selected project's section of the Projects nav (Rosson, 2026-11-11:
// keep the agents and their files in the nav, organised): under the project
// list, a divider, then the project's picture and name, its agents, and
// "Add an agent…".
//   - each agent row: picture, name, a star for the point of contact, the view its
//     tile shows (the picker's icon) and whether it's awake or asleep. Click
//     opens or focuses its tile on the open dashboard; drag drops it onto
//     the canvas (the dashboard-dnd bridge); right-click pins it to the top.
//     ⌘1–9 still focus tiles; the row shows its number on hover or focus,
//     as the tile bar does;
//   - the files an agent shared (Workspace Resources) nest under it, folded
//     per agent: click shows one on this dashboard, drag places it,
//     right-click pins it or removes it from the agent's Files;
//   - "Add an agent…" opens the picker Settings → Projects uses, in a
//     dialog, without leaving the page.
// The collapsed rail keeps the agents as pictures (RailAgents).

import React, { useEffect, useMemo, useRef, useState } from 'react'
import { useProjectsStore } from '@/stores/projects'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { useToastStore } from '@/stores/toast'
import { useWindowModeStore, noteViewerInteractionBlocked } from '@/stores/window-mode'
import { onWorkspaceResourcesChanged } from '@/stores/session-events'
import { showContextMenu } from '@/lib/context-menu'
import { SetiFileIcon } from '@/lib/seti-file-icons'
import { daemonCliGet } from '@/lib/daemon-cli'
import ProjectAvatar from '@/components/Sidebar/ProjectAvatar'
import { ViewIcon } from '@/components/SessionView/ThreadDrawerSelector'
import { Button, Tag, cx } from '@/components/ui'
import { AVATAR_ROUND, SETTING_LIST_ROW_HOVER } from '@/components/Settings/controls/SettingControls'
import { AccessSheet, ConfirmSheet } from '@/components/Settings/sections/AccessDialogs'
import { primaryScope } from '@/kessel/server-scope'
import { completeDashDrag, useDashboardDndStore, type DashDragPayload } from './dashboard-dnd'
import ProjectGroupAvatar from './ProjectGroupAvatar'
import { useTileView } from './DashboardAgentTile'
import type { TileView } from './tile-view'
import { agentCountText, dashboardShowsDoc, removeMemberBlockedReason } from './project-settings'
import {
  createErrorMessage,
  fetchProjectGroupResources,
  removeProjectGroupMember,
  removeWorkspaceResource,
  setProjectGroupPoc,
  type ProjectGroupDashboard,
  type ProjectGroupHtmlDoc,
  type ProjectGroupMemberInfo,
  type ProjectGroupShow,
} from './projects-api'
import {
  loadPinnedMemberIds,
  loadPinnedResourceKeys,
  resourceDocKey,
  sortMembersPinnedFirst,
  sortResourcesPinnedThenAlpha,
  togglePinnedMember,
  togglePinnedResource,
} from './member-pins'
import { AddAgentPicker } from './ProjectSettings'

/** An agent's name as the page shows it. */
export function memberDisplayName(m: ProjectGroupMemberInfo): string {
  return m.agentName ?? m.name ?? m.workspaceId.slice(0, 8)
}

const VIEW_NAME: Record<TileView, string> = {
  terminal: 'Terminal',
  thread: 'Thread',
  chatter: 'Chatter',
  heartbeats: 'Heartbeats',
}

export type Liveness = 'awake' | 'asleep'

/** Whether each agent has a live session, as the tiles ask it
 *  (`sessions/lookup-by-agent`, one per agent), on open and on project
 *  changes. Unknown (missing) while it loads or when the server can't say. */
export function useMembersLiveness(groupId: string, workspaceIds: string[]): Record<string, Liveness> {
  const [state, setState] = useState<Record<string, Liveness>>({})
  const revision = useProjectGroupsStore((s) => s.revision)
  const key = workspaceIds.join(',')
  useEffect(() => {
    let cancelled = false
    setState({})
    for (const id of workspaceIds) {
      daemonCliGet<{ sessionAlive?: boolean }>(primaryScope(), 'sessions/lookup-by-agent', { agent: id })
        .then((r) => {
          if (cancelled || typeof r?.sessionAlive !== 'boolean') return
          setState((s) => ({ ...s, [id]: r.sessionAlive ? 'awake' : 'asleep' }))
        })
        .catch(() => {
          /* unknown: the row shows nothing rather than a guess */
        })
    }
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [groupId, key, revision])
  return state
}

/** Press-and-move on a row starts a drag onto the dashboard (Sidebar's
 *  handleProjectMouseDown threshold); a plain click stays a click. */
function startDashDrag(
  e: React.MouseEvent,
  payload: DashDragPayload,
  suppressClick: React.MutableRefObject<boolean>,
): void {
  if (e.button !== 0) return
  const startX = e.clientX
  const startY = e.clientY
  let started = false

  const onMove = (ev: MouseEvent): void => {
    if (!started && (Math.abs(ev.clientX - startX) > 3 || Math.abs(ev.clientY - startY) > 5)) {
      started = true
      suppressClick.current = true
      document.body.style.cursor = 'grabbing'
      document.body.style.userSelect = 'none'
    }
    if (!started) return
    useDashboardDndStore.getState().setDrag({ payload, x: ev.clientX, y: ev.clientY })
  }

  const onUp = (ev: MouseEvent): void => {
    document.removeEventListener('mousemove', onMove)
    document.removeEventListener('mouseup', onUp)
    if (!started) return
    document.body.style.cursor = ''
    document.body.style.userSelect = ''
    completeDashDrag(payload, ev.clientX, ev.clientY)
    useDashboardDndStore.getState().setDrag(null)
    // The trailing click is swallowed by the flag; re-arm on the next tick
    // in case the browser skips it (pointer released off the row).
    setTimeout(() => {
      suppressClick.current = false
    }, 0)
  }

  document.addEventListener('mousemove', onMove)
  document.addEventListener('mouseup', onUp)
}

function FoldChevron({ open }: { open: boolean }): React.JSX.Element {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 16 16"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      className="w-3 h-3 transition-transform"
      style={{ transform: open ? 'rotate(90deg)' : undefined }}
    >
      <path d="M6 3.5 10.5 8 6 12.5" />
    </svg>
  )
}

/** An agent's right-click menu: go to its tile, make it the point of
 *  contact, show or hide its files, pin it, and remove it from the
 *  project (after the shared confirm). */
async function agentMenu(o: {
  name: string
  projectName: string
  paneNum: number | undefined
  isPoc: boolean
  isPinned: boolean
  files: number
  folded: boolean
  canEdit: boolean
  removeBlocked: string | null
}): Promise<string | null> {
  return showContextMenu([
    { id: 'go', label: 'Go to its tile', shortcut: o.paneNum !== undefined ? `⌘${o.paneNum}` : undefined },
    ...(o.canEdit && !o.isPoc ? [{ id: 'poc', label: 'Make point of contact' }] : []),
    ...(o.files > 0 ? [{ id: 'fold', label: o.folded ? 'Show its files' : 'Hide its files' }] : []),
    { id: 'pin', label: o.isPinned ? 'Unpin' : 'Pin to top' },
    ...(o.canEdit
      ? [
          { id: 'sep', label: '', type: 'separator' },
          {
            id: 'remove',
            label: `Remove from ${o.projectName}…`,
            enabled: o.removeBlocked === null,
            tooltip: o.removeBlocked ?? undefined,
          },
        ]
      : []),
  ])
}

function AgentRow({
  show,
  member,
  isPoc,
  isPinned,
  liveness,
  files,
  folded,
  canEdit,
  onFold,
  onTogglePin,
  onMakePoc,
  onRemove,
}: {
  show: ProjectGroupShow
  member: ProjectGroupMemberInfo
  isPoc: boolean
  isPinned: boolean
  liveness: Liveness | undefined
  /** This agent's files (none: no fold toggle). */
  files: number
  folded: boolean
  /** May change the project's agents (any login today). */
  canEdit: boolean
  onFold: () => void
  onTogglePin: (workspaceId: string) => void
  onMakePoc: () => void
  onRemove: () => void
}): React.JSX.Element {
  // Icon/color come from the workspace-registry row (the member enrichment
  // carries name/path only).
  const workspace = useProjectsStore((s) => s.projects.find((p) => p.id === member.workspaceId))
  const name = memberDisplayName(member)
  const [view] = useTileView(show.id, member.workspaceId, show.defaultView)
  // ⌘N — this agent's tile number on the open dashboard.
  const paneNum = useProjectGroupsStore((s) => s.dashPaneNumbers[member.workspaceId])
  // A read-only window can still focus a tile, but never drag one in.
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const suppressClick = useRef(false)
  const open = (): void => useProjectGroupsStore.getState().requestMemberPane(member.workspaceId)

  return (
    <div className={cx('group relative flex items-stretch', SETTING_LIST_ROW_HOVER)} data-agent-row={member.workspaceId}>
      {files > 0 ? (
        <button
          type="button"
          data-k2-focus=""
          className="relative flex-shrink-0 w-6 flex items-center justify-center no-drag outline-none cursor-pointer text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
          aria-expanded={!folded}
          aria-label={`${folded ? 'Show' : 'Hide'} ${name}’s files`}
          title={`${files} ${files === 1 ? 'file' : 'files'}`}
          onClick={onFold}
          data-agent-fold=""
        >
          <FoldChevron open={!folded} />
        </button>
      ) : (
        <span className="flex-shrink-0 w-6" aria-hidden="true" />
      )}
      <button
        type="button"
        data-k2-focus=""
        className={cx(
          // overflow-hidden + pr-3: the row's right column (⌘N, view, state)
          // keeps a clear inset from the nav's edge and never spills past it.
          'relative flex-1 min-w-0 flex items-center gap-2 pr-3 py-1.5 text-left no-drag outline-none select-none overflow-hidden',
          readOnly ? 'cursor-default' : 'cursor-pointer',
        )}
        title={paneNum !== undefined ? `Go to ${name}’s tile (⌘${paneNum})` : `Open ${name} on this dashboard`}
        onClick={() => {
          if (suppressClick.current) {
            suppressClick.current = false
            return
          }
          // P5 (§6.1/§6.2): open (or focus — one tile per agent) this
          // agent's tile; the mounted ProjectDashboard consumes the request.
          open()
        }}
        onMouseDown={(e) => {
          if (readOnly) return
          startDashDrag(e, { type: 'member', workspaceId: member.workspaceId }, suppressClick)
        }}
        onContextMenu={async (e) => {
          e.preventDefault()
          e.stopPropagation()
          const picked = await agentMenu({
            name,
            projectName: show.name,
            paneNum,
            isPoc,
            isPinned,
            files,
            folded,
            canEdit,
            removeBlocked: removeMemberBlockedReason(member.workspaceId, show.pocWorkspaceId),
          })
          if (picked === 'go') open()
          else if (picked === 'poc') onMakePoc()
          else if (picked === 'fold') onFold()
          else if (picked === 'pin') onTogglePin(member.workspaceId)
          else if (picked === 'remove') onRemove()
        }}
      >
        <ProjectAvatar
          projectPath={member.path ?? ''}
          projectName={member.name ?? name}
          projectColor={workspace?.color ?? 'var(--color-text-muted)'}
          projectId={member.workspaceId}
          iconUrl={workspace?.iconUrl ?? null}
          size={18}
        />
        <span className="min-w-0 flex-shrink type-name text-[var(--color-text-primary)] truncate">{name}</span>
        {/* The full "Point of contact" tag doesn't fit beside the name,
            view and state in a 296px nav (Terminal's type is wider), so the
            row marks it with a star; the words are its tooltip and name. */}
        {isPoc && (
          <span
            className="flex-shrink-0 inline-flex text-[var(--color-accent-text)]"
            title="Point of contact: gets every message in the project chat"
            data-agent-poc=""
          >
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="currentColor" className="w-3 h-3">
              <path d="M8 1.75l1.9 3.85 4.25.62-3.07 3 .72 4.23L8 11.45l-3.8 2 .72-4.23-3.07-3 4.25-.62L8 1.75z" />
            </svg>
            <span className="sr-only">Point of contact</span>
          </span>
        )}
        {isPinned && <span className="sr-only">(pinned)</span>}
        <span className="flex-1" />
        {paneNum !== undefined && (
          <span
            className="flex-shrink-0 type-hint text-[var(--color-text-secondary)] tabular-nums hidden group-hover:inline group-focus-within:inline"
            aria-hidden="true"
            data-agent-shortcut=""
          >
            ⌘{paneNum}
          </span>
        )}
        <span
          className="flex-shrink-0 inline-flex text-[var(--color-text-secondary)] [&>svg]:w-3 [&>svg]:h-3"
          title={`Opens in ${VIEW_NAME[view]}`}
          data-agent-view={view}
        >
          <ViewIcon kind={view} />
          <span className="sr-only">Opens in {VIEW_NAME[view]}</span>
        </span>
        {liveness === 'awake' ? (
          <span
            className="flex-shrink-0 w-2 h-2 rounded-[var(--radius-badge)] bg-[var(--color-status-ok)]"
            title="Awake"
            data-liveness="awake"
          >
            <span className="sr-only">Awake</span>
          </span>
        ) : liveness === 'asleep' ? (
          <span className="flex-shrink-0 type-hint text-[var(--color-text-secondary)]" data-liveness="asleep">
            asleep
          </span>
        ) : null}
      </button>
    </div>
  )
}

function FileRow({
  doc,
  onDashboard,
  dashboardName,
  isPinned,
  onTogglePin,
  onRemoved,
}: {
  doc: ProjectGroupHtmlDoc
  /** Already a tile on the dashboard on screen. */
  onDashboard: boolean
  dashboardName: string | null
  isPinned: boolean
  onTogglePin: (doc: ProjectGroupHtmlDoc) => void
  onRemoved: () => void
}): React.JSX.Element {
  const suppressClick = useRef(false)
  const html = isHtmlFile(doc.filePath)
  const payload: DashDragPayload = { type: 'htmlDoc', workspaceId: doc.workspaceId, filePath: doc.filePath }
  const show = (): void => {
    if (suppressClick.current) {
      suppressClick.current = false
      return
    }
    if (noteViewerInteractionBlocked()) return
    useProjectGroupsStore.getState().requestFileDocPane(doc.workspaceId, doc.filePath)
  }

  return (
    <div
      data-file-row={doc.fileName}
      className={cx('group relative flex items-center gap-2 pl-[3.25rem] pr-3 py-1 select-none', SETTING_LIST_ROW_HOVER)}
      title={html ? `${doc.filePath}\nDouble-click to open on this dashboard` : `${doc.filePath}\nNot an HTML file: only HTML files can show on a dashboard for now`}
      onMouseDown={(e) => {
        if (!html || noteViewerInteractionBlocked()) return
        if ((e.target as HTMLElement).closest('button')) return
        startDashDrag(e, payload, suppressClick)
      }}
      // Double-click opens an HTML file (on this dashboard, or focuses its
      // tile there), as a click on the old Resources row did.
      onDoubleClick={(e) => {
        if (!html || (e.target as HTMLElement).closest('button')) return
        show()
      }}
      onContextMenu={async (e) => {
        e.preventDefault()
        e.stopPropagation()
        const picked = await showContextMenu([
          ...(html
            ? [
                { id: 'open', label: 'Open', tooltip: onDashboard ? 'Go to its tile on this dashboard' : 'Show it on this dashboard' },
                { id: 'sep-open', label: '', type: 'separator' },
              ]
            : []),
          { id: 'toggle-pin', label: isPinned ? 'Unpin' : 'Pin to top' },
          { id: 'remove', label: 'Remove from Files' },
        ])
        if (picked === 'open') show()
        if (picked === 'toggle-pin') onTogglePin(doc)
        if (picked === 'remove') {
          try {
            await removeWorkspaceResource(primaryScope(), doc.workspaceId, doc.filePath)
            onRemoved()
          } catch (err) {
            useToastStore
              .getState()
              .addToast(`Couldn’t remove ${doc.fileName}: ${err instanceof Error ? err.message : String(err)}`, 'error')
          }
        }
      }}
    >
      <span className={cx('flex-shrink-0 w-4 flex items-center justify-center', html ? '' : 'opacity-50')} aria-hidden="true">
        <SetiFileIcon name={doc.fileName} className="w-3.5 h-3.5" />
      </span>
      <span className="min-w-0 flex-1 flex items-center gap-2">
        <span className={cx('type-row truncate', html ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]')}>
          {doc.fileName}
        </span>
        {!html && <span className="sr-only">(not an HTML file, so it can’t show on a dashboard)</span>}
        {doc.missing ? (
          <Tag tone="warn" data-case="as-written" className="flex-shrink-0">
            Missing
          </Tag>
        ) : null}
        {isPinned && <span className="sr-only">(pinned)</span>}
      </span>
      {html ? (
        // A compact icon button, so the name keeps its room: a check once the
        // file is a tile here, else "show here" on hover or focus. The words
        // are its tooltip and accessible name.
        <button
          type="button"
          data-k2-focus=""
          className={cx(
            'relative flex-shrink-0 w-6 h-6 inline-flex items-center justify-center no-drag outline-none cursor-pointer rounded-[var(--radius-selector)] transition-colors hover:bg-[var(--color-wash-1)]',
            onDashboard
              ? 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
              : 'text-[var(--color-accent-text)] opacity-0 group-hover:opacity-100 focus-visible:opacity-100',
          )}
          aria-label={onDashboard ? `${doc.fileName} is on this dashboard: go to it` : `Show ${doc.fileName} on this dashboard`}
          title={
            onDashboard
              ? `On ${dashboardName ?? 'this dashboard'}: go to it`
              : `Show on ${dashboardName ?? 'this dashboard'}`
          }
          onClick={show}
          data-file-show=""
          data-on-dashboard={onDashboard ? '' : undefined}
        >
          {onDashboard ? (
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3">
              <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
            </svg>
          ) : (
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.5} strokeLinecap="round" strokeLinejoin="round" className="w-3.5 h-3.5">
              <rect x="2" y="3" width="12" height="10" />
              <path d="M8 6v4M6 8h4" />
            </svg>
          )}
        </button>
      ) : null}
    </div>
  )
}

/** Whether a file can be a dashboard tile (HTML only, for now). */
function isHtmlFile(path: string): boolean {
  return /\.html?$/i.test(path)
}

/** The project's files (Workspace Resources union). */
function useProjectFiles(groupId: string): {
  docs: ProjectGroupHtmlDoc[] | null
  error: string | null
  reload: () => void
  drop: (doc: ProjectGroupHtmlDoc) => void
} {
  const [docs, setDocs] = useState<ProjectGroupHtmlDoc[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [nonce, setNonce] = useState(0)
  const revision = useProjectGroupsStore((s) => s.revision)

  useEffect(() => {
    let cancelled = false
    const load = (): void => {
      fetchProjectGroupResources(primaryScope(), groupId)
        .then((d) => {
          if (cancelled) return
          setDocs(d)
          setError(null)
        })
        .catch((err) => {
          if (cancelled) return
          setError(err instanceof Error ? err.message : String(err))
        })
    }
    load()
    const off = onWorkspaceResourcesChanged(primaryScope(), () => load())
    return () => {
      cancelled = true
      off()
    }
  }, [groupId, revision, nonce])

  useEffect(() => {
    setDocs(null)
    setError(null)
  }, [groupId])

  return {
    docs,
    error,
    reload: () => {
      setError(null)
      setNonce((n) => n + 1)
    },
    drop: (doc) =>
      setDocs((prev) =>
        (prev ?? []).filter((d) => !(d.workspaceId === doc.workspaceId && d.filePath === doc.filePath)),
      ),
  }
}

/** "Add an agent to <project>": the Settings → Projects picker in a dialog. */
export function AddAgentDialog({
  show,
  onChanged,
  onClose,
}: {
  show: ProjectGroupShow
  onChanged: () => void
  onClose: () => void
}): React.JSX.Element {
  const memberIds = useMemo(() => show.members.map((m) => m.workspaceId), [show.members])
  return (
    <AccessSheet
      title={`Add an agent to ${show.name}`}
      intro="It joins the project’s chat and can go on its dashboards."
      onClose={onClose}
      testId="add-agent-dialog"
      footer={
        <Button variant="ghost" size="md" onClick={onClose}>
          Done
        </Button>
      }
    >
      {/* The rows' hover band runs to the dialog's edges; the search and
          the names keep its text edge. */}
      <div className="-mx-3">
        <AddAgentPicker
          groupId={show.id}
          groupName={show.name}
          memberIds={memberIds}
          onAdded={() => onChanged()}
          inset="px-3"
          listClassName="max-h-[min(50vh,22rem)]"
        />
      </div>
    </AccessSheet>
  )
}

const FOLDED_KEY = 'k2:projects-nav:folded-agents'

function loadFolded(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(FOLDED_KEY) ?? '[]') as unknown
    return Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : []
  } catch {
    return []
  }
}

/** The selected project's section of the nav. */
export function ProjectNavSection({
  show,
  dashboard,
  canAddAgents,
  onChanged,
}: {
  show: ProjectGroupShow
  /** The dashboard on screen (a file shows "On this dashboard" when it's a tile there). */
  dashboard: ProjectGroupDashboard | null
  /** Who may add or remove an agent and pick the point of contact
   *  (project-settings.ts: any login). */
  canAddAgents: boolean
  /** Refetch the project after a change made here. */
  onChanged: () => void
}): React.JSX.Element {
  const [adding, setAdding] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState<ProjectGroupMemberInfo | null>(null)
  const addRef = useRef<HTMLButtonElement>(null)
  const [pinnedIds, setPinnedIds] = useState<string[]>(() => loadPinnedMemberIds(show.id))
  const [pinnedKeys, setPinnedKeys] = useState<string[]>(() => loadPinnedResourceKeys(show.id))
  // Folded agents (their files hidden), per client: everything starts open.
  const [folded, setFolded] = useState<string[]>(loadFolded)
  useEffect(() => {
    setPinnedIds(loadPinnedMemberIds(show.id))
    setPinnedKeys(loadPinnedResourceKeys(show.id))
  }, [show.id])

  const members = useMemo(() => sortMembersPinnedFirst(show.members, pinnedIds).sorted, [show.members, pinnedIds])
  const liveness = useMembersLiveness(show.id, useMemo(() => show.members.map((m) => m.workspaceId), [show.members]))
  const files = useProjectFiles(show.id)
  const filesByAgent = useMemo(() => {
    const sorted = sortResourcesPinnedThenAlpha(files.docs ?? [], pinnedKeys).sorted
    const by: Record<string, ProjectGroupHtmlDoc[]> = {}
    for (const d of sorted) (by[d.workspaceId] ??= []).push(d)
    return by
  }, [files.docs, pinnedKeys])
  const memberIdSet = useMemo(() => new Set(show.members.map((m) => m.workspaceId)), [show.members])
  const loose = useMemo(
    () => Object.entries(filesByAgent).filter(([ws]) => !memberIdSet.has(ws)).flatMap(([, d]) => d),
    [filesByAgent, memberIdSet],
  )
  const toast = (text: string, kind: 'success' | 'error'): void => useToastStore.getState().addToast(text, kind)

  const toggleFold = (id: string): void => {
    setFolded((prev) => {
      const next = prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id]
      try {
        localStorage.setItem(FOLDED_KEY, JSON.stringify(next))
      } catch {
        /* ignore */
      }
      return next
    })
  }

  const makePoc = async (m: ProjectGroupMemberInfo): Promise<void> => {
    const name = memberDisplayName(m)
    try {
      await setProjectGroupPoc(primaryScope(), show.id, m.workspaceId)
      toast(`${name} is now the point of contact and gets the project chat.`, 'success')
      onChanged()
    } catch (err) {
      toast(`Couldn’t make ${name} the point of contact. Nothing changed. The server said: ${createErrorMessage(err)}`, 'error')
    }
  }

  const remove = async (m: ProjectGroupMemberInfo): Promise<void> => {
    setConfirmRemove(null)
    const name = memberDisplayName(m)
    try {
      await removeProjectGroupMember(primaryScope(), show.id, m.workspaceId)
      toast(`Removed ${name} from ${show.name}.`, 'success')
      onChanged()
    } catch (err) {
      toast(`Couldn’t remove ${name}. Nothing changed. The server said: ${createErrorMessage(err)}`, 'error')
    }
  }

  const fileRow = (doc: ProjectGroupHtmlDoc): React.JSX.Element => (
    <FileRow
      key={resourceDocKey(doc)}
      doc={doc}
      onDashboard={dashboard !== null && dashboardShowsDoc(dashboard.layoutJson, show.pocWorkspaceId, doc)}
      dashboardName={dashboard?.name ?? null}
      isPinned={pinnedKeys.includes(resourceDocKey(doc))}
      onTogglePin={(d) => setPinnedKeys(togglePinnedResource(show.id, d))}
      onRemoved={() => files.drop(doc)}
    />
  )

  return (
    <section aria-label={`${show.name}: agents and files`} data-project-nav-section="">
      <div className="flex items-center gap-2.5 px-4 pt-3 pb-1">
        <span className={AVATAR_ROUND}>
          <ProjectGroupAvatar name={show.name} groupId={show.id} size={22} color={show.color} />
        </span>
        <h3 className="min-w-0 flex-1 type-heading text-[var(--color-text-primary)] truncate">{show.name}</h3>
        <span className="flex-shrink-0 type-hint text-[var(--color-text-secondary)]" data-project-section-count="">
          {agentCountText(show.members.length)}
        </span>
      </div>
      <h4 className="px-4 pt-2 pb-1 type-hint font-semibold text-[var(--color-text-secondary)]">Agents</h4>
      <div className="px-1">
        {members.length === 0 ? (
          <p className="px-3 pb-2 type-hint text-[var(--color-text-secondary)]">
            No agents yet. The first one you add becomes the point of contact.
          </p>
        ) : (
          members.map((m) => {
            const mine = filesByAgent[m.workspaceId] ?? []
            const isFolded = folded.includes(m.workspaceId)
            return (
              <div key={m.workspaceId} role="group" aria-label={memberDisplayName(m)}>
                <AgentRow
                  show={show}
                  member={m}
                  isPoc={m.workspaceId === show.pocWorkspaceId}
                  isPinned={pinnedIds.includes(m.workspaceId)}
                  liveness={liveness[m.workspaceId]}
                  files={mine.length}
                  folded={isFolded}
                  canEdit={canAddAgents}
                  onFold={() => toggleFold(m.workspaceId)}
                  onTogglePin={(id) => setPinnedIds(togglePinnedMember(show.id, id))}
                  onMakePoc={() => void makePoc(m)}
                  onRemove={() => setConfirmRemove(m)}
                />
                {!isFolded && mine.length > 0 && <div data-agent-files={m.workspaceId}>{mine.map(fileRow)}</div>}
              </div>
            )
          })
        )}
        {loose.length > 0 && (
          <div role="group" aria-label="Other files">
            <h4 className="px-3 pt-2 pb-1 type-hint font-semibold text-[var(--color-text-secondary)]">Other files</h4>
            {loose.map(fileRow)}
          </div>
        )}
        {files.error && files.docs === null && (
          <div className="px-3 py-2 space-y-1" role="alert">
            <p className="type-hint text-[var(--color-text-primary)]">Couldn’t load the agents’ files.</p>
            <p className="type-hint text-[var(--color-text-secondary)] break-words">The server said: {files.error}</p>
            <Button variant="quiet" onClick={files.reload}>
              Try again
            </Button>
          </div>
        )}
        {canAddAgents && (
          <button
            ref={addRef}
            type="button"
            data-k2-focus=""
            className={cx(
              'relative w-full flex items-center gap-2 pl-6 pr-3 py-1.5 text-left no-drag outline-none cursor-pointer text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]',
              SETTING_LIST_ROW_HOVER,
            )}
            onClick={() => setAdding(true)}
            data-add-agent=""
          >
            <span className="flex-shrink-0 w-[18px] flex items-center justify-center" aria-hidden="true">
              <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-3.5 h-3.5">
                <path d="M8 3.5v9M3.5 8h9" />
              </svg>
            </span>
            <span className="type-control">Add an agent…</span>
          </button>
        )}
      </div>

      {adding && (
        <AddAgentDialog
          show={show}
          onChanged={onChanged}
          onClose={() => {
            setAdding(false)
            addRef.current?.focus({ preventScroll: true })
          }}
        />
      )}
      {confirmRemove && (
        <ConfirmSheet
          testId="remove-agent-confirm"
          title={`Remove ${memberDisplayName(confirmRemove)} from ${show.name}?`}
          message="It stops getting this project’s chat and leaves its dashboards. The agent and its folder stay as they are, and you can add it back any time."
          confirmLabel="Remove from project"
          destructive
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => void remove(confirmRemove)}
        />
      )}
    </section>
  )
}

/** The rail's form: the selected project's agents as pictures with their
 *  awake dot, under a divider, then "+" Add an agent. Click opens or
 *  focuses the tile; drag places it. */
export function RailAgents({
  show,
  canAddAgents,
  onChanged,
}: {
  show: ProjectGroupShow
  canAddAgents: boolean
  onChanged: () => void
}): React.JSX.Element | null {
  const projects = useProjectsStore((s) => s.projects)
  const readOnly = useWindowModeStore((s) => s.resolved && s.mode === 'viewer')
  const suppressClick = useRef(false)
  const addRef = useRef<HTMLButtonElement>(null)
  const [adding, setAdding] = useState(false)
  const pinned = useMemo(() => loadPinnedMemberIds(show.id), [show.id])
  const members = useMemo(() => sortMembersPinnedFirst(show.members, pinned).sorted, [show.members, pinned])
  const liveness = useMembersLiveness(show.id, useMemo(() => show.members.map((m) => m.workspaceId), [show.members]))
  const box =
    'no-drag relative flex items-center justify-center w-9 h-9 flex-shrink-0 outline-none rounded-[var(--radius-field)] select-none'
  return (
    <div className="flex flex-col items-center gap-1 w-full min-h-0 pt-1.5" aria-label={`${show.name}: agents`} role="group">
      <div aria-hidden="true" className="w-6 border-b border-[var(--color-border)] mb-1 flex-shrink-0" />
      <div className="flex flex-col items-center gap-1 w-full min-h-0 overflow-y-auto overflow-x-hidden">
        {members.map((m) => {
          const ws = projects.find((p) => p.id === m.workspaceId)
          const name = memberDisplayName(m)
          const l = liveness[m.workspaceId]
          return (
            <button
              key={m.workspaceId}
              type="button"
              data-k2-focus=""
              data-rail-agent={m.workspaceId}
              aria-label={name}
              title={[name, m.workspaceId === show.pocWorkspaceId ? 'Point of contact' : null, l === 'asleep' ? 'asleep' : null]
                .filter(Boolean)
                .join(', ')}
              className={cx(box, SETTING_LIST_ROW_HOVER, readOnly ? 'cursor-default' : 'cursor-pointer')}
              onClick={() => {
                if (suppressClick.current) {
                  suppressClick.current = false
                  return
                }
                useProjectGroupsStore.getState().requestMemberPane(m.workspaceId)
              }}
              onMouseDown={(e) => {
                if (readOnly) return
                startDashDrag(e, { type: 'member', workspaceId: m.workspaceId }, suppressClick)
              }}
            >
              <ProjectAvatar
                projectPath={m.path ?? ''}
                projectName={m.name ?? name}
                projectColor={ws?.color ?? 'var(--color-text-muted)'}
                projectId={m.workspaceId}
                iconUrl={ws?.iconUrl ?? null}
                size={20}
              />
              {l && (
                <span
                  aria-hidden="true"
                  data-liveness={l}
                  className={cx(
                    'absolute bottom-1 right-1 w-2 h-2 rounded-[var(--radius-badge)] ring-2 ring-[var(--color-bg-surface)]',
                    l === 'awake' ? 'bg-[var(--color-status-ok)]' : 'bg-[var(--color-text-muted)]',
                  )}
                />
              )}
            </button>
          )
        })}
        {canAddAgents && (
          <button
            ref={addRef}
            type="button"
            data-k2-focus=""
            aria-label="Add an agent"
            title="Add an agent"
            className={cx(box, SETTING_LIST_ROW_HOVER, 'cursor-pointer text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]')}
            onClick={() => setAdding(true)}
            data-rail-add-agent=""
          >
            <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={1.75} strokeLinecap="round" className="w-4 h-4">
              <path d="M8 3.5v9M3.5 8h9" />
            </svg>
          </button>
        )}
      </div>
      {adding && (
        <AddAgentDialog
          show={show}
          onChanged={onChanged}
          onClose={() => {
            setAdding(false)
            addRef.current?.focus({ preventScroll: true })
          }}
        />
      )}
    </div>
  )
}
