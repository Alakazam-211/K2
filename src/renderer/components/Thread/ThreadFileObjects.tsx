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
// Thread opens a file tab at the line; Option/Alt-click opens it in the
// background, and the tooltip says so). Without one (Zen, Home) the chips
// show but don't open.
//
// Images (S3, 0.46.3, daemon feature `thread-images-v1`): an image
// attachment that is `ok` or `changed` draws as the picture itself, a card
// up to 240 px wide (aspect kept, the chip's rounding) with the name and
// size muted under it; several wrap in a grid. The picture is the daemon's
// thumbnail (`lib/thread-image-thumbs`, read through the Thread's own
// server, cached), fetched when the card nears the viewport, and drawn
// only through `<img src>`. While it loads: a skeleton with the name. No
// thumbnail (415, any error): today's chip. `hidden` and `gone` stay chips
// and never fetch. A click opens the file tab where the provider exists,
// else the full-size viewer (`ThreadImageLightbox`). Without the feature
// or without a `ThreadAttachmentSourceProvider` (a ticket, the project
// chat): chips only.
//
// Download (0.46.3, Rosson 2026-10-22): every file tile (chip, list row,
// image card) gets a Download button on its right when the daemon said
// `canDownload` for this viewer (people: always; an App guest only with
// `files:download`) and the message's attachments can be read (a
// `ThreadAttachmentSourceProvider` and the doc id). A click saves the file
// through `GET /cli/thread/attachment?mode=download`
// (`lib/thread-attachment-download`); it never opens the file, and opening
// is unchanged.
//
// Tickets (0.46.6, `ticket-attachments-v1`): the ticket rail provides a
// ticket source (`ticketAttachmentSource`): the same chips, cards and
// Download, read through the ticket door. A file someone else attached
// (`mine` false) saves quarantined (TK22).

import {
  createContext,
  useContext,
  useMemo,
  useRef,
  useState,
  useEffect,
  type CSSProperties,
  type JSX,
  type MouseEvent,
  type ReactNode,
} from 'react'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { isMacPlatform } from '@/lib/desktop-chrome'
import {
  fileOpenAriaHint,
  fileOpenOptionsFor,
  fileOpenTooltip,
  isBackgroundOpenEvent,
  type FileOpenOptions,
} from '@/lib/file-open-modifier'
import { SetiFileIcon } from '@/lib/seti-file-icons/SetiFileIcon'
import { ZEN_SOLID_PROPS } from '@/lib/zen/zen-materials'
import {
  isThreadImageFile,
  loadThreadThumb,
  peekThreadThumb,
  threadThumbKey,
  type ThreadImage,
  type ThreadImageRef,
} from '@/lib/thread-image-thumbs'
import { downloadThreadAttachment, threadFileDownloadable } from '@/lib/thread-attachment-download'
import { chamferCutFor, chamferVars } from './chamfer'
import { useThreadColumnNarrow } from './ThreadColumn'
import {
  ThreadImageLightbox,
  attachmentImageRef,
  useNearViewport,
  useThreadAttachmentSource,
  type ThreadAttachmentSource,
} from './ThreadImageObjects'

/** Three or more files draw as a list card. */
export const FILE_LIST_CARD_AT = 3
/** The list card shows this many rows before "Show all". */
export const FILE_LIST_CARD_ROWS = 5

