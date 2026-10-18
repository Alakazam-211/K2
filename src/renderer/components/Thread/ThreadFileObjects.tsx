// File objects on Thread messages (0.46.1, daemon feature
// `thread-attachments-v1`; eval-thread-file-objects-v1 §3.3). An agent
// attaches workspace files with `k2 thread <addr> "…" --file path[:line]`;
// each rides the message as a pointer the daemon checked at send time and
// re-checks on every read (`state`). Under the bubble:
//
//   - one or two files: chips in the compose chip's pill (no ×), so sent
//     and composing chips match;
//   - three or more: a list card ("3 files"), expanded up to five, with a
//     "Show all" for the rest.
//
// States: `changed` keeps an amber dot and "changed since sent" and still
// opens; `gone` strikes the name, says "Not found (moved or deleted)" and
// offers Find in Files; `hidden` (an App guest whose own file access
// doesn't cover it) is a grey "A file (not shared with you)" with no name,
// never clickable.
//
// What a click does comes from `ThreadFileActionsProvider` (the desktop
// Thread opens a file tab at the line). Without one (Zen, Home) the chips
// show but don't open.

import { createContext, useContext, useState, type CSSProperties, type JSX, type ReactNode } from 'react'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import { ZEN_SOLID_PROPS } from '@/lib/zen/zen-materials'
import { chamferCutFor, chamferVars } from './chamfer'

/** Three or more files draw as a list card. */
export const FILE_LIST_CARD_AT = 3
/** The list card shows this many rows before "Show all". */
export const FILE_LIST_CARD_ROWS = 5

export interface ThreadFileActions {
  /** Open the file (a file tab at `line`). */
  open: (file: ThreadFileObject) => void
  /** Search the Files drawer for a file that moved. */
  find?: (file: ThreadFileObject) => void
}

const ThreadFileActionsContext = createContext<ThreadFileActions | null>(null)

export function ThreadFileActionsProvider({ value, children }: { value: ThreadFileActions; children: ReactNode }): JSX.Element {
  return <ThreadFileActionsContext.Provider value={value}>{children}</ThreadFileActionsContext.Provider>
}

/** `245812` → `240 KB`. */
export function formatFileSize(n: number | undefined): string {
  const bytes = typeof n === 'number' && Number.isFinite(n) && n > 0 ? n : 0
  if (bytes < 1024) return `${bytes} B`
  const units = ['KB', 'MB', 'GB']
  let v = bytes / 1024
  let i = 0
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024
    i++
  }
  return `${v >= 10 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`
}

/** `:12` or `:12–40` (empty without a line). */
export function fileLineLabel(f: Pick<ThreadFileObject, 'line' | 'lineEnd'>): string {
  if (!f.line) return ''
  return f.lineEnd && f.lineEnd !== f.line ? `:${f.line}–${f.lineEnd}` : `:${f.line}`
}

/** The muted line under a name: size · type · lines, or the state's words. */
export function fileMetaLine(f: ThreadFileObject): string {
  if (f.state === 'gone') return 'Not found (moved or deleted)'
  if (f.state === 'changed') return `changed since sent${fileLineLabel(f) ? ` · ${fileLineLabel(f)}` : ''}`
  const ext = (f.name ?? '').includes('.') ? (f.name ?? '').split('.').pop()!.toUpperCase() : ''
  return [formatFileSize(f.size), ext, fileLineLabel(f)].filter(Boolean).join(' · ')
}

const isHidden = (f: ThreadFileObject): boolean => f.state === 'hidden' || !f.path || !f.name

function FileRowContent({ file }: { file: ThreadFileObject }): JSX.Element {
  if (isHidden(file)) {
    return (
      <>
        <SetiFileIcon size={16} />
        <span className="min-w-0 truncate" style={{ fontSize: '0.85em' }}>
          A file (not shared with you)
        </span>
      </>
    )
  }
  const gone = file.state === 'gone'
  const changed = file.state === 'changed'
  return (
    <>
      <SetiFileIcon name={file.name} size={16} />
      <span className="flex min-w-0 flex-col" style={{ lineHeight: 1.2 }}>
        <span
          data-thread-file-name=""
          className="truncate"
          style={{
            fontSize: '0.85em',
            fontWeight: 500,
            textDecoration: gone ? 'line-through' : undefined,
            color: gone ? 'var(--zen-text-muted)' : undefined,
          }}
        >
          {file.name}
        </span>
        <span
          data-thread-file-meta=""
          className="truncate"
          style={{ fontSize: '0.72em', color: changed ? 'var(--thread-file-changed)' : 'var(--zen-text-muted)' }}
        >
          {fileMetaLine(file)}
        </span>
      </span>
      {changed && (
        <span
          data-thread-file-changed=""
          aria-hidden
          style={{ width: 7, height: 7, borderRadius: 999, flex: 'none', background: 'var(--thread-file-changed)' }}
        />
      )}
    </>
  )
}

function FindButton({ file, onFind }: { file: ThreadFileObject; onFind: (f: ThreadFileObject) => void }): JSX.Element {
  return (
    <button
      type="button"
      data-thread-file-find=""
      onClick={(e) => {
        e.stopPropagation()
        onFind(file)
      }}
      className="flex-shrink-0 cursor-pointer"
      style={{ fontSize: '0.72em', color: 'var(--zen-accent)', background: 'transparent', border: 'none', padding: 0, font: 'inherit' }}
    >
      Find in Files
    </button>
  )
}

const FILE_TOKENS = { '--thread-file-changed': 'var(--zen-needs-you)' } as CSSProperties

