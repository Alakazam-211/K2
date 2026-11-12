// Folds a LiveWorkbookSource into the view's state: what it has sent so far,
// then each event as it comes. The client's own gestures go through the same
// reducer (`dispatch`).

import { useEffect, useReducer } from 'react'
import { initialLiveState, liveWorkbookReducer, type LiveWorkbookAction, type LiveWorkbookState } from '@/lib/live-view/reducer'
import type { LiveWorkbookSource } from '@/lib/live-view/source'

export function useLiveWorkbook(source: LiveWorkbookSource): [LiveWorkbookState, (a: LiveWorkbookAction) => void] {
  const [state, dispatch] = useReducer(liveWorkbookReducer, source, (src) => src.snapshot().reduce(liveWorkbookReducer, initialLiveState()))
  useEffect(() => {
    // Catch up on anything sent between the first render and this subscription.
    dispatch({ type: 'reset' })
    for (const e of source.snapshot()) dispatch(e)
    return source.subscribe((e) => dispatch(e))
  }, [source])
  return [state, dispatch]
}
