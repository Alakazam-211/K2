// Changes tab — the file list (C2). Groups (conflicts, staged, unstaged,
// branch, untracked) with counts, status badges, +/- counts, list or tree
// layout, and per-file / per-group stage, unstage and discard. Pure view:
// every action is a callback; ChangesPane runs the routes and the confirm.

import { useMemo, useState } from 'react'
import type { ChangesFile, ChangesCounts } from '@/stores/git-changes'
import type { ChangesGroup, ChangesListLayout } from '@/stores/tabs'

// ── Groups and badges ────────────────────────────────────────────────────

export const GROUP_ORDER: ChangesGroup[] = ['conflicted', 'staged', 'unstaged', 'base', 'untracked']

export const GROUP_TITLE: Record<ChangesGroup, string> = {
  conflicted: 'Merge Conflicts',
  staged: 'Staged Changes',
  unstaged: 'Changes',
  base: 'Branch Changes',
  untracked: 'Untracked',
}

const STATUS_BADGE: Record<string, { label: string; color: string }> = {
  M: { label: 'Modified', color: 'var(--color-status-warn-text)' },
  A: { label: 'Added', color: 'var(--color-status-ok-soft)' },
  D: { label: 'Deleted', color: 'var(--color-status-error-soft)' },
  R: { label: 'Renamed', color: 'var(--color-accent)' },
  T: { label: 'Type changed', color: 'var(--color-status-warn-text)' },
  U: { label: 'Untracked', color: 'var(--color-text-muted)' },
  C: { label: 'Conflicted', color: 'var(--color-status-error-soft)' },
}

export function changesRowKey(file: Pick<ChangesFile, 'group' | 'path'>): string {
  return `${file.group}:${file.path}`
}

export function groupFiles(files: ChangesFile[]): Map<ChangesGroup, ChangesFile[]> {
  const out = new Map<ChangesGroup, ChangesFile[]>()
  for (const f of files) {
    const list = out.get(f.group)
    if (list) list.push(f)
    else out.set(f.group, [f])
  }
  return out
}

// ── Tree ─────────────────────────────────────────────────────────────────

interface TreeDir {
  name: string
  key: string
  dirs: Map<string, TreeDir>
  files: ChangesFile[]
}

function buildTree(group: ChangesGroup, files: ChangesFile[]): TreeDir {
  const root: TreeDir = { name: '', key: `${group}:`, dirs: new Map(), files: [] }
  for (const f of files) {
    const parts = f.path.replace(/\/$/, '').split('/')
    parts.pop()
    let node = root
    for (const part of parts) {
      let next = node.dirs.get(part)
      if (!next) {
        next = { name: part, key: `${node.key}${part}/`, dirs: new Map(), files: [] }
        node.dirs.set(part, next)
      }
      node = next
    }
    node.files.push(f)
  }
  return root
}

/** A dir with one child dir and no files folds into "a/b". */
function compact(dir: TreeDir): TreeDir {
  let node = dir
  while (node.files.length === 0 && node.dirs.size === 1) {
    const only = [...node.dirs.values()][0]
    node = { ...only, name: node.name ? `${node.name}/${only.name}` : only.name }
  }
  return node
}

// ── Props ────────────────────────────────────────────────────────────────

export interface ChangesListProps {
  files: ChangesFile[]
  counts: ChangesCounts
  layout: ChangesListLayout
  selectedKey: string | null
  /** May this room stage / unstage? */
  canStage: boolean
  /** May this room discard? */
  canDiscard: boolean
  busy: boolean
  onSelect: (file: ChangesFile) => void
  onStage: (files: ChangesFile[]) => void
  onUnstage: (files: ChangesFile[]) => void
  onDiscard: (files: ChangesFile[]) => void
}

export function ChangesList(props: ChangesListProps): React.JSX.Element {
  const grouped = useMemo(() => groupFiles(props.files), [props.files])
  return (
    <div data-changes-list="" className="flex flex-col">
      {GROUP_ORDER.map((group) => {
        const files = grouped.get(group) ?? []
        const total = props.counts[group] ?? files.length
        if (total === 0 && files.length === 0) return null
        return <GroupSection key={group} {...props} group={group} files={files} total={total} />
      })}
    </div>
  )
}

