// Where the scene tab's stills come from. The real ones (the docs preview
// route: DocsPreviewPictureSource) only when the daemon says it draws this
// file (its listing answers as a scene); otherwise, and outside a file tab
// or a room, the sample's stand-in (loaded on demand, sample only) drawing
// the sample's doc at each rev it has seen. One source per tab, closed when
// the tab goes.

import { useEffect, useRef, useState } from 'react'
import { useOptionalRoom } from '@/components/Room/RoomContext'
import { daemonCliGet } from '@/lib/daemon-cli'
import { DocsPreviewPictureSource, type PreviewDecoders, type PreviewTransport } from '@/lib/live-view/scene/docs-preview-source'
import type { PictureSource } from '@/lib/live-view/scene/picture-source'
import type { SimState } from '@/lib/live-view/scene/sim'
import { useLiveFile } from '../live-file'

/** The browser's decoders: a still as a bitmap; an id PNG read back exactly (no colour conversion, no premultiplied alpha). */
export const BROWSER_DECODERS: PreviewDecoders = {
  image: (bytes, mime) => createImageBitmap(new Blob([bytes as BlobPart], { type: mime })),
  pixels: async (bytes, mime) => {
    const bmp = await createImageBitmap(new Blob([bytes as BlobPart], { type: mime }), { colorSpaceConversion: 'none', premultiplyAlpha: 'none' })
    try {
      const cv = typeof OffscreenCanvas !== 'undefined' ? new OffscreenCanvas(bmp.width, bmp.height) : Object.assign(document.createElement('canvas'), { width: bmp.width, height: bmp.height })
      const cx = cv.getContext('2d', { willReadFrequently: true }) as CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D | null
      if (!cx) throw new Error('no 2d context')
      cx.drawImage(bmp, 0, 0)
      return { width: bmp.width, height: bmp.height, data: cx.getImageData(0, 0, bmp.width, bmp.height).data }
    } finally {
      bmp.close()
    }
  },
}

export function useScenePictures(S: SimState | null): PictureSource | null {
  const latest = useRef(S)
  latest.current = S
  const file = useLiveFile()
  const room = useOptionalRoom()
  const scope = room?.scope ?? null
  const path = file?.path ?? null
  const [source, setSource] = useState<PictureSource | null>(null)
  useEffect(() => {
    let made: PictureSource | null = null
    let live = true
    const ac = new AbortController()
    const sample = (): void => {
      void import('@/lib/live-view/scene/stand-in/stand-in-stills').then(({ StandInStills }) => {
        if (!live) return
        made = new StandInStills((rev) => {
          const cur = latest.current
          return cur ? (cur.revDocs[rev] ?? cur.D ?? null) : null
        })
        setSource(made)
      })
    }
    if (!scope || !path) sample()
    else {
      const t: PreviewTransport = { get: (params, signal) => daemonCliGet<unknown>(scope, 'docs/preview', { path, ...params }, signal ? { signal } : undefined) }
      void DocsPreviewPictureSource.serves(t, ac.signal).then((served) => {
        if (!live) return
        if (!served) return sample()
        made = new DocsPreviewPictureSource(t, BROWSER_DECODERS)
        setSource(made)
      })
    }
    return () => {
      live = false
      ac.abort()
      made?.close()
    }
  }, [scope, path])
  return source
}