export interface ThreadFileActions {
  /** Open the file (a file tab at `line`); `opts.background` (Option/Alt-
   *  click) opens it without showing it. */
  open: (file: ThreadFileObject, opts?: FileOpenOptions) => void
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

/** The Download button on a file tile's right (0.46.3). Shown only when
 *  the daemon said this viewer may save the file. */
export function ThreadFileDownloadButton({ file, onDownload }: { file: ThreadFileObject; onDownload: () => void }): JSX.Element {
  return (
    <button
      type="button"
      data-thread-file-download=""
      title={`Download ${file.name ?? 'file'}`}
      aria-label={`Download ${file.name ?? 'file'}`}
      onMouseDown={(e) => e.stopPropagation()}
      onClick={(e) => {
        e.preventDefault()
        e.stopPropagation()
        onDownload()
      }}
      className="inline-flex flex-shrink-0 cursor-pointer items-center justify-center"
      style={{
        marginLeft: 'auto',
        width: 22,
        height: 22,
        padding: 0,
        borderRadius: 999,
        border: 'none',
        background: 'transparent',
        color: 'var(--zen-text-muted)',
        font: 'inherit',
      }}
    >
      <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden focusable="false">
        <path
          d="M8 2.5v7.5M4.75 6.75 8 10l3.25-3.25M3 13.25h10"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.5"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
    </button>
  )
}

const FILE_TOKENS = { '--thread-file-changed': 'var(--zen-needs-you)' } as CSSProperties

/** The click handlers of an openable chip or row: a plain click opens and
 *  shows the file; Option/Alt-click opens it in the background, without the
 *  browser's own Option-click and without moving focus off the composer. */
function openHandlers(open: (opts?: FileOpenOptions) => void): {
  onMouseDown: (e: MouseEvent) => void
  onClick: (e: MouseEvent) => void
} {
  return {
    onMouseDown: (e) => {
      if (e.button === 0 && isBackgroundOpenEvent(e)) e.preventDefault()
    },
    onClick: (e) => {
      if (isBackgroundOpenEvent(e)) e.preventDefault()
      open(fileOpenOptionsFor(e))
    },
  }
}

/** One file as a chip (one or two files on a message). `onDownload`: the
 *  save action, drawn as a button on the chip's right when the daemon said
 *  this viewer may download the file. */
export function ThreadFileChip({ file, onDownload }: { file: ThreadFileObject; onDownload?: () => void }): JSX.Element {
  const actions = useContext(ThreadFileActionsContext)
  const hidden = isHidden(file)
  const gone = file.state === 'gone'
  const canOpen = !!actions && !hidden && !gone
  const where = hidden ? '' : `${file.path}${fileLineLabel(file)}`
  // A narrow column: a long name may use the message's width.
  const narrow = useThreadColumnNarrow()
  const style: CSSProperties = {
    maxWidth: narrow ? '100%' : '17rem',
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
  if (onDownload && threadFileDownloadable(file)) {
    // The pill holds two buttons side by side: open (when this surface
    // opens files) and Download on the right.
    const isMac = isMacPlatform()
    const bare: CSSProperties = {
      display: 'inline-flex',
      alignItems: 'center',
      gap: 8,
      minWidth: 0,
      padding: 0,
      border: 'none',
      background: 'transparent',
      color: 'inherit',
      font: 'inherit',
      textAlign: 'left',
      cursor: canOpen ? 'pointer' : 'default',
    }
    return (
      <span
        {...common}
        data-thread-file-tile=""
        title={canOpen ? undefined : where}
        className="inline-flex items-center gap-2"
        style={{ ...style, cursor: 'default', paddingRight: 4 }}
      >
        {canOpen ? (
          <button
            type="button"
            data-thread-file-open=""
            title={fileOpenTooltip(where, isMac)}
            aria-label={`Open ${file.name}${file.state === 'changed' ? ' (changed since sent)' : ''}. ${fileOpenAriaHint(isMac)}`}
            {...openHandlers((opts) => actions!.open(file, opts))}
            style={bare}
          >
            <FileRowContent file={file} />
          </button>
        ) : (
          <span className="inline-flex min-w-0 items-center gap-2">
            <FileRowContent file={file} />
          </span>
        )}
        <ThreadFileDownloadButton file={file} onDownload={onDownload} />
      </span>
    )
  }
  if (canOpen) {
    const isMac = isMacPlatform()
    return (
      <button
        type="button"
        {...common}
        title={fileOpenTooltip(where, isMac)}
        aria-label={`Open ${file.name}${file.state === 'changed' ? ' (changed since sent)' : ''}. ${fileOpenAriaHint(isMac)}`}
        {...openHandlers((opts) => actions!.open(file, opts))}
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

/** Three or more files: a list card. `onDownload(i)`: save `files[i]`
 *  (a Download button on each downloadable row's right). */
export function ThreadFileListCard({
  files,
  onDownload,
}: {
  files: ThreadFileObject[]
  onDownload?: (index: number) => void
}): JSX.Element {
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
        if (onDownload && threadFileDownloadable(f)) {
          return (
            <div key={key} {...attrs} data-thread-file-tile="" style={{ ...rowStyle, cursor: 'default' }}>
              {canOpen ? (
                <button
                  type="button"
                  data-thread-file-open=""
                  title={fileOpenTooltip(`${f.path}${fileLineLabel(f)}`)}
                  aria-label={`Open ${f.name}${f.state === 'changed' ? ' (changed since sent)' : ''}. ${fileOpenAriaHint()}`}
                  {...openHandlers((opts) => actions!.open(f, opts))}
                  style={{ ...rowStyle, padding: 0, borderTop: 'none', flex: 1 }}
                >
                  <FileRowContent file={f} />
                </button>
              ) : (
                <span className="flex min-w-0 flex-1 items-center" style={{ gap: 8 }}>
                  <FileRowContent file={f} />
                </span>
              )}
              <ThreadFileDownloadButton file={f} onDownload={() => onDownload(i)} />
            </div>
          )
        }
        return canOpen ? (
          <button
            key={key}
            type="button"
            {...attrs}
            title={fileOpenTooltip(`${f.path}${fileLineLabel(f)}`)}
            aria-label={`Open ${f.name}${f.state === 'changed' ? ' (changed since sent)' : ''}. ${fileOpenAriaHint()}`}
            {...openHandlers((opts) => actions!.open(f, opts))}
            style={rowStyle}
          >
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

/** Image card widths: one picture, or each of several in the grid. */
export const THREAD_IMAGE_CARD_PX = 240
export const THREAD_IMAGE_GRID_PX = 160

/** An image attachment that draws as a picture: a name and path to show,
 *  and a live file (`ok` or `changed`). `hidden` / `gone` stay chips. */
export function threadImageDrawable(f: ThreadFileObject): boolean {
  if (isHidden(f)) return false
  const state = f.state ?? 'ok'
  return (state === 'ok' || state === 'changed') && isThreadImageFile(f)
}

/** One image attachment as its picture (S3). Falls back to the chip when
 *  the server has no thumbnail for it. */
export function ThreadImageCard({
  file,
  source,
  docId,
  n,
  width,
  onDownload,
}: {
  file: ThreadFileObject
  source: ThreadAttachmentSource
  docId: string
  /** The attachment's index on the message (the daemon's `n`). */
  n: number
  width: number
  /** Save it (a Download button in the caption's right). */
  onDownload?: () => void
}): JSX.Element {
  const actions = useContext(ThreadFileActionsContext)
  const imageRef = useMemo<ThreadImageRef>(
    () => attachmentImageRef(source, docId, n, file.modifiedAt),
    [source, docId, n, file.modifiedAt],
  )
  const [img, setImg] = useState<ThreadImage | null>(() => peekThreadThumb(imageRef))
  const [failed, setFailed] = useState(false)
  const [viewing, setViewing] = useState(false)
  const boxRef = useRef<HTMLDivElement>(null)
  const near = useNearViewport(boxRef)
  useEffect(() => {
    if (img || failed || !near) return
    let live = true
    loadThreadThumb(imageRef).then(
      (got) => {
        if (live) setImg(got)
      },
      (err: unknown) => {
        if (!live) return
        // 415 (no thumbnail for this type, or no converter on this box) is
        // expected; anything else earns a console line. Either way: the chip.
        if ((err as { status?: unknown })?.status !== 415) console.warn('[thread] image thumbnail failed:', err)
        setFailed(true)
      },
    )
    return () => {
      live = false
    }
  }, [img, failed, near, imageRef])

  if (failed) return <ThreadFileChip file={file} onDownload={onDownload} />

  const changed = file.state === 'changed'
  const name = file.name ?? ''
  const aspect = img?.width && img?.height ? `${img.width} / ${img.height}` : undefined
  const canOpenTab = !!actions
  const isMac = isMacPlatform()
  const where = `${file.path}${fileLineLabel(file)}`
  const open = canOpenTab
    ? openHandlers((opts) => actions!.open(file, opts))
    : { onClick: () => setViewing(true) }
  return (
    <div
      ref={boxRef}
      data-thread-image={file.path}
      data-state={file.state ?? 'ok'}
      data-thread-image-loading={img ? undefined : ''}
      data-chamfer=""
      {...ZEN_SOLID_PROPS}
      style={{
        width,
        maxWidth: '100%',
        minWidth: 0,
        borderRadius: 'calc(var(--zen-radius, 12px) * 0.75)',
        border: '1px solid var(--zen-border)',
        background: 'var(--zen-surface-raised)',
        color: 'var(--zen-text)',
        overflow: 'hidden',
        ...chamferVars({ cut: chamferCutFor(30), fill: 'var(--zen-surface-raised)', edge: 'var(--zen-border)', border: 1 }),
      }}
    >
      <button
        type="button"
        data-thread-image-open=""
        title={canOpenTab ? fileOpenTooltip(where, isMac) : name}
        aria-label={`${canOpenTab ? 'Open' : 'View'} ${name}${changed ? ' (changed since sent)' : ''}${canOpenTab ? `. ${fileOpenAriaHint(isMac)}` : ''}`}
        {...open}
        style={{
          display: 'block',
          width: '100%',
          padding: 0,
          border: 'none',
          background: 'transparent',
          cursor: canOpenTab ? 'pointer' : 'zoom-in',
          font: 'inherit',
          color: 'inherit',
        }}
      >
        {img ? (
          <img
            src={img.url}
            alt={name}
            draggable={false}
            style={{ display: 'block', width: '100%', height: 'auto', aspectRatio: aspect, objectFit: 'contain', background: 'var(--zen-surface)' }}
          />
        ) : (
          <span
            data-thread-image-skeleton=""
            className="flex items-center justify-center"
            style={{
              height: width >= THREAD_IMAGE_CARD_PX ? 140 : 100,
              padding: '0 10px',
              background: 'var(--zen-surface)',
              color: 'var(--zen-text-muted)',
              fontSize: '0.72em',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {name}
          </span>
        )}
      </button>
      <span
        data-thread-image-caption=""
        className="flex items-center"
        style={{ gap: 8, padding: '4px 10px', fontSize: '0.72em', color: 'var(--zen-text-muted)', minWidth: 0 }}
      >
        <span data-thread-file-name="" className="min-w-0 flex-1 truncate">
          {name}
        </span>
        {changed ? (
          <span data-thread-file-meta="" className="flex flex-shrink-0 items-center" style={{ gap: 5, color: 'var(--thread-file-changed)' }}>
            <span
              data-thread-file-changed=""
              aria-hidden
              style={{ width: 7, height: 7, borderRadius: 999, flex: 'none', background: 'var(--thread-file-changed)' }}
            />
            changed since sent
          </span>
        ) : (
          <span data-thread-file-meta="" className="flex-shrink-0">
            {formatFileSize(file.size)}
          </span>
        )}
        {onDownload && threadFileDownloadable(file) && <ThreadFileDownloadButton file={file} onDownload={onDownload} />}
      </span>
      {viewing && (
        <ThreadImageLightbox imageRef={imageRef} name={name} thumbUrl={img?.url ?? null} onClose={() => setViewing(false)} />
      )}
    </div>
  )
}

/** A message's file objects, on the message's side. `docId` (the message's
 *  id) lets image attachments draw as pictures (S3). */
export function ThreadFileObjects({
  files,
  mine,
  docId,
}: {
  files: ThreadFileObject[] | null | undefined
  mine: boolean
  docId?: string
}): JSX.Element | null {
  const source = useThreadAttachmentSource()
  // The index is the attachment's place on the message (the daemon's `n`).
  const all = (files ?? []).map((f, n) => ({ f, n })).filter(({ f }) => f && typeof f === 'object')
  if (all.length === 0) return null
  const imagesOn = !!source && !!docId && source.scope.serverSupports(source.imagesFeature ?? 'thread-images-v1')
  const images = imagesOn ? all.filter(({ f }) => threadImageDrawable(f)) : []
  const restEntries = all.filter((x) => !images.includes(x))
  const rest = restEntries.map(({ f }) => f)
  // Save through the Thread's own server (the daemon's `canDownload` says
  // whether each tile shows the button). Without a source (a ticket, the
  // project chat) there is no attachment door to read: no button.
  // A file someone else attached saves quarantined (TK22).
  const download =
    source && docId
      ? (f: ThreadFileObject, n: number) => {
          void downloadThreadAttachment(attachmentImageRef(source, docId, n), f.name ?? 'file', { fromOthers: !mine })
        }
      : null
  const side = mine ? 'flex-end' : 'flex-start'
  const width = images.length > 1 ? THREAD_IMAGE_GRID_PX : THREAD_IMAGE_CARD_PX
  return (
    <div
      data-thread-files=""
      className="flex flex-wrap"
      style={{ ...FILE_TOKENS, gap: 6, maxWidth: '100%', justifyContent: side, alignSelf: side }}
    >
      {source && docId && images.length > 0 && (
        <div data-thread-images="" className="flex flex-wrap" style={{ gap: 6, flexBasis: '100%', maxWidth: '100%', justifyContent: side }}>
          {images.map(({ f, n }) => (
            <ThreadImageCard
              key={threadThumbKey(attachmentImageRef(source, docId, n, f.modifiedAt))}
              file={f}
              source={source}
              docId={docId}
              n={n}
              width={width}
              onDownload={download ? () => download(f, n) : undefined}
            />
          ))}
        </div>
      )}
      {rest.length >= FILE_LIST_CARD_AT ? (
        <ThreadFileListCard
          files={rest}
          onDownload={download ? (i) => download(restEntries[i].f, restEntries[i].n) : undefined}
        />
      ) : (
        restEntries.map(({ f, n }, i) => (
          <ThreadFileChip
            key={`${i}:${f.path ?? 'hidden'}`}
            file={f}
            onDownload={download ? () => download(f, n) : undefined}
          />
        ))
      )}
    </div>
  )
}
