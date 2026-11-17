// The composer's model controls: `/model` opens the picker, and the
// composer names the chat's model, server default included
// (ComposeModelButton, left of the stash button while the box is empty).
// The switch is the daemon's (`POST /cli/sessions/model`): this chat only, live,
// the CLI's saved default put back; the daemon writes the Thread note.

import { useCallback, useEffect, useRef, useState, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { checkedModelRow, composeModelId, modelPickerRows } from '@/lib/model-choices'
import { fetchSessionModelOptions, switchSessionModel, type SessionModelOptions } from '@/lib/session-model-api'
import { MODEL_PICKER_WORDS, ModelPicker } from './ModelPicker'

export interface ComposeModel {
  /** Open the picker, its search pre-filled (`/model op` → `op`). */
  open: (query: string) => void
  /** The portalled picker, while open. */
  picker: JSX.Element | null
  /** The model the composer names (server default included); null for a
   *  shell, or until K2 knows. */
  model: string | null
  /** Open the picker beside `anchor` (the model control). */
  openFrom: (anchor: HTMLElement) => void
  /** A switch that failed, in words (shown under the composer). */
  error: string | null
  clearError: () => void
}

type LoadState = 'loading' | 'error' | 'ready'

export function useComposeModel(input: {
  scope: ServerScope
  sessionId: string
  thread?: string
  /** What the picker opens beside: the composer. */
  anchor: () => HTMLElement | null
  /** Focus goes back here when the picker closes by key or pick. */
  refocus: () => void
}): ComposeModel {
  const { scope, sessionId, thread } = input
  const [opts, setOpts] = useState<SessionModelOptions | null>(null)
  const [state, setState] = useState<LoadState>('loading')
  const [openAt, setOpenAt] = useState<{ query: string; anchor: HTMLElement | null } | null>(null)
  const [error, setError] = useState<string | null>(null)
  const seq = useRef(0)

  const load = useCallback(() => {
    if (!sessionId) return
    const mine = ++seq.current
    setState('loading')
    fetchSessionModelOptions(scope, sessionId).then(
      (o) => {
        if (mine !== seq.current) return
        setOpts(o)
        setState('ready')
      },
      () => {
        if (mine !== seq.current) return
        setState('error')
      },
    )
  }, [scope, sessionId])

  // The chip needs to know the chat's model before anyone opens the picker.
  useEffect(() => {
    setOpts(null)
    load()
  }, [load])

  const open = useCallback(
    (query: string) => {
      setError(null)
      setOpenAt({ query, anchor: input.anchor() })
      if (state !== 'ready') load()
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [load, state],
  )

  const choose = useCallback(
    async (value: string) => {
      if (!opts) return
      // The default row: the server's default, else the harness's own.
      const model = value || opts.serverDefault || 'default'
      try {
        await switchSessionModel(scope, sessionId, model, thread)
        setOpts((o) => (o ? { ...o, current: model } : o))
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      }
    },
    [opts, scope, sessionId, thread],
  )

  const rowsFor = useCallback(
    (query: string) =>
      opts
        ? modelPickerRows({ harness: opts.harness, mode: 'thread', serverDefault: opts.serverDefault, current: opts.current, query })
        : [],
    [opts],
  )

  const notSwitchable = state === 'ready' && opts !== null && !opts.switchable
  const picker =
    openAt === null ? null : (
      <ModelPicker
        anchor={openAt.anchor}
        rows={rowsFor}
        checked={opts ? checkedModelRow(rowsFor(''), opts.current, opts.serverDefault) : ''}
        state={notSwitchable ? 'error' : state}
        onRetry={load}
        initialQuery={openAt.query}
        footer={notSwitchable ? `This chat runs ${opts?.harness || 'a shell'}: K2 switches Claude chats.` : MODEL_PICKER_WORDS.threadFooter}
        label="Model for this chat"
        testId="compose-model-picker"
        onPick={(v) => void choose(v)}
        onClose={(refocus) => {
          setOpenAt(null)
          if (refocus) input.refocus()
        }}
      />
    )

  const model = opts ? composeModelId(opts) : null
  const openFrom = useCallback((anchor: HTMLElement) => {
    setError(null)
    setOpenAt({ query: '', anchor })
  }, [])

  return { open, picker, model, openFrom, error, clearError: () => setError(null) }
}