function groupActions(group: ChangesGroup): { stage: boolean; unstage: boolean; discard: boolean } {
  switch (group) {
    case 'staged':
      return { stage: false, unstage: true, discard: false }
    case 'unstaged':
    case 'untracked':
      return { stage: true, unstage: false, discard: true }
    case 'conflicted':
      // Staging a conflicted file marks it resolved. Discard is refused
      // (409) by the daemon, so it is not offered.
      return { stage: true, unstage: false, discard: false }
    case 'base':
      return { stage: false, unstage: false, discard: false }
  }
}

function GroupSection({
  group,
  files,
  total,
  layout,
  canStage,
  canDiscard,
  busy,
  onStage,
  onUnstage,
  onDiscard,
  ...rest
}: ChangesListProps & { group: ChangesGroup; total: number }): React.JSX.Element {
  const [collapsed, setCollapsed] = useState(false)
  const acts = groupActions(group)
  const rowProps: ChangesListProps = { ...rest, files, layout, canStage, canDiscard, busy, onStage, onUnstage, onDiscard }
  const btn =
    'text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-40 disabled:cursor-not-allowed'
  return (
    <div data-changes-group={group} className="mb-1">
      <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-2 py-1">
        <button
          type="button"
          className="flex min-w-0 flex-1 items-center gap-1 text-left text-xs font-medium text-[var(--color-text-secondary)]"
          onClick={() => setCollapsed((c) => !c)}
          aria-expanded={!collapsed}
        >
          <span className="w-3 text-[10px] text-[var(--color-text-muted)]">{collapsed ? '▸' : '▾'}</span>
          <span className="truncate">{GROUP_TITLE[group]}</span>
          <span data-changes-group-count="" className="tabular-nums text-[var(--color-text-muted)]">
            {total}
          </span>
        </button>
        {acts.discard && canDiscard && files.length > 0 && (
          <button type="button" className={btn} disabled={busy} onClick={() => onDiscard(files)}>
            {group === 'untracked' ? 'Trash All' : 'Discard All'}
          </button>
        )}
        {acts.stage && canStage && files.length > 0 && (
          <button type="button" className={btn} disabled={busy} onClick={() => onStage(files)}>
            Stage All
          </button>
        )}
        {acts.unstage && canStage && files.length > 0 && (
          <button type="button" className={btn} disabled={busy} onClick={() => onUnstage(files)}>
            Unstage All
          </button>
        )}
      </div>
      {!collapsed &&
        (layout === 'tree' ? (
          <TreeRows
            dir={buildTree(group, files)}
            depth={0}
            rowProps={rowProps}
          />
        ) : (
          files.map((f) => (
            <FileRow
              key={changesRowKey(f)}
              file={f}
              depth={0}
              showDir
              rowProps={rowProps}
            />
          ))
        ))}
    </div>
  )
}

function TreeRows({ dir, depth, rowProps }: { dir: TreeDir; depth: number; rowProps: ChangesListProps }): React.JSX.Element {
  const [collapsed, setCollapsed] = useState<Set<string>>(() => new Set())
  const dirs = [...dir.dirs.values()].map(compact).sort((a, b) => a.name.localeCompare(b.name))
  return (
    <>
      {dirs.map((d) => {
        const isCollapsed = collapsed.has(d.key)
        return (
          <div key={d.key}>
            <button
              type="button"
              data-changes-dir={d.key}
              className="flex w-full items-center gap-1 py-0.5 text-left text-xs text-[var(--color-text-muted)] hover:bg-white/[0.04]"
              style={{ paddingLeft: 8 + depth * 12 }}
              onClick={() =>
                setCollapsed((prev) => {
                  const next = new Set(prev)
                  if (next.has(d.key)) next.delete(d.key)
                  else next.add(d.key)
                  return next
                })
              }
            >
              <span className="w-3 text-[10px]">{isCollapsed ? '▸' : '▾'}</span>
              <span className="truncate">{d.name}</span>
            </button>
            {!isCollapsed && <TreeRows dir={d} depth={depth + 1} rowProps={rowProps} />}
          </div>
        )
      })}
      {dir.files.map((f) => (
        <FileRow key={changesRowKey(f)} file={f} depth={depth} showDir={false} rowProps={rowProps} />
      ))}
    </>
  )
}

