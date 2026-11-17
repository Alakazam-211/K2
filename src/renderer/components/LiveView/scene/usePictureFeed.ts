// The real picture's state for the view on screen: the docs preview
// source says each step (a poll or the docs socket's push) in the pushes'
// shape, folded here per view (foldPreviewPush), so the S3 states read the
// same as the sample's: drawing the first, updating over an older still,
// failed and its Retry, ready. `epoch` moves when the tab should ask again
// (the listing came in with the boxes' bounds, a still landed or was
// retried). The sample's source has no feed: the snapshot's state stands.

import { useCallback, useEffect, useState } from 'react'
import { DocsPreviewPictureSource } from '@/lib/live-view/scene/docs-preview-source'
import type { PictureSource, PictureStatus } from '@/lib/live-view/scene/picture-source'
import { foldPreviewPush } from '@/lib/live-view/scene/preview-route'

/** Before anything is said: the first still is being asked for. */
export const FIRST: PictureStatus = { shown: null, want: 0, state: 'rendering', renderMs: null, reusedFrom: null, withoutModifiers: false, reason: null, landed: false, recovered: false, retrying: false, again: false }

export function usePictureFeed(source: PictureSource | null, view: string, meshEpoch = 0): { status: PictureStatus | null; epoch: number; retry: () => void } {
  const docs = source instanceof DocsPreviewPictureSource ? source : null
  const [byView, setByView] = useState<Record<string, PictureStatus>>({})
  const [epoch, setEpoch] = useState(0)
  useEffect(() => {
    setByView({})
    if (!docs) return
    return docs.subscribe((e) => {
      if (e.type === 'listing') return setEpoch((n) => n + 1)
      setByView((m) => ({ ...m, [e.container]: foldPreviewPush(m[e.container] ?? FIRST, e, e.container) }))
      if (e.type === 'preview_ready' && e.state === 'ready') setEpoch((n) => n + 1)
    })
  }, [docs])
  // A mesh changed (a live source's derived mesh_hash moved): every view's still is stale; ask again.
  useEffect(() => {
    if (!docs || !meshEpoch) return
    docs.refreshAll()
    setEpoch((n) => n + 1)
  }, [docs, meshEpoch])
  const retry = useCallback(() => {
    if (!docs) return
    docs.retry(view)
    setEpoch((n) => n + 1)
  }, [docs, view])
  return { status: docs ? (byView[view] ?? FIRST) : null, epoch, retry }
}
