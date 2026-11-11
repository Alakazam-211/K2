// The pieces behind image attachments on Thread messages (S3,
// `thread-images-v1`): where a Thread's attachments are read from, the
// "near the viewport" check that makes a card fetch lazily, and the image
// viewer a card opens (0.46.11). The card itself is `ThreadImageCard` in
// ThreadFileObjects.
//
// Every picture is drawn through `<img src>` with a `blob:` URL the
// daemon's bytes became (`lib/thread-image-thumbs`), never as markup.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type JSX,
  type KeyboardEvent as ReactKeyboardEvent,
  type ReactNode,
  type RefObject,
  type TouchEvent as ReactTouchEvent,
} from 'react'
import { createPortal } from 'react-dom'
import type { ServerScope } from '@/kessel/server-scope'
import type { ThreadFileObject } from '@/components/SessionView/overlayThread'
import { Button, DialogCloseButton, DialogFrame, DialogScrim, cx } from '@/components/ui'
import { ChevronLeftIcon, ChevronRightIcon, DownloadIcon, PopOutIcon } from '@/components/icons/K2Icons'
import {
  fetchThreadImage,
  loadThreadThumb,
  peekThreadThumb,
  threadThumbKey,
  ticketAttachmentReader,
  type AttachmentReader,
  type ThreadImage,
  type ThreadImageRef,
} from '@/lib/thread-image-thumbs'

/** Where a chat's attachments are read from: the server they live on and
 *  the door. A Thread names its address (`addr`, the Thread door); a
 *  ticket (0.46.6, `ticketAttachmentSource`) brings its own `read` (the
 *  ticket door) and `key`. Set by the desktop Thread pane, Zen's
 *  conversation and the ticket rail; without it (the project chat, a
 *  ticket on a server without the feature) images stay chips. */
export interface ThreadAttachmentSource {
  scope: ServerScope
  /** A Thread's address (the Thread door). */
  addr?: string
  /** Another door (a ticket's); wins over `addr`. */
  read?: AttachmentReader
  /** The cache key of this source (default `thread:<addr>`), so pictures of
   *  two sources never share a cache entry. */
  key?: string
  /** The `/boot-status` key that says pictures can be drawn (default
   *  `thread-images-v1`). */
  imagesFeature?: Parameters<ServerScope['serverSupports']>[0]
}

/** A ticket's files (0.46.6, `ticket-attachments-v1`): the ticket door on
 *  `scope`, keyed by the ticket. Memoize it per (scope, ticket). */
export function ticketAttachmentSource(scope: ServerScope, ticketId: string): ThreadAttachmentSource {
  return {
    scope,
    read: ticketAttachmentReader(scope, ticketId),
    key: `ticket:${ticketId}`,
    imagesFeature: 'ticket-attachments-v1',
  }
}

/** The image ref of one attachment of `source`. */
export function attachmentImageRef(source: ThreadAttachmentSource, docId: string, n: number, modifiedAt?: number): ThreadImageRef {
  return {
    scope: source.scope,
    ...(source.addr !== undefined ? { addr: source.addr } : {}),
    ...(source.read ? { read: source.read } : {}),
    ...(source.key ? { sourceKey: source.key } : {}),
    docId,
    n,
    ...(modifiedAt !== undefined ? { modifiedAt } : {}),
  }
}

const ThreadAttachmentSourceContext = createContext<ThreadAttachmentSource | null>(null)

export function ThreadAttachmentSourceProvider({
  value,
  children,
}: {
  value: ThreadAttachmentSource | null
  children: ReactNode
}): JSX.Element {
  return <ThreadAttachmentSourceContext.Provider value={value}>{children}</ThreadAttachmentSourceContext.Provider>
}

export function useThreadAttachmentSource(): ThreadAttachmentSource | null {
  return useContext(ThreadAttachmentSourceContext)
}

/** True once `ref`'s element is within `margin` of the viewport (and from
 *  then on). Without IntersectionObserver (tests, old engines): at once. */
export function useNearViewport(ref: RefObject<Element | null>, margin = '300px'): boolean {
  const [near, setNear] = useState(() => typeof IntersectionObserver === 'undefined')
  useEffect(() => {
    if (near) return
    const el = ref.current
    if (!el || typeof IntersectionObserver === 'undefined') {
      setNear(true)
      return
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) {
          setNear(true)
          io.disconnect()
        }
      },
      { rootMargin: margin },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [near, ref, margin])
  return near
}


