// The pieces behind image attachments on Thread messages (S3,
// `thread-images-v1`): where a Thread's attachments are read from, the
// "near the viewport" check that makes a card fetch lazily, and the
// full-size viewer a card opens where there is no file tab to open
// (Zen, Home). The card itself is `ThreadImageCard` in ThreadFileObjects.
//
// Every picture is drawn through `<img src>` with a `blob:` URL the
// daemon's bytes became (`lib/thread-image-thumbs`), never as markup.

import {
  createContext,
  useContext,
  useEffect,
  useRef,
  useState,
  type JSX,
  type ReactNode,
  type RefObject,
} from 'react'
import { createPortal } from 'react-dom'
import type { ServerScope } from '@/kessel/server-scope'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import {
  fetchThreadImage,
  ticketAttachmentReader,
  type AttachmentReader,
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

/** The full-size picture over the window: the whole image
 *  (`mode=preview`), the thumbnail meanwhile (and if the whole image can't
 *  come). Esc or a click anywhere closes it. */
export function ThreadImageLightbox({
  imageRef,
  name,
  thumbUrl,
  onClose,
}: {
  imageRef: ThreadImageRef
  name: string
  thumbUrl: string | null
  onClose: () => void
}): JSX.Element {
  const [full, setFull] = useState<string | null>(null)
  const boxRef = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref: boxRef, fullscreen: true, kind: 'modal' })

  useEffect(() => {
    let live = true
    let url: string | null = null
    fetchThreadImage(imageRef, 'preview').then(
      (img) => {
        url = img.url
        if (live) setFull(img.url)
        else URL.revokeObjectURL(img.url)
      },
      (err: unknown) => {
        // The thumbnail stays up; say why in the console only.
        if (live) console.warn('[thread] full image failed, showing the thumbnail:', err)
      },
    )
    return () => {
      live = false
      if (url) URL.revokeObjectURL(url)
    }
  }, [imageRef])

  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  const src = full ?? thumbUrl
  return createPortal(
    <div
      ref={boxRef}
      data-thread-image-lightbox=""
      data-full={full ? '' : undefined}
      className="fixed inset-0 z-[99999] flex items-center justify-center p-6"
      style={{ background: 'var(--color-scrim, rgba(0,0,0,0.7))', cursor: 'zoom-out' }}
      role="dialog"
      aria-modal="true"
      aria-label={name}
      onClick={onClose}
    >
      {src ? (
        <img src={src} alt={name} className="max-h-full max-w-full object-contain" draggable={false} />
      ) : (
        <span style={{ color: '#fff', fontSize: 13 }}>Loading {name}…</span>
      )}
      <button
        type="button"
        aria-label="Close"
        onClick={(e) => {
          e.stopPropagation()
          onClose()
        }}
        className="absolute top-3 right-3 h-7 w-7 flex items-center justify-center bg-black/70 text-white text-lg leading-none hover:bg-black/90"
        style={{ borderRadius: 999 }}
      >
        ×
      </button>
    </div>,
    document.body,
  )
}