/** One file as a chip (one or two files on a message). */
export function ThreadFileChip({ file }: { file: ThreadFileObject }): JSX.Element {
  const actions = useContext(ThreadFileActionsContext)
  const hidden = isHidden(file)
  const gone = file.state === 'gone'
  const canOpen = !!actions && !hidden && !gone
  const where = hidden ? '' : `${file.path}${fileLineLabel(file)}`
  const style: CSSProperties = {
    maxWidth: '17rem',
    minWidth: 0,
    padding: '4px 10px 4px 7px',
    borderRadius: 999,
    background: 'var(--zen-surface-raised)',
    border: `1px ${hidden || gone ? 'dashed' : 'solid'} var(--zen-border)`,
    color: hidden ? 'var(--zen-text-muted)' : 'var(--zen-text)',
    cursor: canOpen ? 'pointer' : 'default',
    textAlign: 'left',
    font: 'inherit',
    ...chamferVars({ cut: chamferCutFor(30), fill: 'var(--zen-surface-raised)', edge: 'var(--zen-border)', border: 1 }),
  }
  const common = {
    'data-thread-file': hidden ? 'hidden' : file.path,
    'data-state': hidden ? 'hidden' : file.state ?? 'ok',
    'data-chamfer': '',
    ...ZEN_SOLID_PROPS,
  }
  if (canOpen) {
    return (
      <button
        type="button"
        {...common}
        title={`Open ${where}`}
        aria-label={`Open ${file.name}${file.state === 'changed' ? ' (changed since sent)' : ''}`}
        onClick={() => actions!.open(file)}
        className="inline-flex items-center gap-2"
        style={style}
      >
        <FileRowContent file={file} />
      </button>
    )
  }
  return (
    <span {...common} title={hidden ? undefined : where} className="inline-flex items-center gap-2" style={style}>
      <FileRowContent file={file} />
      {gone && actions?.find && <FindButton file={file} onFind={actions.find} />}
    </span>
  )
}

/** Three or more files: a list card. */
export function ThreadFileListCard({ files }: { files: ThreadFileObject[] }): JSX.Element {
  const actions = useContext(ThreadFileActionsContext)
  const [all, setAll] = useState(false)
  const shown = all ? files : files.slice(0, FILE_LIST_CARD_ROWS)
  return (
    <div
      data-thread-file-list=""
      {...ZEN_SOLID_PROPS}
      data-chamfer=""
      style={{
        width: '21rem',
        maxWidth: '100%',
        borderRadius: 'calc(var(--zen-radius) * 0.75)',
        border: '1px solid var(--zen-border)',
        background: 'var(--zen-surface-raised)',
        color: 'var(--zen-text)',
        ...chamferVars({ cut: chamferCutFor(40), fill: 'var(--zen-surface-raised)', edge: 'var(--zen-border)', border: 1 }),
      }}
    >
      <div style={{ padding: '5px 10px', fontSize: '0.72em', color: 'var(--zen-text-muted)', borderBottom: '1px solid var(--zen-border)' }}>
        {files.length} files
      </div>
      {shown.map((f, i) => {
        const hidden = isHidden(f)
        const gone = f.state === 'gone'
        const canOpen = !!actions && !hidden && !gone
        const rowStyle: CSSProperties = {
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          width: '100%',
          minWidth: 0,
          padding: '5px 10px',
          background: 'transparent',
          border: 'none',
          borderTop: i === 0 ? 'none' : '1px solid var(--zen-border)',
          color: hidden ? 'var(--zen-text-muted)' : 'inherit',
          cursor: canOpen ? 'pointer' : 'default',
          textAlign: 'left',
          font: 'inherit',
        }
        const key = `${i}:${f.path ?? 'hidden'}`
        const attrs = { 'data-thread-file': hidden ? 'hidden' : f.path, 'data-state': hidden ? 'hidden' : f.state ?? 'ok' }
        return canOpen ? (
          <button key={key} type="button" {...attrs} title={`Open ${f.path}${fileLineLabel(f)}`} onClick={() => actions!.open(f)} style={rowStyle}>
            <FileRowContent file={f} />
          </button>
        ) : (
          <div key={key} {...attrs} style={rowStyle}>
            <FileRowContent file={f} />
            {gone && actions?.find && <FindButton file={f} onFind={actions.find} />}
          </div>
        )
      })}
      {files.length > FILE_LIST_CARD_ROWS && (
        <button
          type="button"
          data-thread-file-more=""
          onClick={() => setAll((v) => !v)}
          className="cursor-pointer"
          style={{
            width: '100%',
            padding: '5px 10px',
            fontSize: '0.72em',
            color: 'var(--zen-accent)',
            background: 'transparent',
            border: 'none',
            borderTop: '1px solid var(--zen-border)',
            textAlign: 'left',
            font: 'inherit',
          }}
        >
          {all ? 'Show fewer' : `Show all ${files.length}`}
        </button>
      )}
    </div>
  )
}

/** A message's file objects, on the message's side. */
export function ThreadFileObjects({ files, mine }: { files: ThreadFileObject[] | null | undefined; mine: boolean }): JSX.Element | null {
  const list = (files ?? []).filter((f) => f && typeof f === 'object')
  if (list.length === 0) return null
  return (
    <div
      data-thread-files=""
      className="flex flex-wrap"
      style={{ ...FILE_TOKENS, gap: 6, maxWidth: '100%', justifyContent: mine ? 'flex-end' : 'flex-start', alignSelf: mine ? 'flex-end' : 'flex-start' }}
    >
      {list.length >= FILE_LIST_CARD_AT ? (
        <ThreadFileListCard files={list} />
      ) : (
        list.map((f, i) => <ThreadFileChip key={`${i}:${f.path ?? 'hidden'}`} file={f} />)
      )}
    </div>
  )
}
