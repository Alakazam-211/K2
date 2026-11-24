// The agent chat's attach, shared (Rosson 2026-11-23: "when you add the UI
// with the + we just keep it identical to what we have elsewhere"). Every
// compose bar that takes files draws these, never a lookalike:
//   - `ComposeAttachButton`: the "+" (`ComposePlusGlyph` on a lightable
//     `ThreadRoundButton`), opening the "+" menu (Attach a file or image,
//     Save to stash);
//   - `ComposeAttachmentChip`: one file waiting to ride the message, a
//     picture for an image (its preview opens on a click), the name and ×,
//     and an upload's progress while it is on its way.
// The agent chat's box (`TerminalComposeBar`) and Home DMs and channels
// (`HomeChatView`) both use them; only where the bytes go differs.

import { useCallback, useEffect, useRef, useState, type CSSProperties, type JSX, type Ref } from 'react'
import { createPortal } from 'react-dom'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import { useDialogCancel } from '@/components/ui/dialog-cancel'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { loadHostImageObjectUrl, revokeObjectUrl } from '@/lib/load-host-binary'
import { ThreadAttachmentChip, ThreadRoundButton } from './ThreadZenKit'

/** "+" as two bars: ink exactly COMPOSE_TOOL.plusInk wide. */
export function ComposePlusGlyph(): JSX.Element {
  const w = COMPOSE_TOOL.plusInk
  return (
    <svg data-compose-glyph="plus" width={w} height={w} viewBox={`0 0 ${w} ${w}`} fill="currentColor" aria-hidden="true" style={{ display: 'block' }}>
      <rect x={w / 2 - 1} y={0} width={2} height={w} />
      <rect x={0} y={w / 2 - 1} width={w} height={2} />
    </svg>
  )
}

export const COMPOSE_TOOL = {
  gap: 10,
  /** "+", "/" and Send are this tall (Send is this square): the one-line
   *  textarea's height, so the field is 26 + 2×6 padding + 2×1 border =
   *  40px tall (Rosson 2026-10-16: 6px less than the 46px it was). */
  height: 26,
  /** Each button's side padding around its ink (the hit area). */
  pad: 5,
  /** Ink widths of the two drawn glyphs. */
  plusInk: 12,
  slashInk: 8,
} as const

/** The compose bar's "+": attach (and, where the box has one, the stash).
 *  The same button in every box that takes files. */
export function ComposeAttachButton({
  menuOpen,
  buttonRef,
  onClick,
  disabled,
  style,
  testId,
  label = 'Attach or save',
  title = 'Attach a file, or save to stash',
}: {
  menuOpen: boolean
  buttonRef?: Ref<HTMLButtonElement>
  onClick: () => void
  disabled?: boolean
  style?: CSSProperties
  testId?: string
  label?: string
  title?: string
}): JSX.Element {
  return (
    <ThreadRoundButton
      label={label}
      title={title}
      lightable
      disabled={disabled}
      menuOpen={menuOpen}
      buttonRef={buttonRef}
      onClick={onClick}
      data={{ 'aria-haspopup': 'menu', 'aria-expanded': menuOpen, 'data-testid': testId, 'data-compose-attach': 'plus' }}
      style={style ?? COMPOSE_TOOL_STYLE}
    >
      <ComposePlusGlyph />
    </ThreadRoundButton>
  )
}

/** The "+" button's box (the Thread bar's tool size). */
export const COMPOSE_TOOL_STYLE: CSSProperties = { width: 'auto', minWidth: 0, height: COMPOSE_TOOL.height, padding: `0 ${COMPOSE_TOOL.pad}px` }

/** One file waiting to ride a message: an image as its picture chip, any
 *  other file as its name chip; `progress` (0..1) while it uploads. */
export function ComposeAttachmentChip({
  name,
  title,
  image,
  path,
  blob,
  onRemove,
  progress,
}: {
  name: string
  title?: string
  image: boolean
  path?: string
  blob?: Blob
  onRemove: () => void
  progress?: number | null
}): JSX.Element {
  if (image && (path || blob)) {
    return <ComposeImageThumb path={path} blob={blob} name={name} variant="chip" onRemove={onRemove} progress={progress} />
  }
  return <ThreadAttachmentChip name={name} title={title ?? path ?? name} onRemove={onRemove} progress={progress} />
}

