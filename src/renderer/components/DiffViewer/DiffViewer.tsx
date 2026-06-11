import { useState, useEffect, useMemo } from 'react'
// Plan B — git diff is host-aware daemon data: route it through the
// `/cli/git/*` HTTP layer (local OR remote) instead of the localhost-pinned
// Tauri `git_*` invoke proxy. GET params are snake_case (`path`,
// `file_path`); the JSON DiffHunk[] response matches the Rust struct as-is.
import { daemonCliGet } from '@/lib/daemon-cli'
import { useProjectsStore } from '@/stores/projects'

// ── Types ────────────────────────────────────────────────────────────────

interface DiffLine {
  kind: 'add' | 'remove' | 'context'
  content: string
}

interface DiffHunk {
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  lines: DiffLine[]
}

interface DiffViewerProps {
  filePath: string
  className?: string
}

// ── Component ────────────────────────────────────────────────────────────

export function DiffViewer({ filePath, className }: DiffViewerProps): React.JSX.Element {
  const [hunks, setHunks] = useState<DiffHunk[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)
  const projects = useProjectsStore((s) => s.projects)

  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const repoPath = activeWorkspace?.worktreePath ?? activeProject?.path

  useEffect(() => {
    if (!repoPath || !filePath) return
    setLoading(true)
    setError(null)

    daemonCliGet<DiffHunk[]>('git/diff-file', { path: repoPath, file_path: filePath })
      .then((result) => {
        setHunks(result)
        setLoading(false)
      })
      .catch((e) => {
        setError(String(e))
        setLoading(false)
      })
  }, [repoPath, filePath])

  const fileName = filePath.split('/').pop() || filePath

  const stats = useMemo(() => {
    let additions = 0
    let deletions = 0
    for (const hunk of hunks) {
      for (const line of hunk.lines) {
        if (line.kind === 'add') additions++
        if (line.kind === 'remove') deletions++
      }
    }
    return { additions, deletions }
  }, [hunks])

  if (loading) {
    return (
      <div className={`flex items-center justify-center h-full ${className || ''}`}>
        <span className="text-xs text-[var(--color-text-muted)]">Loading diff...</span>
      </div>
    )
  }

  if (error) {
    return (
      <div className={`flex items-center justify-center h-full ${className || ''}`}>
        <span className="text-xs text-red-400">{error}</span>
      </div>
    )
  }

  if (hunks.length === 0) {
    return (
      <div className={`flex items-center justify-center h-full ${className || ''}`}>
        <span className="text-xs text-[var(--color-text-muted)]">No changes</span>
      </div>
    )
  }

  return (
    <div className={`flex flex-col h-full overflow-hidden ${className || ''}`}>
      {/* File header */}
      <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)]">
        <span className="text-xs font-medium text-[var(--color-text-primary)]">{fileName}</span>
        <span className="text-[10px] text-[var(--color-text-muted)]">{filePath}</span>
        <div className="ml-auto flex items-center gap-2">
          {stats.additions > 0 && (
            <span className="text-[10px] text-green-400 font-mono">+{stats.additions}</span>
          )}
          {stats.deletions > 0 && (
            <span className="text-[10px] text-red-400 font-mono">-{stats.deletions}</span>
          )}
        </div>
      </div>

      {/* Diff content */}
      <div className="flex-1 overflow-auto font-mono text-xs leading-5">
        {hunks.map((hunk, hunkIdx) => (
          <div key={hunkIdx}>
            {/* Hunk header */}
            <div className="diff-hunk-header px-4 py-1 text-[var(--color-text-muted)] bg-[#1a1a2e] select-none">
              @@ -{hunk.oldStart},{hunk.oldCount} +{hunk.newStart},{hunk.newCount} @@
            </div>

            {/* Lines */}
            {hunk.lines.map((line, lineIdx) => {
              // Calculate line numbers
              let oldLineNum = hunk.oldStart
              let newLineNum = hunk.newStart
              for (let i = 0; i < lineIdx; i++) {
                const prev = hunk.lines[i]
                if (prev.kind === 'context') { oldLineNum++; newLineNum++ }
                else if (prev.kind === 'remove') { oldLineNum++ }
                else if (prev.kind === 'add') { newLineNum++ }
              }

              const lineClass =
                line.kind === 'add'
                  ? 'diff-line-add'
                  : line.kind === 'remove'
                    ? 'diff-line-remove'
                    : 'diff-line-context'

              const prefix = line.kind === 'add' ? '+' : line.kind === 'remove' ? '-' : ' '

              return (
                <div key={`${hunkIdx}-${lineIdx}`} className={`flex ${lineClass}`}>
                  {/* Old line number */}
                  <span className="diff-line-num w-10 text-right pr-2 select-none shrink-0">
                    {line.kind !== 'add' ? oldLineNum : ''}
                  </span>
                  {/* New line number */}
                  <span className="diff-line-num w-10 text-right pr-2 select-none shrink-0">
                    {line.kind !== 'remove' ? newLineNum : ''}
                  </span>
                  {/* Prefix */}
                  <span className="w-4 text-center select-none shrink-0">{prefix}</span>
                  {/* Content */}
                  <span className="flex-1 whitespace-pre overflow-x-auto">{line.content}</span>
                </div>
              )
            })}

            {/* Separator between hunks */}
            {hunkIdx < hunks.length - 1 && (
              <div className="diff-hunk-separator px-4 py-1 text-center text-[10px] text-[var(--color-text-muted)] select-none">
                ⋯
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  )
}
