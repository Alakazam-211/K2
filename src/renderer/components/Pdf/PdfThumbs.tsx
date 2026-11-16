// The Pages tab: a thumbnail per page, drawn with this tab's layers, the
// page on screen marked. A thumbnail draws when it scrolls into view (all at
// once where there's no IntersectionObserver), one page at a time.

import { useEffect, useRef } from 'react'
import { cx } from '@/components/ui'
import type { OcConfig } from '@/lib/pdf/layers'
import type { PdfDoc } from './usePdfDocument'

const THUMB_W = 120

function Thumb({ doc, n, cfg, cfgKey, current, onGo, queue }: { doc: PdfDoc; n: number; cfg: OcConfig | null; cfgKey: number; current: boolean; onGo: (n: number) => void; queue: (job: () => Promise<void>) => void }): React.JSX.Element {
  const canvas = useRef<HTMLCanvasElement>(null)
  const box = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    let cancelled = false
    const draw = (): void =>
      queue(async () => {
        if (cancelled || !canvas.current) return
        const page = await doc.getPage(n)
        const base = page.getViewport({ scale: 1 })
        const dpr = window.devicePixelRatio || 1
        const scale = THUMB_W / base.width
        const vp = page.getViewport({ scale: scale * dpr })
        const c = canvas.current
        if (cancelled || !c) return
        c.width = Math.floor(vp.width)
        c.height = Math.floor(vp.height)
        c.style.width = `${THUMB_W}px`
        c.style.height = `${Math.round(base.height * scale)}px`
        try {
          await page.render({ canvas: c, viewport: vp, ...(cfg ? { optionalContentConfigPromise: Promise.resolve(cfg as never) } : {}) }).promise
        } catch (err) {
          if ((err as Error)?.name !== 'RenderingCancelledException') console.error('[pdf-viewer] Thumbnail error:', err)
        }
      })
    if (typeof IntersectionObserver === 'undefined' || !box.current) {
      draw()
      return () => void (cancelled = true)
    }
    const io = new IntersectionObserver((es) => {
      if (!es.some((e) => e.isIntersecting)) return
      io.disconnect()
      draw()
    })
    io.observe(box.current)
    return () => {
      cancelled = true
      io.disconnect()
    }
  }, [doc, n, cfg, cfgKey, queue])
  return (
    <li>
      <button ref={box} type="button" data-k2-focus="" aria-label={`Page ${n}`} aria-current={current ? 'page' : undefined} className={cx('pdf-thumb no-drag', current ? 'pdf-thumb-on' : '')} onClick={() => onGo(n)}>
        <canvas ref={canvas} data-pdf-thumb="1" className="pdf-thumb-canvas" />
        <span className="type-hint lv-num">{n}</span>
      </button>
    </li>
  )
}

export function PdfThumbs({ doc, cfg, cfgKey, page, onGo }: { doc: PdfDoc; cfg: OcConfig | null; cfgKey: number; page: number; onGo: (n: number) => void }): React.JSX.Element {
  // One draw at a time: a long drawing set doesn't start a hundred renders at once.
  const chain = useRef<Promise<void>>(Promise.resolve())
  const queue = useRef((job: () => Promise<void>) => {
    chain.current = chain.current.then(job, job)
  }).current
  return (
    <ul data-testid="pdf-thumbs" className="pdf-thumbs m-0 list-none p-0">
      {Array.from({ length: doc.numPages }, (_, i) => (
        <Thumb key={i + 1} doc={doc} n={i + 1} cfg={cfg} cfgKey={cfgKey} current={page === i + 1} onGo={onGo} queue={queue} />
      ))}
    </ul>
  )
}
