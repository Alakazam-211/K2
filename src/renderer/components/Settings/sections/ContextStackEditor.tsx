import React, { useCallback, useEffect, useRef, useState } from 'react'
import {
  addContextLayer,
  contextErrorMessage,
  DEFAULT_PRESET_CHIPS,
  FALLBACK_PINNED,
  fetchContextPresets,
  fetchContextStack,
  layerDisplayLabel,
  moveContextLayer,
  removeContextLayer,
  setContextLayerEnabled,
  type ContextLayer,
  type ContextPreset,
  type LayerStack,
  type PinnedLayer,
} from '@/lib/context-stack'
import { useConnectHostStore } from '@/stores/connect-host'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'
import { useToastStore } from '@/stores/toast'

interface Props {
  /** Absolute workspace path being edited (project.path). */
  projectPath: string
}

/**
 * Per-workspace always-on context stack editor (context hamburger).
 *
 * Pinned rows are read-only (Agent / Project / Tooling). Optional layers
 * support enable toggle, reorder, remove, add-file, and preset chips.
 * Compose happens server-side; this component only mutates the stack via
 * /cli/context/* and keeps optimistic local state (no fetchProjects).
 */
export function ContextStackEditor({ projectPath }: Props): React.JSX.Element {
  const [stack, setStack] = useState<LayerStack | null>(null)
  const [presets, setPresets] = useState<ContextPreset[]>(DEFAULT_PRESET_CHIPS)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const fileInputRef = useRef<HTMLInputElement>(null)
  const projectPathRef = useRef(projectPath)
  projectPathRef.current = projectPath

  const load = useCallback(async (opts?: { silent?: boolean }) => {
    const path = projectPathRef.current
    if (!opts?.silent) {
      setLoading(true)
      setError(null)
    }
    try {
      const next = await fetchContextStack(path)
      if (projectPathRef.current !== path) return
      setStack(next)
      setError(null)
    } catch (err) {
      if (projectPathRef.current !== path) return
      setError(contextErrorMessage(err, 'Failed to load context stack'))
      // Keep any prior stack; if none, show fallback pinned so the UI is usable.
      setStack((prev) => prev ?? {
        pinned: FALLBACK_PINNED,
        layers: [],
        softWarn: false,
        composedBytes: 0,
      })
    } finally {
      if (projectPathRef.current === path && !opts?.silent) setLoading(false)
    }
  }, [])

  useEffect(() => {
    setStack(null)
    setLoading(true)
    void load()
  }, [projectPath, load])

  useEffect(() => {
    let cancelled = false
    void fetchContextPresets()
      .then((list) => {
        if (!cancelled && list.length > 0) setPresets(list)
      })
      .catch(() => {
        /* keep DEFAULT_PRESET_CHIPS */
      })
    return () => {
      cancelled = true
    }
  }, [])

  const toastError = (err: unknown, fallback: string): void => {
    useToastStore.getState().addToast(contextErrorMessage(err, fallback), 'error')
  }

  const withBusy = async (id: string, op: () => Promise<void>): Promise<void> => {
    setBusyId(id)
    try {
      await op()
    } finally {
      setBusyId(null)
    }
  }

  const handleToggle = async (layer: ContextLayer): Promise<void> => {
    const nextEnabled = !layer.enabled
    const prev = stack
    // Optimistic
    setStack((s) =>
      s
        ? {
            ...s,
            layers: s.layers.map((l) =>
              l.id === layer.id ? { ...l, enabled: nextEnabled } : l,
            ),
          }
        : s,
    )
    await withBusy(layer.id, async () => {
      try {
        await setContextLayerEnabled(projectPath, layer.id, nextEnabled)
        await load({ silent: true })
      } catch (err) {
        setStack(prev)
        toastError(err, 'Could not update layer')
      }
    })
  }

  const handleMove = async (
    layer: ContextLayer,
    direction: 'up' | 'down',
  ): Promise<void> => {
    if (!stack) return
    const idx = stack.layers.findIndex((l) => l.id === layer.id)
    if (idx < 0) return
    if (direction === 'up' && idx === 0) return
    if (direction === 'down' && idx >= stack.layers.length - 1) return

    const prev = stack
    const reordered = [...stack.layers]
    const swap = direction === 'up' ? idx - 1 : idx + 1
    ;[reordered[idx], reordered[swap]] = [reordered[swap], reordered[idx]]
    setStack({
      ...stack,
      layers: reordered.map((l, i) => ({ ...l, position: i })),
    })

    await withBusy(layer.id, async () => {
      try {
        await moveContextLayer(projectPath, layer.id, { direction })
        await load({ silent: true })
      } catch (err) {
        setStack(prev)
        toastError(err, 'Could not reorder layer')
      }
    })
  }

  const handleRemove = async (layer: ContextLayer): Promise<void> => {
    const prev = stack
    setStack((s) =>
      s
        ? {
            ...s,
            layers: s.layers.filter((l) => l.id !== layer.id),
          }
        : s,
    )
    await withBusy(layer.id, async () => {
      try {
        await removeContextLayer(projectPath, layer.id)
        await load({ silent: true })
      } catch (err) {
        setStack(prev)
        toastError(err, 'Could not remove layer')
      }
    })
  }

  const addByPath = async (path: string): Promise<void> => {
    setAdding(true)
    try {
      await addContextLayer({ project: projectPath, path })
      await load({ silent: true })
    } catch (err) {
      toastError(err, 'Could not add layer')
    } finally {
      setAdding(false)
    }
  }

  const addByPreset = async (preset: ContextPreset): Promise<void> => {
    setAdding(true)
    try {
      await addContextLayer({
        project: projectPath,
        preset: preset.id,
        label: preset.label,
      })
      await load({ silent: true })
    } catch (err) {
      toastError(err, `Could not add ${preset.label}`)
    } finally {
      setAdding(false)
    }
  }

  /** Host-aware file pick: local uses native input; remote uses RemoteFolderPicker. */
  const handleAddFile = async (): Promise<void> => {
    const activeHost = useConnectHostStore.getState().activeHost
    if (activeHost === 'local') {
      // Native picker. Tauri may expose absolute `.path` on the File object;
      // when it does not, fall back to the host FS browser (local daemon).
      fileInputRef.current?.click()
      return
    }
    const path = await useRemoteFolderPickerStore.getState().open({
      mode: 'file',
      accept: (n) => n.toLowerCase().endsWith('.md'),
      title: 'Add to context stack',
    })
    if (path) await addByPath(path)
  }

  const handleNativeFile = async (
    e: React.ChangeEvent<HTMLInputElement>,
  ): Promise<void> => {
    const file = e.target.files?.[0]
    e.target.value = ''
    if (!file) return

    // Tauri / some webviews attach the absolute path on the File object.
    const pathProp = (file as File & { path?: string }).path
    if (pathProp && pathProp.length > 0) {
      await addByPath(pathProp)
      return
    }

    // No absolute path from the native input — browse the local daemon FS
    // so context/add receives a real host path under the workspace.
    const path = await useRemoteFolderPickerStore.getState().open({
      mode: 'file',
      accept: (n) => n.toLowerCase().endsWith('.md'),
      title: 'Add to context stack',
    })
    if (path) await addByPath(path)
  }

  const pinned: PinnedLayer[] = stack?.pinned?.length ? stack.pinned : FALLBACK_PINNED
  const layers: ContextLayer[] = stack?.layers ?? []
  const softWarn = stack?.softWarn ?? false
  const alreadyPresetIds = new Set(
    layers
      .map((l) => {
        // Match by source (preset:wiki-index) or path.
        if (l.source.startsWith('preset:')) {
          const short = l.source.replace(/^preset:/, '').replace(/-/g, ':')
          // wiki-index → wiki:index style
          if (short.includes(':')) return short
          const parts = l.source.replace(/^preset:/, '').split('-')
          if (parts.length >= 2) return `${parts[0]}:${parts.slice(1).join('-')}`
        }
        return null
      })
      .filter(Boolean) as string[],
  )
  // Also mark by path so chips hide after add.
  const alreadyPaths = new Set(layers.map((l) => l.path))

  const suggestionChips = presets.filter(
    (p) => !alreadyPresetIds.has(p.id) && !alreadyPaths.has(p.path),
  )

  return (
    <div className="space-y-2" data-settings-id="projects.context-stack">
      <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
        Always-on context is a stack of markdown files K2 composes into{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">.k2/AGENTS.md</span>.
        Pinned rows are fixed; optional layers can be toggled, reordered, or removed.
        Manage the same stack from the CLI with{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">k2 agent context</span>.
      </p>

      {softWarn && (
        <div className="text-[10px] leading-snug px-2.5 py-1.5 border border-[color-mix(in_srgb,var(--color-status-warn-amber)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn-amber)_10%,transparent)] text-[var(--color-status-warn-amber-soft)]">
          Stack is getting large
          {stack?.composedBytes
            ? ` (~${formatBytes(stack.composedBytes)})`
            : ''}
          . Big always-on context can bloat every model session — consider
          disabling unused layers.
        </div>
      )}

      {error && !loading && (
        <div className="text-[10px] leading-snug px-2.5 py-1.5 border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error)_8%,transparent)] text-[var(--color-status-error-soft)]">
          Could not load stack: {error}
          <button
            type="button"
            onClick={() => void load()}
            className="ml-2 underline cursor-pointer no-drag"
          >
            Retry
          </button>
        </div>
      )}

      {loading && !stack ? (
        <div className="text-[10px] text-[var(--color-text-muted)] py-2">Loading stack…</div>
      ) : (
        <div className="border border-[var(--color-border)]">
          {/* Pinned */}
          <div className="px-2.5 py-1.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40">
            <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
              Pinned
            </div>
          </div>
          {pinned.map((row) => (
            <PinnedRow key={row.id} layer={row} />
          ))}

          {/* Optional */}
          <div className="px-2.5 py-1.5 border-t border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40">
            <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
              Optional layers
              <span className="normal-case tracking-normal ml-1.5 text-[var(--color-text-muted)]/80">
                (order = AGENTS.md)
              </span>
            </div>
          </div>
          {layers.length === 0 ? (
            <div className="px-2.5 py-2.5 text-[10px] text-[var(--color-text-muted)] italic">
              No optional layers yet. Add a markdown file or a suggestion below.
            </div>
          ) : (
            layers.map((layer, i) => (
              <OptionalRow
                key={layer.id}
                layer={layer}
                isFirst={i === 0}
                isLast={i === layers.length - 1}
                busy={busyId === layer.id || adding}
                onToggle={() => void handleToggle(layer)}
                onUp={() => void handleMove(layer, 'up')}
                onDown={() => void handleMove(layer, 'down')}
                onRemove={() => void handleRemove(layer)}
              />
            ))
          )}
        </div>
      )}

      {/* Actions */}
      <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
        <button
          type="button"
          onClick={() => void handleAddFile()}
          disabled={adding}
          className="px-2.5 py-1 text-[10px] font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
        >
          {adding ? 'Adding…' : 'Add file…'}
        </button>
        <input
          ref={fileInputRef}
          type="file"
          accept=".md,text/markdown,text/x-markdown"
          className="hidden"
          onChange={(e) => void handleNativeFile(e)}
        />

        {suggestionChips.map((p) => (
          <button
            key={p.id}
            type="button"
            onClick={() => void addByPreset(p)}
            disabled={adding}
            title={`${p.label} → ${p.path}`}
            className="px-2 py-1 text-[10px] text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
          >
            + {p.label}
          </button>
        ))}
      </div>

      <p className="text-[9px] text-[var(--color-text-muted)] leading-snug">
        Manager / K2 guidance can be added as optional layers later when those
        presets ship — agent mode no longer injects always-on context by type
        alone.
      </p>
    </div>
  )
}

function PinnedRow({ layer }: { layer: PinnedLayer }): React.JSX.Element {
  const missing = !layer.exists && !layer.generated
  return (
    <div
      className={`flex items-center gap-2 px-2.5 py-1.5 border-b border-[var(--color-border)] last:border-b-0 ${
        missing ? 'bg-[color-mix(in_srgb,var(--color-status-error)_6%,transparent)]' : ''
      }`}
    >
      <span
        className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] w-10 flex-shrink-0"
        title="Pinned — always included"
      >
        pin
      </span>
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-2 min-w-0">
          <span className="text-[11px] font-medium text-[var(--color-text-primary)] flex-shrink-0">
            {layer.label}
          </span>
          {layer.generated ? (
            <span className="text-[9px] text-[var(--color-text-muted)] italic truncate">
              generated k2-cli pointer
            </span>
          ) : (
            <span
              className={`text-[9px] font-mono truncate ${
                missing
                  ? 'text-[var(--color-status-error-soft)]'
                  : 'text-[var(--color-text-muted)]'
              }`}
              title={layer.path}
            >
              {layer.path || '—'}
              {missing ? ' · missing' : ''}
            </span>
          )}
        </div>
      </div>
      <span className="text-[9px] text-[var(--color-text-muted)] flex-shrink-0">read-only</span>
    </div>
  )
}

function OptionalRow({
  layer,
  isFirst,
  isLast,
  busy,
  onToggle,
  onUp,
  onDown,
  onRemove,
}: {
  layer: ContextLayer
  isFirst: boolean
  isLast: boolean
  busy: boolean
  onToggle: () => void
  onUp: () => void
  onDown: () => void
  onRemove: () => void
}): React.JSX.Element {
  const missing = !layer.exists
  const label = layerDisplayLabel(layer)
  return (
    <div
      className={`flex items-center gap-1.5 px-2 py-1.5 border-b border-[var(--color-border)] last:border-b-0 ${
        missing
          ? 'bg-[color-mix(in_srgb,var(--color-status-error)_6%,transparent)]'
          : !layer.enabled
            ? 'opacity-60'
            : ''
      }`}
    >
      <button
        type="button"
        onClick={onToggle}
        role="switch"
        aria-checked={layer.enabled}
        disabled={busy}
        className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
          layer.enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
        title={layer.enabled ? 'Disable layer' : 'Enable layer'}
      >
        <span
          className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
            layer.enabled ? 'translate-x-3.5' : 'translate-x-0.5'
          }`}
        />
      </button>

      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-1.5 min-w-0">
          <span className="text-[11px] font-medium text-[var(--color-text-primary)] flex-shrink-0 truncate max-w-[40%]">
            {label}
          </span>
          <span
            className={`text-[9px] font-mono truncate ${
              missing
                ? 'text-[var(--color-status-error-soft)]'
                : 'text-[var(--color-text-muted)]'
            }`}
            title={layer.path}
          >
            {layer.path}
            {missing ? ' · missing' : ''}
          </span>
        </div>
      </div>

      <div className="flex items-center gap-0.5 flex-shrink-0">
        <button
          type="button"
          onClick={onUp}
          disabled={busy || isFirst}
          className="w-5 h-5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 no-drag cursor-pointer disabled:cursor-not-allowed"
          title="Move up"
          aria-label="Move up"
        >
          ↑
        </button>
        <button
          type="button"
          onClick={onDown}
          disabled={busy || isLast}
          className="w-5 h-5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] disabled:opacity-30 no-drag cursor-pointer disabled:cursor-not-allowed"
          title="Move down"
          aria-label="Move down"
        >
          ↓
        </button>
        <button
          type="button"
          onClick={onRemove}
          disabled={busy}
          className="px-1.5 h-5 text-[10px] text-[var(--color-status-error-soft)] hover:bg-[color-mix(in_srgb,var(--color-status-error)_10%,transparent)] disabled:opacity-30 no-drag cursor-pointer disabled:cursor-not-allowed"
          title="Remove from stack (file stays on disk)"
        >
          ✕
        </button>
      </div>
    </div>
  )
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KiB`
  return `${(n / (1024 * 1024)).toFixed(1)} MiB`
}
