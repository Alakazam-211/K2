// Chat History → Imported Threads (0.46.1 clone polish).
//
// Threads that arrived with a clone (or `k2 thread import`) get their own
// group in the workspace's history. The daemon owns the list
// (`GET /cli/thread/imports?ws=`, table `overlay_imports`, migration 0180)
// and says where each one reads now (`addr`) and whether the whole Thread
// is imported history (`readOnly`); this group renders that and opens the
// Thread read-only in a viewer. Hidden when nothing was imported.

import { useCallback, useEffect, useState } from 'react'

import { Button, DialogFrame, DialogScrim } from '@/components/ui'
import { useRoom } from '@/components/Room/RoomContext'
import { daemonCliGet } from '@/lib/daemon-cli'
import { onChatHistoryChanged } from '@/stores/session-events'
import { ThreadOverlayColumn } from '@/components/SessionView/ThreadOverlayColumn'
import type { ServerScope } from '@/kessel/server-scope'

export interface ImportedThread {
  id: string
  localConversationId: string
  role: 'canonical' | 'pending_canonical' | 'sidecar' | 'detached' | string
  cloneId: string
  sourceHost: string
  sourceHandle: string
  name: string | null
  docCount: number
  importedAt: number
  /** Where to read it now; null = nowhere addressable (not openable). */
  addr: string | null
  /** The whole Thread is imported history. */
  readOnly: boolean
}

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}

/** Parse the daemon's rows; a row without an id or conversation is dropped. */
export function parseImportedThreads(body: unknown): ImportedThread[] {
  const rows = (body as { imports?: unknown } | null)?.imports
  if (!Array.isArray(rows)) return []
  const out: ImportedThread[] = []
  for (const r of rows as Record<string, unknown>[]) {
    const id = str(r.id)
    const localConversationId = str(r.localConversationId)
    if (!id || !localConversationId) continue
    out.push({
      id,
      localConversationId,
      role: str(r.role),
      cloneId: str(r.cloneId),
      sourceHost: str(r.sourceHost),
      sourceHandle: str(r.sourceHandle),
      name: typeof r.name === 'string' && r.name.trim() ? r.name : null,
      docCount: typeof r.docCount === 'number' ? r.docCount : 0,
      importedAt: typeof r.importedAt === 'number' ? r.importedAt : 0,
      addr: typeof r.addr === 'string' && r.addr ? r.addr : null,
      readOnly: r.readOnly === true,
    })
  }
  // Newest first.
  return out.sort((a, b) => b.importedAt - a.importedAt)
}

export async function fetchImportedThreads(scope: ServerScope, projectPath: string): Promise<ImportedThread[]> {
  return parseImportedThreads(await daemonCliGet<unknown>(scope, 'thread/imports', { ws: projectPath }))
}

/** The row's title: its saved name, else where it came from. */
export function importedTitle(r: ImportedThread): string {
  if (r.name) return r.name
  const from = r.sourceHandle || 'Thread'
  return r.role === 'canonical' || r.role === 'pending_canonical' ? `${from} main Thread` : `${from} Thread`
}

/** The row's second line: where it lives here, how many messages, from where. */
export function importedDetail(r: ImportedThread): string {
  const where =
    r.role === 'canonical'
      ? 'in the main Thread'
      : r.role === 'pending_canonical'
        ? 'joins the main Thread when Chat starts'
        : r.role === 'sidecar'
          ? 'in its side Thread'
          : 'read-only history'
  const n = `${r.docCount} message${r.docCount === 1 ? '' : 's'}`
  const host = r.sourceHost ? ` · from ${r.sourceHost}` : ''
  return `${where} · ${n}${host}`
}

export function ImportedThreadsSection({ projectPath }: { projectPath: string | null }): React.JSX.Element | null {
  const room = useRoom()
  const [rows, setRows] = useState<ImportedThread[]>([])
  const [open, setOpen] = useState(true)
  const [viewing, setViewing] = useState<ImportedThread | null>(null)

  const load = useCallback(() => {
    if (!projectPath) {
      setRows([])
      return
    }
    fetchImportedThreads(room.scope, projectPath).then(setRows, (e: unknown) => {
      // An older server has no route: the group just doesn't show.
      console.warn('[chat-history] imported Threads:', e)
      setRows([])
    })
  }, [room.scope, projectPath])

  useEffect(() => {
    load()
    return onChatHistoryChanged(room.scope, load)
  }, [load, room.scope])

  if (rows.length === 0) return null

  return (
    <div data-testid="imported-threads">
      <button
        type="button"
        className="no-drag w-full flex items-center gap-1.5 px-3 py-1.5 border-y border-[var(--color-border)] hover:bg-white/[0.03] transition-colors"
        onClick={() => setOpen((v) => !v)}
      >
        <span className="text-[9px] text-[var(--color-text-muted)] w-2">{open ? '▼' : '▶'}</span>
        <span className="text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] font-mono">
          Imported Threads
        </span>
        <span className="text-[9px] text-[var(--color-text-muted)] ml-auto tabular-nums">{rows.length}</span>
      </button>
      {open && (
        <div className="py-0.5">
          {rows.map((r) => (
            <button
              key={r.id}
              type="button"
              data-testid={`imported-thread-${r.id}`}
              disabled={!r.addr}
              onClick={() => setViewing(r)}
              title={r.addr ? 'Open this imported Thread' : 'This Thread has no address here yet'}
              className="no-drag w-full flex items-center gap-2 px-3 h-8 hover:bg-white/[0.04] active:bg-white/[0.06] text-left disabled:opacity-50 disabled:cursor-default transition-colors"
            >
              <span className="w-1.5 h-1.5 flex-shrink-0 bg-[var(--color-text-muted)]" aria-hidden="true" />
              <div className="flex-1 min-w-0 flex flex-col justify-center">
                <span className="text-[11px] text-[var(--color-text-secondary)] font-mono truncate leading-tight">
                  {importedTitle(r)}
                </span>
                <span className="text-[10px] text-[var(--color-text-muted)] font-mono leading-tight truncate">
                  {importedDetail(r)}
                </span>
              </div>
            </button>
          ))}
        </div>
      )}
      {viewing?.addr && <ImportedThreadViewer row={viewing} addr={viewing.addr} onClose={() => setViewing(null)} />}
    </div>
  )
}

/** The imported Thread, read-only (no message box). */
export function ImportedThreadViewer({
  row,
  addr,
  onClose,
}: {
  row: ImportedThread
  addr: string
  onClose: () => void
}): React.JSX.Element {
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return (
    <>
      <DialogScrim zIndex={50} onClick={onClose} />
      <DialogFrame
        zIndex={51}
        role="dialog"
        aria-label={importedTitle(row)}
        data-testid="imported-thread-viewer"
        className="w-[640px] max-w-[calc(100vw-32px)] h-[70vh] flex flex-col"
      >
        <div className="flex items-center gap-2 px-4 py-2 border-b border-[var(--color-border)]">
          <div className="min-w-0 flex-1">
            <div className="text-xs text-[var(--color-text-primary)] truncate">{importedTitle(row)}</div>
            <div className="text-[10px] text-[var(--color-text-muted)] truncate">
              Imported with a clone{row.sourceHost ? ` from ${row.sourceHost}` : ''}. Imported messages are read-only.
            </div>
          </div>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </div>
        <div className="relative flex-1 min-h-0 flex">
          <ThreadOverlayColumn addr={addr} conversationId={row.localConversationId} active composeBar={null} />
        </div>
      </DialogFrame>
    </>
  )
}
