// Changes tab (prd-changes-tab-v1, C2): the workspace's git changes in the
// main area. List and actions on the left, the diff (C3's viewer, through
// `ChangesDiffSlot`) on the right. Data is the daemon's — one live
// `useGitChanges` store per workspace on the ROOM's server, refreshed by
// `git_changed`; this pane only holds what it is looking at (selection,
// diff mode, base, view, list layout), saved in the layout item.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useRoom, useRoomProjects, useRoomTabs } from '@/components/Room/RoomContext'
import { scopeMayWrite } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { useRoomResolvedAgentCommand } from '@/hooks/useResolvedAgentCommand'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import {
  commitStaged,
  discardFiles,
  fetchDiffSides,
  initRepo,
  stageAllChanges,
  stageFiles,
  unstageFiles,
  useGitChanges,
  type ChangesFile,
  type GitSnapshotRepo,
} from '@/stores/git-changes'
import {
  addNote,
  deleteNotes,
  resolveNotes,
  sendNotes,
  updateNote,
  useGitNotes,
  type NoteCounts,
  type ReviewNote,
} from '@/stores/git-notes'
import type { ChangesItemData, ChangesListLayout } from '@/stores/tabs'
import { ChangesList, changesRowKey } from './ChangesList'
import { ChangesDiffSlot } from './ChangesDiffSlot'
import { ChangesNotesList } from './ChangesNotesList'
import type { ChangesNotesApi, DiffSidesState } from './diff-slot-types'
import { buildAiCommitPrompt } from './ai-commit'

export interface ChangesPaneProps {
  tabId: string
  paneGroupId: string
  itemId: string
  data: ChangesItemData
}

/** The row the item's saved selection points at: same group + path, else the
 *  same path in another group (it was just staged / unstaged), else null. */
export function resolveSelection(files: ChangesFile[], data: Pick<ChangesItemData, 'selectedPath' | 'selectedGroup'>): ChangesFile | null {
  if (!data.selectedPath) return null
  const exact = files.find((f) => f.path === data.selectedPath && f.group === data.selectedGroup)
  if (exact) return exact
  return files.find((f) => f.path === data.selectedPath) ?? null
}

/** Confirm copy for a discard (untracked files go to the Trash). */
export function discardConfirmCopy(files: ChangesFile[]): { title: string; message: string; confirmLabel: string } {
  const untracked = files.filter((f) => f.group === 'untracked')
  const tracked = files.length - untracked.length
  const one = files.length === 1 ? files[0].path : null
  if (tracked === 0) {
    return {
      title: 'Move to Trash?',
      message: `Move ${one ?? `${files.length} untracked files`} to the Trash? ${
        one ? 'It is' : 'They are'
      } not deleted; you can restore from the Trash.`,
      confirmLabel: 'Move to Trash',
    }
  }
  let message = `Discard your changes to ${one ?? `${files.length} files`}? The current copy goes to the Trash first, then the file is restored from the index.`
  if (untracked.length > 0) {
    message += ` ${untracked.length} untracked file${untracked.length === 1 ? '' : 's'} move${untracked.length === 1 ? 's' : ''} to the Trash.`
  }
  return { title: 'Discard changes?', message, confirmLabel: 'Discard' }
}