function splitPath(path: string): { name: string; dir: string } {
  const trimmed = path.replace(/\/$/, '')
  const i = trimmed.lastIndexOf('/')
  const name = (i >= 0 ? trimmed.slice(i + 1) : trimmed) + (path.endsWith('/') ? '/' : '')
  return { name, dir: i >= 0 ? trimmed.slice(0, i) : '' }
}

function FileRow({
  file,
  depth,
  showDir,
  rowProps,
}: {
  file: ChangesFile
  depth: number
  showDir: boolean
  rowProps: ChangesListProps
}): React.JSX.Element {
  const { selectedKey, canStage, canDiscard, busy, onSelect, onStage, onUnstage, onDiscard } = rowProps
  const acts = groupActions(file.group)
  const key = changesRowKey(file)
  const selected = selectedKey === key
  const badge = STATUS_BADGE[file.status] ?? STATUS_BADGE.M
  const { name, dir } = splitPath(file.path)
  const iconBtn =
    'flex h-4 w-4 items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-40'
  return (
    <div
      data-changes-row={key}
      data-selected={selected ? '' : undefined}
      role="button"
      tabIndex={0}
      className={`group flex cursor-pointer items-center gap-1.5 py-0.5 pr-2 ${
        selected ? 'bg-white/[0.08]' : 'hover:bg-white/[0.04]'
      }`}
      style={{ paddingLeft: 8 + depth * 12 + (showDir ? 0 : 12) }}
      onClick={() => onSelect(file)}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onSelect(file)
        }
      }}
      title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
    >
      <span
        data-changes-status={file.status}
        className="flex h-4 w-4 flex-shrink-0 items-center justify-center text-[10px] font-bold"
        style={{ color: badge.color }}
        title={badge.label}
      >
        {file.status}
      </span>
      <span className="min-w-0 flex-1 truncate text-xs text-[var(--color-text-secondary)]">
        {file.oldPath ? <span className="text-[var(--color-text-muted)]">{splitPath(file.oldPath).name} → </span> : null}
        {name}
        {showDir && dir ? <span className="ml-1.5 text-[10px] text-[var(--color-text-muted)]">{dir}</span> : null}
      </span>
      <LineCounts file={file} />
      <span className="flex flex-shrink-0 items-center gap-0.5 opacity-0 group-hover:opacity-100 focus-within:opacity-100">
        {acts.discard && canDiscard && (
          <button
            type="button"
            className={iconBtn}
            disabled={busy}
            aria-label={file.group === 'untracked' ? `Move ${file.path} to Trash` : `Discard changes to ${file.path}`}
            title={file.group === 'untracked' ? 'Move to Trash' : 'Discard changes'}
            onClick={(e) => {
              e.stopPropagation()
              onDiscard([file])
            }}
          >
            ↺
          </button>
        )}
        {acts.stage && canStage && (
          <button
            type="button"
            className={iconBtn}
            disabled={busy}
            aria-label={`Stage ${file.path}`}
            title={file.group === 'conflicted' ? 'Mark resolved (stage)' : 'Stage'}
            onClick={(e) => {
              e.stopPropagation()
              onStage([file])
            }}
          >
            +
          </button>
        )}
        {acts.unstage && canStage && (
          <button
            type="button"
            className={iconBtn}
            disabled={busy}
            aria-label={`Unstage ${file.path}`}
            title="Unstage"
            onClick={(e) => {
              e.stopPropagation()
              onUnstage([file])
            }}
          >
            −
          </button>
        )}
      </span>
    </div>
  )
}

function LineCounts({ file }: { file: ChangesFile }): React.JSX.Element | null {
  if (file.dir) return null
  if (file.binary) {
    return <span className="flex-shrink-0 text-[10px] text-[var(--color-text-muted)]">bin</span>
  }
  if (file.additions == null && file.deletions == null) return null
  return (
    <span data-changes-lines="" className="flex-shrink-0 text-[10px] tabular-nums">
      {file.additions ? <span className="text-[var(--color-status-ok-soft)]">+{file.additions}</span> : null}
      {file.additions && file.deletions ? ' ' : null}
      {file.deletions ? <span className="text-[var(--color-status-error-soft)]">−{file.deletions}</span> : null}
    </span>
  )
}