// ── The image viewer (0.46.11) ───────────────────────────────────────────
//
// A click on a picture in a Thread message opens it in a modal on the
// shared `DialogFrame` (the ×, Esc and a press outside close it): the
// picture fitted to a frame of about 85% of the window, its name as the
// caption. A message with several pictures shows "2 of 5" with previous and
// next (buttons, ←/→, a swipe on touch), starting at the one clicked, and
// from three pictures a strip of thumbnails. "Open in tab" closes it and
// opens the picture as a file tab (the surface's own file opener, which
// focuses the tab when it is already open); Download saves it through the
// attachment door. Focus moves in and goes back to the clicked card.
//
// The whole picture (`mode=preview`) is read when it is first shown; its
// thumbnail (the card's, from the cache) shows meanwhile, and stays if the
// whole picture can't come. The viewer lets its preview URLs go on close.

/** One picture in the viewer: its file and where its bytes are read. */
export interface ThreadViewerImage {
  file: ThreadFileObject
  imageRef: ThreadImageRef
  /** Save it: given when the daemon said this viewer may download it. */
  onDownload?: () => void
}

/** The strip of thumbnails shows from this many pictures. */
export const THREAD_VIEWER_STRIP_AT = 3

/** A horizontal swipe at least this long (px) turns the picture. */
const SWIPE_PX = 40

/** "2 of 5" (`at` counts from 0). */
export function threadViewerCount(at: number, total: number): string {
  return `${at + 1} of ${total}`
}

/** The caption: the picture's alt text, which is its file name. */
function viewerName(image: ThreadViewerImage): string {
  return image.file.name || image.file.path?.split('/').pop() || 'Image'
}

/** The cached thumbnail of `ref` (the card already asked for it), or the
 *  one fetch shared with the card. Key the caller by `threadThumbKey`. */
function useViewerThumb(ref: ThreadImageRef): ThreadImage | null {
  const [img, setImg] = useState<ThreadImage | null>(() => peekThreadThumb(ref))
  useEffect(() => {
    if (img) return
    let live = true
    loadThreadThumb(ref).then(
      (got) => {
        if (live) setImg(got)
      },
      () => undefined,
    )
    return () => {
      live = false
    }
  }, [img, ref])
  return img
}

const FOCUSABLE = 'button:not([disabled]), [href], input, select, textarea, [tabindex]:not([tabindex="-1"])'

function isTypingTarget(t: EventTarget | null): boolean {
  return t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))
}

/** The picture shown: the whole image once it came, else the thumbnail. */
function ViewerPicture({ image, full, failed }: { image: ThreadViewerImage; full: string | null; failed: boolean }): JSX.Element {
  const thumb = useViewerThumb(image.imageRef)
  const name = viewerName(image)
  const src = full ?? thumb?.url ?? null
  if (!src) {
    return (
      <span data-thread-image-viewer-note="" className="m-auto px-4 text-center text-xs text-[var(--color-text-muted)]">
        {failed ? `Couldn’t load ${name}` : `Loading ${name}…`}
      </span>
    )
  }
  return (
    <img
      data-thread-image-viewer-picture=""
      data-full={full ? '' : undefined}
      src={src}
      alt={name}
      draggable={false}
      // Fitted to the stage, never past its own size (a small picture
      // stays crisp); centred by the auto margins.
      className="absolute inset-0 m-auto block max-h-full max-w-full object-contain"
    />
  )
}

/** One thumbnail in the strip. */
function ViewerThumb({
  image,
  index,
  total,
  current,
  onPick,
}: {
  image: ThreadViewerImage
  index: number
  total: number
  current: boolean
  onPick: () => void
}): JSX.Element {
  const thumb = useViewerThumb(image.imageRef)
  const name = viewerName(image)
  return (
    <button
      type="button"
      data-thread-image-viewer-thumb={index}
      data-k2-chamfer="control"
      aria-label={`Image ${threadViewerCount(index, total)}: ${name}`}
      aria-current={current ? 'true' : undefined}
      title={name}
      onClick={onPick}
      className={cx(
        'k2-focus relative h-12 w-12 flex-shrink-0 cursor-pointer overflow-hidden p-0 transition-opacity',
        // A rounded square (a pill field radius would make it a circle).
        'rounded-[var(--radius-field-multiline)] border-2 bg-[var(--color-bg-inset)] [--k2-ch-bw:2px]',
        current
          ? 'border-[var(--color-accent)] opacity-100'
          : 'border-[var(--color-control-edge)] opacity-60 hover:opacity-100',
      )}
    >
      {thumb ? (
        <img src={thumb.url} alt="" draggable={false} className="block h-full w-full object-cover" />
      ) : (
        <span aria-hidden className="block h-full w-full" />
      )}
    </button>
  )
}

/** The viewer over the window. `start`: the picture clicked. `onClose`:
 *  `cancel` (the ×, Esc, a press outside: focus goes back to the card) or
 *  `tab` ("Open in tab": the tab takes focus). `onOpenInTab`: the surface's
 *  file opener; without one (no workspace to open it in) there is no
 *  "Open in tab". */