export function ChangesPane({ tabId, paneGroupId, itemId, data }: ChangesPaneProps): React.JSX.Element {
  const room = useRoom()
  const scope = room.scope
  const workspacePath = data.workspacePath
  const base = data.base ?? null
  const view = data.view ?? 'single'
  const mode = data.mode ?? 'inline'
  const listLayout: ChangesListLayout = data.listLayout ?? 'list'

  const { snapshot, error, loading } = useGitChanges(scope, workspacePath, base)
  const setItemState = useRoomTabs((s) => s.setChangesItemState)
  const patch = useCallback(
    (p: Parameters<typeof setItemState>[3]) => setItemState(tabId, paneGroupId, itemId, p),
    [setItemState, tabId, paneGroupId, itemId],
  )

  const canStage = scopeMayWrite(scope, 'git/stage')
  const canDiscard = scopeMayWrite(scope, 'git/discard')
  const canCommit = scopeMayWrite(scope, 'git/commit')

  const repo: GitSnapshotRepo | null = snapshot && snapshot.isRepo ? snapshot : null
  const files = repo?.files ?? EMPTY_FILES
  const rev = snapshot?.rev ?? null

  // ── Selection ──────────────────────────────────────────────────────────
  const saved = resolveSelection(files, data)
  // Nothing saved (or it left the list): show the first row without saving.
  const file = saved ?? (view === 'single' ? files[0] ?? null : null)
  const selectedKey = file ? changesRowKey(file) : null

  const onSelect = useCallback(
    (f: ChangesFile) => patch({ selectedPath: f.path, selectedGroup: f.group }),
    [patch],
  )

  // ── The selected file's sides (refetched when rev moves) ───────────────
  const [sides, setSides] = useState<DiffSidesState>({ status: 'idle' })
  const sidesSeq = useRef(0)
  const fileGroup = file?.group
  const filePath = file?.path
  const fileOld = file?.oldPath ?? null
  useEffect(() => {
    const seq = ++sidesSeq.current
    if (view !== 'single' || !filePath || !fileGroup) {
      setSides({ status: 'idle' })
      return
    }
    setSides((prev) => ({ status: 'loading', data: 'data' in prev && prev.data?.path === filePath ? prev.data : undefined }))
    fetchDiffSides(scope, workspacePath, { group: fileGroup, path: filePath, oldPath: fileOld }, base).then(
      (d) => {
        if (seq === sidesSeq.current) setSides({ status: 'ready', data: d })
      },
      (e: unknown) => {
        if (seq !== sidesSeq.current) return
        setSides((prev) => ({
          status: 'error',
          error: e instanceof Error ? e.message : String(e),
          data: 'data' in prev ? prev.data : undefined,
        }))
      },
    )
  }, [scope, workspacePath, base, view, fileGroup, filePath, fileOld, rev])

  const loadSides = useCallback(
    (f: ChangesFile) => fetchDiffSides(scope, workspacePath, f, base),
    [scope, workspacePath, base],
  )

  // ── "Open in editor" (room-aware: the room's tabs, the room's server) ──
  const repoRoot = repo?.repoRoot ?? null
  const onOpenFile = useCallback(
    (rel: string) => {
      const root = repoRoot ?? workspacePath
      room.tabs.getState().openFileAsTab(`${root.replace(/\/+$/, '')}/${rel}`)
    },
    [room, repoRoot, workspacePath],
  )

  // ── Review notes (C5) ──────────────────────────────────────────────────
  const isRepo = repo !== null
  const { list: notesList } = useGitNotes(scope, isRepo ? workspacePath : null)
  const canNote = scopeMayWrite(scope, 'git/notes/add')
  const canSendNotes = scopeMayWrite(scope, 'git/notes/send')
  const notesApi: ChangesNotesApi | undefined = useMemo(() => {
    if (!isRepo) return undefined
    const toast = (label: string, e: unknown): never => {
      useToastStore.getState().addToast(`${label}: ${e instanceof Error ? e.message : String(e)}`, 'error')
      throw e
    }
    return {
      notes: notesList?.notes ?? EMPTY_NOTES,
      canWrite: canNote,
      add: async (input) => {
        await addNote(scope, workspacePath, input).catch((e: unknown) => toast('Note not saved', e))
      },
      update: async (id, text) => {
        await updateNote(scope, workspacePath, id, text).catch((e: unknown) => toast('Note not saved', e))
      },
      resolve: async (id) => {
        await resolveNotes(scope, workspacePath, [id]).catch((e: unknown) => toast('Resolve failed', e))
      },
      remove: async (id) => {
        await deleteNotes(scope, workspacePath, [id]).catch((e: unknown) => toast('Delete failed', e))
      },
    }
  }, [isRepo, notesList, canNote, scope, workspacePath])

  // ── Actions ────────────────────────────────────────────────────────────
  const [busy, setBusy] = useState(false)
  const run = useCallback(async (label: string, fn: () => Promise<unknown>) => {
    setBusy(true)
    try {
      await fn()
    } catch (e) {
      useToastStore.getState().addToast(`${label}: ${e instanceof Error ? e.message : String(e)}`, 'error')
    } finally {
      setBusy(false)
    }
  }, [])

  const onStage = useCallback(
    (fs: ChangesFile[]) => void run('Stage failed', () => stageFiles(scope, workspacePath, fs.map((f) => f.path))),
    [run, scope, workspacePath],
  )
  const onUnstage = useCallback(
    (fs: ChangesFile[]) => void run('Unstage failed', () => unstageFiles(scope, workspacePath, fs.map((f) => f.path))),
    [run, scope, workspacePath],
  )
  const onStageAll = useCallback(
    () => void run('Stage all failed', () => stageAllChanges(scope, workspacePath)),
    [run, scope, workspacePath],
  )
  const onDiscard = useCallback(
    async (fs: ChangesFile[]) => {
      if (fs.length === 0) return
      const copy = discardConfirmCopy(fs)
      const ok = await useConfirmDialogStore.getState().confirm({ ...copy, destructive: true })
      if (!ok) return
      await run('Discard failed', async () => {
        const result = await discardFiles(scope, workspacePath, fs.map((f) => f.path))
        if (result?.failed?.length) {
          useToastStore
            .getState()
            .addToast(
              `Not discarded (left untouched): ${result.failed.map((f) => `${f.path} — ${f.error}`).join('; ')}`,
              'error',
            )
        }
      })
    },
    [run, scope, workspacePath],
  )

  // ── Notes list actions ─────────────────────────────────────────────────
  const onSendNotes = useCallback(
    () =>
      void run('Send to agent failed', async () => {
        const r = await sendNotes(scope, workspacePath)
        const more = r.remaining > 0 ? ` (${r.remaining} more next time)` : ''
        useToastStore.getState().addToast(`Sent ${r.sent} note${r.sent === 1 ? '' : 's'} to the agent${more}`, 'success')
      }),
    [run, scope, workspacePath],
  )
  const onSelectNote = useCallback(
    (n: ReviewNote) => {
      const row = files.find((f) => f.path === n.path && f.group === data.selectedGroup) ?? files.find((f) => f.path === n.path)
      if (row) patch({ selectedPath: row.path, selectedGroup: row.group })
    },
    [files, data.selectedGroup, patch],
  )
  const onResolveNote = useCallback(
    (n: ReviewNote, resolved: boolean) =>
      void run(resolved ? 'Resolve failed' : 'Reopen failed', () => resolveNotes(scope, workspacePath, [n.id], resolved)),
    [run, scope, workspacePath],
  )
  const onEditNote = useCallback(
    async (n: ReviewNote, text: string) => {
      await run('Note not saved', () => updateNote(scope, workspacePath, n.id, text))
    },
    [run, scope, workspacePath],
  )
  const onDeleteNote = useCallback(
    async (n: ReviewNote) => {
      const ok = await useConfirmDialogStore.getState().confirm({
        title: 'Delete note?',
        message: `Delete the note on ${n.path}:${n.startLine}? This can't be undone.`,
        confirmLabel: 'Delete',
        destructive: true,
      })
      if (ok) await run('Delete failed', () => deleteNotes(scope, workspacePath, [n.id]))
    },
    [run, scope, workspacePath],
  )

  // ── Initialize repository (not a repo yet) ─────────────────────────────
  const canInit = scopeMayWrite(scope, 'git/init')
  const onInit = useCallback(
    () => void run('Initialize failed', () => initRepo(scope, workspacePath)),
    [run, scope, workspacePath],
  )

  // ── Commit ─────────────────────────────────────────────────────────────
  const [message, setMessage] = useState('')
  const stagedCount = repo?.counts.staged ?? 0
  const onCommit = useCallback(async () => {
    const msg = message.trim()
    if (!msg || stagedCount === 0) return
    await run('Commit failed', async () => {
      await commitStaged(scope, workspacePath, msg)
      setMessage('')
    })
  }, [message, stagedCount, run, scope, workspacePath])

  // ── AI Commit (kept from the drawer) ───────────────────────────────────
  const workspace = useRoomProjects((projects) => {
    for (const p of projects) {
      const wt = p.workspaces.find((w) => w.worktreePath === workspacePath)
      if (wt) return wt
    }
    const project = projects.find((p) => p.path === workspacePath)
    return project?.workspaces.find((w) => !w.worktreePath) ?? project?.workspaces[0] ?? null
  })
  const resolvedAgent = useRoomResolvedAgentCommand(workspace?.id, { projectPath: workspacePath })
  const isWorktree = workspace?.type === 'worktree'
  const onAiCommit = useCallback(
    (includeMerge: boolean) => {
      if (!resolvedAgent) {
        useToastStore.getState().addToast('No enabled agent to run AI Commit', 'error')
        return
      }
      const prompt = buildAiCommitPrompt(files, { includeMerge, branchName: repo?.branch ?? 'current branch' })
      const tabs = room.tabs.getState()
      tabs.addTabToGroup(tabs.activeGroupIndex, workspacePath, {
        title: includeMerge ? 'AI Commit & Merge' : 'AI Commit',
        command: resolvedAgent.command,
        args: [...resolvedAgent.args, prompt],
      })
    },
    [resolvedAgent, files, repo?.branch, room, workspacePath],
  )

  // ── Base picker (branches load on first open of the picker) ────────────
  const [branches, setBranches] = useState<string[] | null>(null)
  const loadBranches = useCallback(() => {
    if (branches) return
    daemonCliGet<{ current: string; local: string[]; remote: string[] }>(scope, 'git/branches', { path: workspacePath }).then(
      (b) => setBranches([...(b?.local ?? []), ...(b?.remote ?? [])].filter((n) => n && n !== b?.current)),
      (e: unknown) => useToastStore.getState().addToast(`Could not list branches: ${e instanceof Error ? e.message : String(e)}`, 'error'),
    )
  }, [branches, scope, workspacePath])
  const baseOptions = useMemo(() => {
    const list = branches ?? []
    return base && !list.includes(base) ? [base, ...list] : list
  }, [branches, base])

  // ── Render ─────────────────────────────────────────────────────────────
  if (!snapshot) {
    return (
      <Centered>
        {error ? `Could not read changes: ${error}` : loading ? 'Loading changes…' : 'Loading changes…'}
      </Centered>
    )
  }
  if (!repo) {
    return (
      <div data-changes-pane="" data-changes-not-repo="" className="flex h-full w-full items-center justify-center p-4">
        <div className="max-w-sm text-center">
          <p className="text-xs text-[var(--color-text-primary)]">This workspace isn&apos;t a git repository</p>
          <p className="mt-1 text-[11px] text-[var(--color-text-muted)]">
            Initialize one here to track changes, review them and send notes to the agent.
          </p>
          {canInit ? (
            <button
              type="button"
              data-changes-init=""
              className="mt-3 bg-[var(--color-accent)] px-3 py-1.5 text-xs font-medium text-[var(--color-on-accent)] hover:opacity-90 disabled:opacity-40"
              disabled={busy}
              onClick={onInit}
            >
              {busy ? 'Initializing…' : 'Initialize repository'}
            </button>
          ) : (
            <p className="mt-2 text-[10px] text-[var(--color-text-muted)]">View only: this room cannot change files on its server.</p>
          )}
        </div>
      </div>
    )
  }

  const seg = (on: boolean): string =>
    `px-1.5 py-0.5 text-[10px] ${on ? 'bg-white/[0.12] text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'}`

  return (
    <div data-changes-pane="" className="flex h-full w-full min-h-0 bg-[var(--color-bg)]">
      <div className="flex w-[320px] min-w-[240px] flex-shrink-0 flex-col border-r border-[var(--color-border)]">
        {/* Branch + compare */}
        <div className="flex flex-col gap-1 border-b border-[var(--color-border)] px-2 py-1.5">
          <div className="flex items-center gap-2 text-xs">
            <span data-changes-branch="" className="truncate font-medium text-[var(--color-text-primary)]">
              {repo.branch ?? (repo.headOid ? `(detached ${repo.headOid.slice(0, 7)})` : '(no branch)')}
            </span>
            {(repo.ahead > 0 || repo.behind > 0) && (
              <span className="flex-shrink-0 text-[10px] tabular-nums text-[var(--color-text-muted)]">
                {repo.ahead > 0 && <span className="text-[var(--color-status-ok-soft)]">↑{repo.ahead}</span>}
                {repo.ahead > 0 && repo.behind > 0 && ' '}
                {repo.behind > 0 && <span className="text-[var(--color-status-error-soft)]">↓{repo.behind}</span>}
              </span>
            )}
            {repo.state !== 'clean' && (
              <span className="flex-shrink-0 text-[10px] uppercase text-[var(--color-status-warn-text)]">{repo.state.replace('_', ' ')}</span>
            )}
            <span className="flex-1" />
            {loading && <span className="text-[10px] text-[var(--color-text-muted)]">…</span>}
          </div>
          <div className="flex items-center gap-1.5 text-[10px] text-[var(--color-text-muted)]">
            <label className="flex min-w-0 flex-1 items-center gap-1">
              <span>Compare</span>
              <select
                data-changes-base=""
                aria-label="Compare against"
                className="min-w-0 flex-1 bg-[var(--color-bg-elevated)] text-[10px] text-[var(--color-text-secondary)] outline-none"
                value={base ?? ''}
                onFocus={loadBranches}
                onMouseDown={loadBranches}
                onChange={(e) => patch({ base: e.target.value || null, selectedPath: null, selectedGroup: null })}
              >
                <option value="">HEAD (staged / unstaged)</option>
                {baseOptions.map((b) => (
                  <option key={b} value={b}>
                    {b}
                  </option>
                ))}
              </select>
            </label>
            {repo.base && (
              <span className="flex-shrink-0 tabular-nums" title="HEAD vs the base branch">
                ↑{repo.base.ahead} ↓{repo.base.behind}
              </span>
            )}
          </div>
          <div className="flex items-center gap-2">
            <span className="flex" role="group" aria-label="List layout">
              <button type="button" className={seg(listLayout === 'list')} aria-pressed={listLayout === 'list'} onClick={() => patch({ listLayout: 'list' })}>
                List
              </button>
              <button type="button" className={seg(listLayout === 'tree')} aria-pressed={listLayout === 'tree'} onClick={() => patch({ listLayout: 'tree' })}>
                Tree
              </button>
            </span>
            <span className="flex" role="group" aria-label="Diff view">
              <button type="button" className={seg(view === 'single')} aria-pressed={view === 'single'} onClick={() => patch({ view: 'single' })}>
                File
              </button>
              <button type="button" className={seg(view === 'all')} aria-pressed={view === 'all'} onClick={() => patch({ view: 'all' })}>
                All
              </button>
            </span>
            <span className="flex" role="group" aria-label="Diff layout">
              <button type="button" className={seg(mode === 'inline')} aria-pressed={mode === 'inline'} onClick={() => patch({ mode: 'inline' })}>
                Inline
              </button>
              <button type="button" className={seg(mode === 'split')} aria-pressed={mode === 'split'} onClick={() => patch({ mode: 'split' })}>
                Split
              </button>
            </span>
            <span className="flex-1" />
            {canStage && !base && repo.counts.unstaged + repo.counts.untracked > 0 && (
              <button
                type="button"
                className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-40"
                disabled={busy}
                onClick={onStageAll}
              >
                Stage all
              </button>
            )}
          </div>
        </div>

        {repo.truncated && (
          <div data-changes-truncated="" className="border-b border-[var(--color-border)] px-2 py-1 text-[10px] text-[var(--color-status-warn-text)]">
            Too many changes: showing the first {repo.limit.toLocaleString()} of {repo.counts.total.toLocaleString()}.
          </div>
        )}
        {error && (
          <div className="border-b border-[var(--color-border)] px-2 py-1 text-[10px] text-[var(--color-status-error-soft)]">
            Refresh failed: {error}
          </div>
        )}

        <div className="min-h-0 flex-1 overflow-y-auto">
          {repo.counts.total === 0 ? (
            <p className="px-3 py-6 text-center text-xs text-[var(--color-text-muted)]">
              {base ? `No changes against ${base}` : 'Working tree clean'}
            </p>
          ) : (
            <ChangesList
              files={files}
              counts={repo.counts}
              layout={listLayout}
              selectedKey={selectedKey}
              canStage={canStage}
              canDiscard={canDiscard}
              busy={busy}
              onSelect={onSelect}
              onStage={onStage}
              onUnstage={onUnstage}
              onDiscard={(fs) => void onDiscard(fs)}
            />
          )}
          {notesList && (
            <ChangesNotesList
              notes={notesList.notes}
              counts={notesList.counts ?? EMPTY_COUNTS}
              canWrite={canNote && canSendNotes}
              busy={busy}
              onSend={onSendNotes}
              onSelect={onSelectNote}
              onResolve={onResolveNote}
              onEdit={onEditNote}
              onDelete={(n) => void onDeleteNote(n)}
            />
          )}
        </div>

        {!canStage && (
          <div className="border-t border-[var(--color-border)] px-2 py-1 text-[10px] text-[var(--color-text-muted)]">
            View only: this room cannot change files on its server.
          </div>
        )}

        {repo.counts.total > 0 && canStage && (
          <div className="flex gap-1 border-t border-[var(--color-border)] p-2">
            <button
              type="button"
              className="flex-1 bg-[var(--color-accent)] px-2 py-1 text-xs font-medium text-[var(--color-on-accent)] hover:opacity-90"
              onClick={() => onAiCommit(false)}
            >
              AI Commit
            </button>
            {isWorktree && (
              <button
                type="button"
                className="flex-1 bg-[var(--color-accent)] px-2 py-1 text-xs font-medium text-[var(--color-on-accent)] hover:opacity-90"
                onClick={() => onAiCommit(true)}
              >
                AI Commit & Merge
              </button>
            )}
          </div>
        )}

        {canCommit && (
          <div className="border-t border-[var(--color-border)] p-2">
            <textarea
              data-changes-commit-message=""
              className="w-full resize-none border border-[var(--color-border)] bg-[var(--color-bg-elevated)] px-2 py-1.5 text-xs text-[var(--color-text-primary)] outline-none placeholder:text-[var(--color-text-muted)] focus:border-[var(--color-accent)]"
              placeholder={stagedCount > 0 ? 'Commit message (⌘/Ctrl+Enter)' : 'Stage files to commit'}
              rows={3}
              value={message}
              onChange={(e) => setMessage(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                  e.preventDefault()
                  void onCommit()
                }
              }}
            />
            <button
              type="button"
              data-changes-commit=""
              className="mt-1 w-full bg-[var(--color-accent)] px-3 py-1.5 text-xs font-medium text-[var(--color-on-accent)] hover:opacity-90 disabled:cursor-not-allowed disabled:opacity-40"
              disabled={!message.trim() || stagedCount === 0 || busy}
              onClick={() => void onCommit()}
            >
              {busy ? 'Working…' : `Commit (${stagedCount} file${stagedCount === 1 ? '' : 's'})`}
            </button>
          </div>
        )}
      </div>

      <div className="min-w-0 flex-1">
        <ChangesDiffSlot
          workspacePath={workspacePath}
          view={view}
          mode={mode}
          base={base}
          file={view === 'single' ? file : null}
          files={files}
          sides={view === 'single' ? sides : IDLE}
          rev={rev}
          loadSides={loadSides}
          onSelectFile={onSelect}
          onOpenFile={onOpenFile}
          notes={notesApi}
        />
      </div>
    </div>
  )
}

const EMPTY_FILES: ChangesFile[] = []
const EMPTY_NOTES: ReviewNote[] = []
const EMPTY_COUNTS: NoteCounts = { open: 0, unsent: 0, sent: 0, resolved: 0 }
const IDLE: DiffSidesState = { status: 'idle' }

function Centered({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div data-changes-pane="" className="flex h-full w-full items-center justify-center p-4">
      <p className="text-xs text-[var(--color-text-muted)]">{children}</p>
    </div>
  )
}