export function ComposeImageThumb({
  path,
  blob,
  name: givenName,
  onRemove,
  variant = 'thumb',
  progress,
}: {
  /** A file on this room's server (the agent chat's attachments). */
  path?: string
  /** Bytes in hand, on no server yet (a Home chat upload). */
  blob?: Blob
  /** The name shown (the path's last part when absent). */
  name?: string
  onRemove: () => void
  /** `chip`: the Thread's Zen chip (a small round picture, the name, ×). */
  variant?: 'thumb' | 'chip'
  /** An upload on its way (0..1), drawn on the chip. */
  progress?: number | null
}): JSX.Element {
  const scope = useOptionalRoom()?.scope ?? null
  const [url, setUrl] = useState<string | null>(null)
  const [open, setOpen] = useState(false)
  const urlRef = useRef<string | null>(null)
  const lightboxRef = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: open && url !== null, ref: lightboxRef, fullscreen: true, kind: 'modal' })

  useEffect(() => {
    let cancelled = false
    const ac = new AbortController()
    if (urlRef.current) {
      revokeObjectUrl(urlRef.current)
      urlRef.current = null
    }
    setUrl(null)
    if (blob) {
      const local = URL.createObjectURL(blob)
      urlRef.current = local
      setUrl(local)
      return () => {
        cancelled = true
        ac.abort()
        URL.revokeObjectURL(local)
        urlRef.current = null
      }
    }
    if (!path || !scope) return
    void loadHostImageObjectUrl(scope, path, { signal: ac.signal })
      .then((r) => {
        if (cancelled || ac.signal.aborted) {
          revokeObjectUrl(r.url)
          return
        }
        urlRef.current = r.url
        setUrl(r.url)
      })
      .catch(() => {
        if (!cancelled) setUrl(null)
      })
    return () => {
      cancelled = true
      ac.abort()
      if (urlRef.current) {
        revokeObjectUrl(urlRef.current)
        urlRef.current = null
      }
    }
  }, [path, blob])

  // The preview: Esc, the × and a press beside the image close it.
  const closePreview = useCallback(() => setOpen(false), [])
  const previewCancel = useDialogCancel(open && url ? closePreview : undefined)

  const name = givenName ?? (path ? path.split('/').pop()?.split('\\').pop() || path : 'image')
  const preview = open && url ? createPortal(
      <div
        ref={lightboxRef}
        className="fixed inset-0 z-[99999] flex items-center justify-center bg-[var(--color-scrim)] p-6"
        role="dialog"
        aria-modal="true"
        aria-label={name}
      >
        <img
          src={url}
          alt={name}
          className="max-h-full max-w-full object-contain"
          style={{ borderRadius: 0 }}
          {...previewCancel}
        />
        <button
          type="button"
          aria-label="Close (Esc)"
          title="Close (Esc)"
          onClick={closePreview}
          className="absolute top-3 right-3 h-7 w-7 flex items-center justify-center bg-black/70 text-white text-lg leading-none hover:bg-black/90"
          style={{ borderRadius: 0 }}
        >
          ×
        </button>
      </div>,
      document.body,
    ) : null
  if (variant === 'chip') {
    return (
      <>
        <ThreadAttachmentChip
          name={name}
          title={path ?? name}
          onRemove={onRemove}
          progress={progress}
          onOpen={url ? () => setOpen(true) : undefined}
          leading={
            <span
              aria-hidden
              data-compose-chip-thumb={url ? 'image' : 'loading'}
              className="flex-shrink-0 overflow-hidden"
              style={{ width: 20, height: 20, borderRadius: 999, background: 'var(--zen-surface)' }}
            >
              {url ? <img src={url} alt="" className="h-full w-full object-cover" /> : null}
            </span>
          }
        />
        {preview}
      </>
    )
  }
  return (
    <>
    <div className="relative h-20 w-20 flex-shrink-0 overflow-hidden border border-[var(--color-border)] bg-[var(--color-bg)]" style={{ borderRadius: 0 }}>
      {url ? (
        <button
          type="button"
          className="h-full w-full cursor-zoom-in p-0 border-0 bg-transparent"
          aria-label={`View ${name}`}
          onClick={() => setOpen(true)}
        >
          <img src={url} alt={name} className="h-full w-full object-cover" />
        </button>
      ) : (
        <div className="h-full w-full bg-[var(--color-bg-hover)]" title={path} />
      )}
      <button
        type="button"
        aria-label={`Remove ${name}`}
        onClick={(e) => {
          e.stopPropagation()
          onRemove()
        }}
        className="absolute top-0 right-0 h-4 w-4 flex items-center justify-center bg-black/60 text-white text-[10px] leading-none hover:bg-black/80"
      >
        ×
      </button>
    </div>
    {preview}
    </>
  )
}