export function ThreadImageViewer({
  images,
  start,
  onClose,
  onOpenInTab,
}: {
  images: ThreadViewerImage[]
  start: number
  onClose: (why: 'cancel' | 'tab') => void
  onOpenInTab?: (file: ThreadFileObject) => void
}): JSX.Element | null {
  const total = images.length
  const clamp = useCallback((i: number) => Math.min(Math.max(i, 0), Math.max(total - 1, 0)), [total])
  const [at, setAt] = useState(() => clamp(start))
  const current = images[clamp(at)]
  const boxRef = useRef<HTMLDivElement>(null)
  const prevRef = useRef<HTMLButtonElement>(null)
  const nextRef = useRef<HTMLButtonElement>(null)
  const refocus = useRef<HTMLElement | null>(null)
  const titleId = useRef(`thread-image-viewer-${Math.random().toString(36).slice(2)}`).current

  // The whole pictures read so far (key → URL, null: it failed).
  const previews = useRef(new Map<string, string | null>())
  const asked = useRef(new Set<string>())
  const alive = useRef(true)
  const [, setVersion] = useState(0)
  useEffect(() => {
    alive.current = true
    const held = previews.current
    const sent = asked.current
    return () => {
      alive.current = false
      for (const url of held.values()) if (url) URL.revokeObjectURL(url)
      held.clear()
      sent.clear()
    }
  }, [])
  const currentRef = current?.imageRef
  useEffect(() => {
    if (!currentRef) return
    const key = threadThumbKey(currentRef)
    if (asked.current.has(key)) return
    asked.current.add(key)
    fetchThreadImage(currentRef, 'preview').then(
      (img) => {
        if (!alive.current || previews.current.get(key)) {
          URL.revokeObjectURL(img.url)
          return
        }
        previews.current.set(key, img.url)
        setVersion((v) => v + 1)
      },
      (err: unknown) => {
        if (!alive.current) return
        // The thumbnail stays up; say why in the console only.
        console.warn('[thread] full image failed, showing the thumbnail:', err)
        previews.current.set(key, null)
        setVersion((v) => v + 1)
      },
    )
  }, [currentRef])

  const go = useCallback(
    (step: number) => {
      setAt((i) => {
        const next = clamp(i + step)
        // The arrow that just reached the end turns off: keep focus on the
        // other one rather than dropping it on the page.
        const active = document.activeElement
        if (next === 0 && active === prevRef.current) refocus.current = nextRef.current
        if (next === total - 1 && active === nextRef.current) refocus.current = prevRef.current
        return next
      })
    },
    [clamp, total],
  )
  useEffect(() => {
    const el = refocus.current
    refocus.current = null
    el?.focus()
  }, [at])

  // Focus moves in (the dialog itself, so its name is read first).
  useLayoutEffect(() => {
    boxRef.current?.focus()
  }, [])

  // ←/→ wherever focus is (the viewer is on top while it is open).
  useEffect(() => {
    if (total < 2) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.defaultPrevented || e.altKey || e.ctrlKey || e.metaKey || e.shiftKey || e.isComposing) return
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return
      if (isTypingTarget(e.target)) return
      e.preventDefault()
      go(e.key === 'ArrowLeft' ? -1 : 1)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [go, total])

  // Tab stays inside the viewer.
  const trapTab = (e: ReactKeyboardEvent<HTMLDivElement>): void => {
    if (e.key !== 'Tab') return
    const box = boxRef.current
    if (!box) return
    const items = Array.from(box.querySelectorAll<HTMLElement>(FOCUSABLE))
    if (items.length === 0) return
    const first = items[0]
    const last = items[items.length - 1]
    const active = document.activeElement
    if (e.shiftKey && (active === first || active === box)) {
      e.preventDefault()
      last.focus()
    } else if (!e.shiftKey && active === last) {
      e.preventDefault()
      first.focus()
    }
  }

  // A swipe on touch.
  const touch = useRef<{ x: number; y: number } | null>(null)
  const onTouchStart = (e: ReactTouchEvent): void => {
    const t = e.touches[0]
    touch.current = t ? { x: t.clientX, y: t.clientY } : null
  }
  const onTouchEnd = (e: ReactTouchEvent): void => {
    const s = touch.current
    touch.current = null
    const t = e.changedTouches[0]
    if (!s || !t || total < 2) return
    const dx = t.clientX - s.x
    const dy = t.clientY - s.y
    if (Math.abs(dx) >= SWIPE_PX && Math.abs(dx) > Math.abs(dy)) go(dx < 0 ? 1 : -1)
  }

  if (!current) return null
  const name = viewerName(current)
  const key = threadThumbKey(current.imageRef)
  const full = previews.current.get(key) ?? null
  const failed = previews.current.has(key) && previews.current.get(key) === null
  const several = total > 1
  const cancel = (): void => onClose('cancel')

  // React events bubble out of a portal to the message it sits in: a click
  // in the viewer is not a click on the bubble. (Presses and keys pass:
  // the dialog's own outside-press and Esc listen for them on the window.)
  const stop = (e: { stopPropagation: () => void }): void => e.stopPropagation()
  return createPortal(
    <div data-thread-image-viewer-root="" style={{ display: 'contents' }} onClick={stop} onDoubleClick={stop} onContextMenu={stop}>
      <DialogScrim />
      <DialogFrame
        data-thread-image-viewer=""
        onCancel={cancel}
        closeButton="inline"
        className="flex flex-col overflow-hidden"
        style={{ width: 'min(86vw, 1440px)', height: 'min(86vh, 1000px)' }}
      >
        <div
          ref={boxRef}
          role="dialog"
          aria-modal="true"
          aria-labelledby={titleId}
          tabIndex={-1}
          onKeyDown={trapTab}
          className="flex min-h-0 flex-1 flex-col outline-none"
        >
          <div className="flex items-center gap-2 border-b border-[var(--color-border)] px-4 py-2">
            {several && (
              <span
                data-thread-image-viewer-count=""
                aria-live="polite"
                aria-atomic="true"
                className="flex-shrink-0 text-xs tabular-nums text-[var(--color-text-muted)]"
              >
                {threadViewerCount(at, total)}
              </span>
            )}
            <span
              id={titleId}
              data-thread-image-viewer-caption=""
              title={current.file.path ?? name}
              className="min-w-0 flex-1 truncate text-[13px] font-medium text-[var(--color-text-primary)]"
            >
              {name}
            </span>
            {current.onDownload && (
              <Button
                variant="quiet"
                data-thread-image-viewer-download=""
                aria-label={`Download ${name}`}
                title={`Download ${name}`}
                onClick={current.onDownload}
                className="flex flex-shrink-0 items-center gap-1.5"
              >
                <DownloadIcon size={14} />
                Download
              </Button>
            )}
            {onOpenInTab && (
              <Button
                data-thread-image-viewer-open-tab=""
                aria-label={`Open ${name} in a tab`}
                title={`Open ${name} in a tab`}
                onClick={() => {
                  onClose('tab')
                  onOpenInTab(current.file)
                }}
                className="flex flex-shrink-0 items-center gap-1.5"
              >
                <PopOutIcon size={14} />
                Open in tab
              </Button>
            )}
            <DialogCloseButton className="-mr-2" onClick={cancel} />
          </div>
          <div
            data-thread-image-viewer-stage=""
            className="flex min-h-0 flex-1 items-center gap-2 px-2 py-3"
            onTouchStart={onTouchStart}
            onTouchEnd={onTouchEnd}
          >
            {several && (
              <Button
                ref={prevRef}
                variant="quiet"
                data-thread-image-viewer-prev=""
                aria-label="Previous image"
                title="Previous image (←)"
                disabled={at === 0}
                onClick={() => go(-1)}
                className="flex h-9 w-9 flex-shrink-0 items-center justify-center !p-0"
              >
                <ChevronLeftIcon size={18} />
              </Button>
            )}
            <div className="relative h-full min-h-0 min-w-0 flex-1">
              <ViewerPicture key={key} image={current} full={full} failed={failed} />
            </div>
            {several && (
              <Button
                ref={nextRef}
                variant="quiet"
                data-thread-image-viewer-next=""
                aria-label="Next image"
                title="Next image (→)"
                disabled={at === total - 1}
                onClick={() => go(1)}
                className="flex h-9 w-9 flex-shrink-0 items-center justify-center !p-0"
              >
                <ChevronRightIcon size={18} />
              </Button>
            )}
          </div>
          {total >= THREAD_VIEWER_STRIP_AT && (
            <div
              data-thread-image-viewer-strip=""
              role="group"
              aria-label="All images"
              className="flex-shrink-0 overflow-x-auto border-t border-[var(--color-border)] px-4 py-2.5"
            >
              {/* Centred while it fits; scrolls from its start when it doesn't. */}
              <div className="mx-auto flex w-max gap-2">
                {images.map((img, i) => (
                  <ViewerThumb
                    key={threadThumbKey(img.imageRef)}
                    image={img}
                    index={i}
                    total={total}
                    current={i === at}
                    onPick={() => setAt(i)}
                  />
                ))}
              </div>
            </div>
          )}
        </div>
      </DialogFrame>
    </div>,
    document.body,
  )
}
