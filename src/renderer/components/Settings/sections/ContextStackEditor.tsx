import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
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
import { daemonCliGet } from '@/lib/daemon-cli'
import { seedWiki } from '@/components/Wiki/wiki-api'
import { useConnectHostStore } from '@/stores/connect-host'
import { useRemoteFolderPickerStore } from '@/stores/remote-folder-picker'
import { useToastStore } from '@/stores/toast'

/** What Edit should open in the parent Settings takeover. */
export type ContextEditTarget =
  | { kind: 'agent' }
  | { kind: 'project' }
  | { kind: 'file'; absPath: string; label: string }

interface Props {
  projectPath: string
  /** Open AI File Editor (or persona/project editors) for a stack entry. */
  onEdit?: (target: ContextEditTarget) => void
}

const TOOLING_PREVIEW = `## Tooling

This workspace is managed by **K2**. You have the \`k2\` CLI — load the **k2-cli** skill (\`.k2/skills/k2-cli/SKILL.md\`) for the full command reference (\`msg\`, \`inbox\`, \`activity\`, \`connections\`, \`heartbeat\`, \`feedback\`, \`project\`, \`mail\`).

This section is generated — toggle it on/off in the stack; there is no separate file to edit.
`

type Selected =
  | { kind: 'system'; id: string }
  | { kind: 'optional'; id: string }
  | null

/**
 * Per-workspace always-on context stack editor (context hamburger).
 *
 * Left: system + optional layers (toggles, reorder, remove, add).
 * Right: View of the selected .md (or generated tooling text).
 * Edit opens the AI File Editor via `onEdit` (not for wiki packs / tooling).
 */
