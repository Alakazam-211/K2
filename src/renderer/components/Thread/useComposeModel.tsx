// The composer's model controls (0.46.13): `/model` opens the picker, and a
// quiet chip names the chat's model when it isn't the server default. The
// switch is the daemon's (`POST /cli/sessions/model`): this chat only, live,
// the CLI's saved default put back; the daemon writes the Thread note.

import { useCallback, useEffect, useRef, useState, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { checkedModelRow, modelLabel, modelPickerRows, showModelChip } from '@/lib/model-choices'
import { fetchSessionModelOptions, switchSessionModel, type SessionModelOptions } from '@/lib/session-model-api'
import { MODEL_PICKER_WORDS, ModelPicker } from './ModelPicker'

export interface ComposeModel {
  /** Open the picker, its search pre-filled (`/model op` → `op`). */
  open: (query: string) => void
  /** The portalled picker, while open. */
  picker: JSX.Element | null
  /** The chip, when the chat isn't on the server default. */
  chip: JSX.Element | null
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

  const current = opts?.current ?? null
  const chip =
    opts && opts.switchable && showModelChip(current, opts.serverDefault) && current ? (
      <button
        type="button"
        data-testid="compose-model-chip"
        data-k2-focus=""
        className="k2-focus inline-flex items-center gap-1 self-start rounded-[var(--radius-pill)] border border-[var(--color-border)] px-2 py-0.5 type-hint text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]"
        title="Model for this chat (click to change)"
        aria-label={`Model for this chat: ${modelLabel(current)}. Change`}
        onClick={(e) => {
          setError(null)
          setOpenAt({ query: '', anchor: e.currentTarget })
        }}
      >
        {modelLabel(current)}
      </button>
    ) : null

  return { open, picker, chip, error, clearError: () => setError(null) }
}