export function ContextStackEditor({ projectPath, onEdit }: Props): React.JSX.Element {
  const [stack, setStack] = useState<LayerStack | null>(null)
  const [presets, setPresets] = useState<ContextPreset[]>(DEFAULT_PRESET_CHIPS)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [selected, setSelected] = useState<Selected>(null)
  const [viewBody, setViewBody] = useState<string>('')
  const [viewLoading, setViewLoading] = useState(false)
  const [viewError, setViewError] = useState<string | null>(null)
  const [wikiSeeded, setWikiSeeded] = useState<boolean | null>(null)
  const [seedingWiki, setSeedingWiki] = useState(false)
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
      // Default select first system layer when nothing selected.
      setSelected((prev) => {
        if (prev) return prev
        const first = next.pinned[0]
        return first ? { kind: 'system', id: first.id } : null
      })
    } catch (err) {
      if (projectPathRef.current !== path) return
      setError(contextErrorMessage(err, 'Failed to load context stack'))
      setStack((prev) => prev ?? {
        pinned: FALLBACK_PINNED.map((p) => ({ ...p, enabled: true, editable: !p.generated })),
        layers: [],
        softWarn: false,
        composedBytes: 0,
      })
    } finally {
      if (projectPathRef.current === path && !opts?.silent) setLoading(false)
    }
  }, [])

  const checkWikiSeed = useCallback(async () => {
    const path = projectPathRef.current
    try {
      // Seeded if either Home or _Index exists under .k2/wiki/
      const indexPath = `${path}/.k2/wiki/_Index.md`
      const homePath = `${path}/.k2/wiki/Home.md`
      const [i, h] = await Promise.all([
        daemonCliGet<{ content?: string }>('fs/read-file', { path: indexPath }).then(() => true).catch(() => false),
        daemonCliGet<{ content?: string }>('fs/read-file', { path: homePath }).then(() => true).catch(() => false),
      ])
      if (projectPathRef.current === path) setWikiSeeded(i || h)
    } catch {
      if (projectPathRef.current === path) setWikiSeeded(false)
    }
  }, [])

  useEffect(() => {
    setStack(null)
    setLoading(true)
    setSelected(null)
    setViewBody('')
    setWikiSeeded(null)
    void load()
    void checkWikiSeed()
  }, [projectPath, load, checkWikiSeed])

  useEffect(() => {
    let cancelled = false
    void fetchContextPresets()
      .then((list) => {
        if (!cancelled && list.length > 0) setPresets(list)
      })
      .catch(() => { /* keep DEFAULT_PRESET_CHIPS */ })
    return () => {
      cancelled = true
    }
  }, [])

  // Load view body when selection changes.
  useEffect(() => {
    if (!stack || !selected) {
      setViewBody('')
      setViewError(null)
      return
    }
    let cancelled = false
    const run = async () => {
      setViewLoading(true)
      setViewError(null)
      try {
        if (selected.kind === 'system') {
          const row = stack.pinned.find((p) => p.id === selected.id)
          if (!row) {
            setViewBody('')
            return
          }
          if (row.generated || row.id === 'pinned:tooling') {
            if (!cancelled) setViewBody(TOOLING_PREVIEW)
            return
          }
          if (!row.path || !row.exists) {
            if (!cancelled) {
              setViewBody('')
              setViewError(row.exists === false ? 'File missing on disk.' : 'No path for this layer.')
            }
            return
          }
          const abs = row.path.startsWith('/')
            ? row.path
            : `${projectPath.replace(/\/$/, '')}/${row.path.replace(/^\//, '')}`
          const r = await daemonCliGet<{ content: string }>('fs/read-file', { path: abs })
          if (!cancelled) setViewBody(r.content || '*(empty file)*')
          return
        }
        const layer = stack.layers.find((l) => l.id === selected.id)
        if (!layer) {
          setViewBody('')
          return
        }
        if (!layer.exists) {
          if (!cancelled) {
            setViewBody('')
            setViewError('File missing on disk — re-add the path or restore the file.')
          }
          return
        }
        const abs = layer.path.startsWith('/')
          ? layer.path
          : `${projectPath.replace(/\/$/, '')}/${layer.path.replace(/^\//, '')}`
        const r = await daemonCliGet<{ content: string }>('fs/read-file', { path: abs })
        if (!cancelled) setViewBody(r.content || '*(empty file)*')
      } catch (err) {
        if (!cancelled) {
          setViewBody('')
          setViewError(contextErrorMessage(err, 'Could not read file'))
        }
      } finally {
        if (!cancelled) setViewLoading(false)
      }
    }
    void run()
    return () => {
      cancelled = true
    }
  }, [selected, stack, projectPath])

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

  const handleToggleSystem = async (layer: PinnedLayer): Promise<void> => {
    const nextEnabled = !(layer.enabled ?? true)
    const prev = stack
    setStack((s) =>
      s
        ? {
            ...s,
            pinned: s.pinned.map((l) =>
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

  const handleToggle = async (layer: ContextLayer): Promise<void> => {
    const nextEnabled = !layer.enabled
    const prev = stack
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
    if (selected?.kind === 'optional' && selected.id === layer.id) {
      setSelected(stack?.pinned[0] ? { kind: 'system', id: stack.pinned[0].id } : null)
    }
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
      await checkWikiSeed()
    } catch (err) {
      toastError(err, `Could not add ${preset.label}`)
    } finally {
      setAdding(false)
    }
  }

  const handleSeedWiki = async (): Promise<void> => {
    setSeedingWiki(true)
    try {
      await seedWiki(projectPath)
      useToastStore.getState().addToast('Wiki seeded (Home.md + _Index.md)', 'success')
      await checkWikiSeed()
    } catch (err) {
      toastError(err, 'Could not seed wiki')
    } finally {
      setSeedingWiki(false)
    }
  }

  const handleAddFile = async (): Promise<void> => {
    const activeHost = useConnectHostStore.getState().activeHost
    if (activeHost === 'local') {
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
    const pathProp = (file as File & { path?: string }).path
    if (pathProp && pathProp.length > 0) {
      await addByPath(pathProp)
      return
    }
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

  const alreadyPaths = new Set(layers.map((l) => l.path))
  const suggestionChips = presets.filter((p) => !alreadyPaths.has(p.path))

  const selectedMeta = useMemo(() => {
    if (!selected || !stack) return null
    if (selected.kind === 'system') {
      const row = stack.pinned.find((p) => p.id === selected.id)
      if (!row) return null
      const isWiki = false
      const canEdit = Boolean(row.editable && row.path && !row.generated)
      return {
        label: row.label,
        path: row.path,
        canEdit,
        isWiki,
        isTooling: Boolean(row.generated),
        systemId: row.id,
      }
    }
    const layer = stack.layers.find((l) => l.id === selected.id)
    if (!layer) return null
    const isWiki =
      layer.source.startsWith('preset:wiki') ||
      layer.path.includes('.k2/wiki/') ||
      layer.path.includes('/wiki/')
    // Wiki notes: View only (not AI File Editor persona-style edit).
    const canEdit = !isWiki && layer.exists && !!layer.path
    return {
      label: layerDisplayLabel(layer),
      path: layer.path,
      canEdit,
      isWiki,
      isTooling: false,
      systemId: null as string | null,
    }
  }, [selected, stack])

  const handleEdit = (): void => {
    if (!onEdit || !selectedMeta?.canEdit || !stack || !selected) return
    if (selected.kind === 'system') {
      if (selected.id === 'pinned:agent') {
        onEdit({ kind: 'agent' })
        return
      }
      if (selected.id === 'pinned:project') {
        onEdit({ kind: 'project' })
        return
      }
    }
    const abs = selectedMeta.path.startsWith('/')
      ? selectedMeta.path
      : `${projectPath.replace(/\/$/, '')}/${selectedMeta.path.replace(/^\//, '')}`
    onEdit({ kind: 'file', absPath: abs, label: selectedMeta.label })
  }

  return (
    <div className="space-y-2" data-settings-id="projects.context-stack">
      <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
        Always-on context is a stack of markdown files K2 composes into{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">.k2/AGENTS.md</span>.
        Toggle system defaults (persona, project knowledge, k2-cli pointer) or add optional layers.
        CLI:{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">k2 agent context</span>.
      </p>

      {softWarn && (
        <div className="text-[10px] leading-snug px-2.5 py-1.5 border border-[color-mix(in_srgb,var(--color-status-warn-amber)_40%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn-amber)_10%,transparent)] text-[var(--color-status-warn-amber-soft)]">
          Stack is getting large
          {stack?.composedBytes ? ` (~${formatBytes(stack.composedBytes)})` : ''}.
          Big always-on context can bloat every model session — consider disabling unused layers.
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

      {wikiSeeded === false && (
        <div className="flex items-center justify-between gap-2 text-[10px] leading-snug px-2.5 py-1.5 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40 text-[var(--color-text-secondary)]">
          <span>
            Wiki not seeded — no{' '}
            <span className="font-mono">.k2/wiki/Home.md</span> or{' '}
            <span className="font-mono">_Index.md</span> yet. Seed before adding wiki layers,
            or create notes manually.
          </span>
          <button
            type="button"
            onClick={() => void handleSeedWiki()}
            disabled={seedingWiki}
            className="flex-shrink-0 px-2 py-1 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-50"
          >
            {seedingWiki ? 'Seeding…' : 'Seed wiki'}
          </button>
        </div>
      )}

      {loading && !stack ? (
        <div className="text-[10px] text-[var(--color-text-muted)] py-2">Loading stack…</div>
      ) : (
        <div className="border border-[var(--color-border)] grid grid-cols-1 lg:grid-cols-2 min-h-[220px]">
          {/* Left: stack list */}
          <div className="border-b lg:border-b-0 lg:border-r border-[var(--color-border)] min-w-0 flex flex-col">
            <div className="px-2.5 py-1.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40">
              <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
                System (default on)
              </div>
            </div>
            {pinned.map((row) => (
              <SystemRow
                key={row.id}
                layer={row}
                selected={selected?.kind === 'system' && selected.id === row.id}
                busy={busyId === row.id || adding}
                onSelect={() => setSelected({ kind: 'system', id: row.id })}
                onToggle={() => void handleToggleSystem(row)}
              />
            ))}

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
                  selected={selected?.kind === 'optional' && selected.id === layer.id}
                  isFirst={i === 0}
                  isLast={i === layers.length - 1}
                  busy={busyId === layer.id || adding}
                  onSelect={() => setSelected({ kind: 'optional', id: layer.id })}
                  onToggle={() => void handleToggle(layer)}
                  onUp={() => void handleMove(layer, 'up')}
                  onDown={() => void handleMove(layer, 'down')}
                  onRemove={() => void handleRemove(layer)}
                />
              ))
            )}
          </div>

          {/* Right: viewer */}
          <div className="min-w-0 flex flex-col bg-[var(--color-bg)]/40">
            <div className="px-2.5 py-1.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40 flex items-center justify-between gap-2">
              <div className="min-w-0">
                <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
                  View
                </div>
                {selectedMeta && (
                  <div className="text-[10px] text-[var(--color-text-primary)] truncate" title={selectedMeta.path}>
                    {selectedMeta.label}
                    {selectedMeta.path ? (
                      <span className="ml-1.5 font-mono text-[var(--color-text-muted)]">
                        {selectedMeta.path}
                      </span>
                    ) : null}
                  </div>
                )}
              </div>
              <div className="flex items-center gap-1 flex-shrink-0">
                {selectedMeta?.canEdit && onEdit && (
                  <button
                    type="button"
                    onClick={handleEdit}
                    className="px-2 py-1 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 no-drag cursor-pointer"
                  >
                    Edit
                  </button>
                )}
                {selectedMeta?.isWiki && (
                  <span className="text-[9px] text-[var(--color-text-muted)] italic px-1">
                    wiki · view only
                  </span>
                )}
                {selectedMeta?.isTooling && (
                  <span className="text-[9px] text-[var(--color-text-muted)] italic px-1">
                    generated
                  </span>
                )}
              </div>
            </div>
            <div className="flex-1 overflow-auto px-3 py-2 min-h-[160px] max-h-[360px]">
              {!selected && (
                <p className="text-[10px] text-[var(--color-text-muted)] italic">
                  Select a layer to view its content.
                </p>
              )}
              {selected && viewLoading && (
                <p className="text-[10px] text-[var(--color-text-muted)]">Loading…</p>
              )}
              {selected && viewError && !viewLoading && (
                <p className="text-[10px] text-[var(--color-status-error-soft)]">{viewError}</p>
              )}
              {selected && !viewLoading && !viewError && viewBody && (
                <div className="prose prose-invert prose-sm max-w-none text-[11px] leading-relaxed text-[var(--color-text-secondary)] [&_h1]:text-sm [&_h2]:text-xs [&_h3]:text-[11px]">
                  <Markdown remarkPlugins={[remarkGfm]}>{viewBody}</Markdown>
                </div>
              )}
            </div>
          </div>
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

        {suggestionChips.map((p) => {
          const isWikiPreset = p.id.startsWith('wiki:')
          const needsSeed = isWikiPreset && wikiSeeded === false
          return (
            <button
              key={p.id}
              type="button"
              onClick={() => void addByPreset(p)}
              disabled={adding || needsSeed}
              title={
                needsSeed
                  ? 'Seed the wiki first (Home.md / _Index.md missing)'
                  : `${p.label} → ${p.path}`
              }
              className="px-2 py-1 text-[10px] text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
            >
              + {p.label}
              {needsSeed ? ' (seed wiki first)' : ''}
            </button>
          )
        })}
      </div>
    </div>
  )
}

function SystemRow({
  layer,
  selected,
  busy,
  onSelect,
  onToggle,
}: {
  layer: PinnedLayer
  selected: boolean
  busy: boolean
  onSelect: () => void
  onToggle: () => void
}): React.JSX.Element {
  const enabled = layer.enabled ?? true
  const missing = !layer.exists && !layer.generated
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onSelect()
        }
      }}
      className={`flex items-center gap-1.5 px-2 py-1.5 border-b border-[var(--color-border)] cursor-pointer no-drag ${
        selected
          ? 'bg-[var(--color-accent)]/10'
          : missing
            ? 'bg-[color-mix(in_srgb,var(--color-status-error)_6%,transparent)]'
            : !enabled
              ? 'opacity-60'
              : 'hover:bg-[var(--color-bg-elevated)]/50'
      }`}
    >
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation()
          onToggle()
        }}
        role="switch"
        aria-checked={enabled}
        disabled={busy}
        className={`w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 ${
          enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
        }`}
        title={enabled ? 'Exclude from AGENTS.md' : 'Include in AGENTS.md'}
      >
        <span
          className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
            enabled ? 'translate-x-3.5' : 'translate-x-0.5'
          }`}
        />
      </button>
      <div className="flex-1 min-w-0">
        <div className="flex items-baseline gap-1.5 min-w-0">
          <span className="text-[11px] font-medium text-[var(--color-text-primary)] flex-shrink-0">
            {layer.label}
          </span>
          {layer.generated ? (
            <span className="text-[9px] text-[var(--color-text-muted)] italic truncate">
              generated
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
    </div>
  )
}

function OptionalRow({
  layer,
  selected,
  isFirst,
  isLast,
  busy,
  onSelect,
  onToggle,
  onUp,
  onDown,
  onRemove,
}: {
  layer: ContextLayer
  selected: boolean
  isFirst: boolean
  isLast: boolean
  busy: boolean
  onSelect: () => void
  onToggle: () => void
  onUp: () => void
  onDown: () => void
  onRemove: () => void
}): React.JSX.Element {
  const missing = !layer.exists
  const label = layerDisplayLabel(layer)
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault()
          onSelect()
        }
      }}
      className={`flex items-center gap-1.5 px-2 py-1.5 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer no-drag ${
        selected
          ? 'bg-[var(--color-accent)]/10'
          : missing
            ? 'bg-[color-mix(in_srgb,var(--color-status-error)_6%,transparent)]'
            : !layer.enabled
              ? 'opacity-60'
              : 'hover:bg-[var(--color-bg-elevated)]/50'
      }`}
    >
      <button
        type="button"
        onClick={(e) => {
          e.stopPropagation()
          onToggle()
        }}
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

      <div className="flex items-center gap-0.5 flex-shrink-0" onClick={(e) => e.stopPropagation()}>
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
