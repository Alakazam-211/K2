import React, { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'
import { FileViewerPane } from '@/components/FileViewerPane/FileViewerPane'
import { DialogFrame, DialogScrim } from '@/components/ui'
import {
  addContextLayer,
  contextErrorMessage,
  DEFAULT_CATALOG_ENTRIES,
  FALLBACK_PINNED,
  FALLBACK_TOOLING_PREVIEW,
  fetchContextCatalog,
  fetchContextStack,
  layerDisplayLabel,
  mergeContextCatalog,
  moveContextLayer,
  removeContextLayer,
  setContextLayerEnabled,
  type ContextLayer,
  type ContextCatalogEntry,
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
  /** Canonical Agent controls — rendered under the stack in the left column. */
  canonicalSlot?: React.ReactNode
}

type Selected =
  | { kind: 'system'; id: string }
  | { kind: 'optional'; id: string }
  | null

/**
 * Per-workspace always-on context stack editor (context management stack).
 *
 * Full-height page split:
 *   Left  — stack + add chips + Canonical Agent
 *   Right — FileViewerPane (same system as AI File Editor) fills remaining space
 */
export function ContextStackEditor({
  projectPath,
  onEdit,
  canonicalSlot,
}: Props): React.JSX.Element {
  const [stack, setStack] = useState<LayerStack | null>(null)
  const [catalog, setCatalog] = useState<ContextCatalogEntry[]>(DEFAULT_CATALOG_ENTRIES)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [adding, setAdding] = useState(false)
  const [selected, setSelected] = useState<Selected>(null)
  const [wikiSeeded, setWikiSeeded] = useState<boolean | null>(null)
  const [seedingWiki, setSeedingWiki] = useState(false)
  const [browseOpen, setBrowseOpen] = useState(false)
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
    setWikiSeeded(null)
    void load()
    void checkWikiSeed()
  }, [projectPath, load, checkWikiSeed])

  const refreshCatalog = useCallback(async (): Promise<void> => {
    try {
      const list = await fetchContextCatalog()
      if (list.length > 0) setCatalog(mergeContextCatalog(list))
    } catch {
      /* keep DEFAULT_CATALOG_ENTRIES / last good list */
    }
  }, [])

  useEffect(() => {
    void refreshCatalog()
  }, [refreshCatalog])

  // Escape closes the browse-defaults modal.
  useEffect(() => {
    if (!browseOpen) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        setBrowseOpen(false)
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [browseOpen])

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

  const addByCatalog = async (entry: ContextCatalogEntry): Promise<void> => {
    setAdding(true)
    try {
      await addContextLayer({
        project: projectPath,
        catalog: entry.id,
        label: entry.label,
      })
      await load({ silent: true })
      await checkWikiSeed()
      useToastStore.getState().addToast(`Added “${entry.label}” to the stack`, 'success')
    } catch (err) {
      toastError(err, `Could not add ${entry.label}`)
    } finally {
      setAdding(false)
    }
  }

  const openBrowseDefaults = (): void => {
    setBrowseOpen(true)
    void refreshCatalog()
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

  const alreadyPaths = useMemo(
    () => new Set(layers.map((l) => l.path)),
    [layers],
  )
  const availableDefaultsCount = useMemo(
    () => catalog.filter((p) => !alreadyPaths.has(p.path)).length,
    [catalog, alreadyPaths],
  )

  const selectedMeta = useMemo(() => {
    if (!selected || !stack) return null
    if (selected.kind === 'system') {
      const row = stack.pinned.find((p) => p.id === selected.id)
      if (!row) return null
      const canEdit = Boolean(row.editable && row.path && !row.generated)
      return {
        label: row.label,
        path: row.path,
        exists: row.exists,
        canEdit,
        isWiki: false,
        isTooling: Boolean(row.generated),
        preview: row.preview,
      }
    }
    const layer = stack.layers.find((l) => l.id === selected.id)
    if (!layer) return null
    const isWiki =
      layer.source.startsWith('catalog:wiki') ||
      layer.path.includes('.k2/wiki/') ||
      layer.path.includes('/wiki/')
    const isLiveRoster =
      layer.source === 'catalog:connections-roster' ||
      layer.source === 'catalog:heartbeats-roster' ||
      layer.source === 'catalog:skills-roster' ||
      layer.source === 'catalog:users-roster' ||
      layer.path.includes('connections-roster.md') ||
      layer.path.includes('heartbeats-roster.md') ||
      layer.path.includes('skills-roster.md') ||
      layer.path.includes('users-roster.md')
    // Wiki + live rosters: View only (not AI File Editor).
    const canEdit = !isWiki && !isLiveRoster && layer.exists && !!layer.path
    return {
      label: layerDisplayLabel(layer),
      path: layer.path,
      exists: layer.exists,
      canEdit,
      isWiki: isWiki || isLiveRoster,
      isTooling: false,
      preview: undefined,
    }
  }, [selected, stack])

  /** Absolute path for FileViewerPane — null for tooling / no selection / missing path. */
  const viewerAbsPath = useMemo(() => {
    if (!selectedMeta || selectedMeta.isTooling || !selectedMeta.path) return null
    if (selectedMeta.path.startsWith('/')) return selectedMeta.path
    return `${projectPath.replace(/\/$/, '')}/${selectedMeta.path.replace(/^\//, '')}`
  }, [selectedMeta, projectPath])

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
    if (!viewerAbsPath) return
    onEdit({ kind: 'file', absPath: viewerAbsPath, label: selectedMeta.label })
  }

  return (
    <>
    <div
      className="flex flex-row h-full min-h-0 w-full border border-[var(--color-border)]"
      data-settings-id="projects.context-stack"
    >
      {/* ── LEFT: stack + chips + Canonical Agent ── */}
      <div className="w-[min(36%,calc(20rem+30px))] min-w-[calc(15.5rem+30px)] max-w-[calc(24rem+30px)] flex-shrink-0 border-r border-[var(--color-border)] min-h-0 flex flex-col bg-[var(--color-bg)]">
        <div className="flex-1 min-h-0 overflow-y-auto [scrollbar-gutter:stable]">
          <div className="px-2.5 py-1.5 border-b border-[var(--color-border)]">
            <div className="text-[10px] font-medium text-[var(--color-text-primary)]">
              Context stack
            </div>
            <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5 leading-snug">
              Composes into{' '}
              <span className="font-mono">.k2/AGENTS.md</span>
              {softWarn && stack?.composedBytes
                ? ` · large (~${formatBytes(stack.composedBytes)})`
                : ''}
            </p>
          </div>

          {error && !loading && (
            <div className="mx-2 my-1.5 text-[10px] leading-snug px-2 py-1.5 border border-[color-mix(in_srgb,var(--color-status-error)_30%,transparent)] bg-[color-mix(in_srgb,var(--color-status-error)_8%,transparent)] text-[var(--color-status-error-soft)]">
              {error}
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
            <div className="mx-2 my-1.5 flex items-center justify-between gap-2 text-[10px] leading-snug px-2 py-1.5 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40 text-[var(--color-text-secondary)]">
              <span>Wiki not seeded</span>
              <button
                type="button"
                onClick={() => void handleSeedWiki()}
                disabled={seedingWiki}
                className="flex-shrink-0 px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-50"
              >
                {seedingWiki ? '…' : 'Seed'}
              </button>
            </div>
          )}

          {loading && !stack ? (
            <div className="px-2.5 py-3 text-[10px] text-[var(--color-text-muted)]">
              Loading stack…
            </div>
          ) : (
            <>
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
                  Optional
                </div>
              </div>
              {layers.length === 0 ? (
                <div className="px-2.5 py-2.5 text-[10px] text-[var(--color-text-muted)] italic">
                  No optional layers yet.
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
            </>
          )}

          <div className="px-2 py-2 flex flex-wrap items-center gap-1.5 border-t border-[var(--color-border)]">
            <button
              type="button"
              onClick={() => void handleAddFile()}
              disabled={adding}
              className="px-2 py-1 text-[10px] font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-50"
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
            <button
              type="button"
              onClick={openBrowseDefaults}
              disabled={adding}
              title={
                availableDefaultsCount > 0
                  ? `${availableDefaultsCount} catalog item${availableDefaultsCount === 1 ? '' : 's'} available`
                  : 'Browse context catalog'
              }
              className="px-2 py-1 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/20 no-drag cursor-pointer disabled:opacity-50"
            >
              Browse catalog
            </button>
          </div>
        </div>

        {canonicalSlot && (
          <div className="flex-shrink-0 border-t border-[var(--color-border)] px-2.5 py-2.5 bg-[var(--color-bg-elevated)]/30">
            {canonicalSlot}
          </div>
        )}
      </div>

      {/* ── RIGHT: full-height FileViewer (same system as AI File Editor) ── */}
      <div className="flex-1 min-w-0 min-h-0 flex flex-col">
        <div className="flex-shrink-0 h-8 px-2.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40 flex items-center justify-between gap-2">
          <div className="text-[10px] text-[var(--color-text-muted)] truncate min-w-0">
            {selectedMeta ? (
              <>
                <span className="text-[var(--color-text-primary)] font-medium">
                  {selectedMeta.label}
                </span>
                {selectedMeta.path ? (
                  <span className="ml-1.5 font-mono opacity-70">{selectedMeta.path}</span>
                ) : null}
              </>
            ) : (
              'Select a layer'
            )}
          </div>
          <div className="flex items-center gap-1.5 flex-shrink-0">
            {(selectedMeta?.isWiki || selectedMeta?.isTooling) && (
              <span className="text-[9px] text-[var(--color-text-muted)] italic">view only</span>
            )}
            {selectedMeta?.canEdit && onEdit && (
              <button
                type="button"
                onClick={handleEdit}
                className="px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 hover:bg-[var(--color-accent)]/20 border border-[var(--color-accent)]/30 no-drag cursor-pointer"
              >
                Edit with AI
              </button>
            )}
          </div>
        </div>

        <div className="flex-1 min-h-0 relative">
          {!selected && (
            <div className="absolute inset-0 flex items-center justify-center text-[11px] text-[var(--color-text-muted)]">
              Select a context layer to read it here
            </div>
          )}
          {selectedMeta?.isTooling && (
            <div className="absolute inset-0 overflow-y-auto p-4 [scrollbar-gutter:stable]">
              <p className="text-[10px] text-[var(--color-text-muted)] leading-snug mb-3">
                Generated block inlined into{' '}
                <span className="font-mono text-[var(--color-text-secondary)]">AGENTS.md</span>
                {' '}as <span className="font-mono">## Tooling</span> when this layer is on.
                There is no file to edit.
              </p>
              <div className="prose prose-invert prose-sm max-w-none text-[12px] text-[var(--color-text-secondary)]">
                <Markdown remarkPlugins={[remarkGfm]}>
                  {selectedMeta.preview?.trim() || FALLBACK_TOOLING_PREVIEW}
                </Markdown>
              </div>
            </div>
          )}
          {selectedMeta && !selectedMeta.isTooling && !viewerAbsPath && (
            <div className="absolute inset-0 flex items-center justify-center text-[11px] text-[var(--color-status-error-soft)]">
              No file path for this layer
            </div>
          )}
          {viewerAbsPath && !selectedMeta?.isTooling && (
            <div className="absolute inset-0">
              <FileViewerPane
                key={viewerAbsPath}
                filePath={viewerAbsPath}
                paneId="settings-context-viewer"
                tabId="settings-context-viewer"
              />
            </div>
          )}
        </div>
      </div>
    </div>

    {browseOpen && (
      <BrowseDefaultsModal
        catalog={catalog}
        alreadyPaths={alreadyPaths}
        wikiSeeded={wikiSeeded}
        seedingWiki={seedingWiki}
        adding={adding}
        onClose={() => setBrowseOpen(false)}
        onAdd={(p) => void addByCatalog(p)}
        onSeedWiki={() => void handleSeedWiki()}
      />
    )}
    </>
  )
}

type CatalogFilter = 'all' | 'recommended' | 'live' | 'wiki'

const CATALOG_FILTERS: { id: CatalogFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'recommended', label: 'Recommended' },
  { id: 'live', label: 'Live' },
  { id: 'wiki', label: 'Wiki' },
]

function hasTag(p: ContextCatalogEntry, tag: string): boolean {
  return (p.tags ?? []).some((t) => t.toLowerCase() === tag.toLowerCase())
}

function isRecommended(p: ContextCatalogEntry): boolean {
  // Boolean field only — never treat free-form tags as recommended.
  return Boolean(p.recommended)
}

function matchesCatalogFilter(p: ContextCatalogEntry, filter: CatalogFilter): boolean {
  switch (filter) {
    case 'all':
      return true
    case 'recommended':
      return isRecommended(p)
    case 'live':
      return p.kind === 'live' || hasTag(p, 'live')
    case 'wiki':
      return hasTag(p, 'wiki') || p.id.startsWith('wiki:')
    default:
      return true
  }
}

/** Modal: host context catalog (built-in + future installed packs). */
function BrowseDefaultsModal({
  catalog,
  alreadyPaths,
  wikiSeeded,
  seedingWiki,
  adding,
  onClose,
  onAdd,
  onSeedWiki,
}: {
  catalog: ContextCatalogEntry[]
  alreadyPaths: Set<string>
  wikiSeeded: boolean | null
  seedingWiki: boolean
  adding: boolean
  onClose: () => void
  onAdd: (entry: ContextCatalogEntry) => void
  onSeedWiki: () => void
}): React.JSX.Element {
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<CatalogFilter>('all')
  const searchRef = useRef<HTMLInputElement>(null)

  useEffect(() => {
    // Focus search on open so typing filters immediately.
    const t = window.setTimeout(() => searchRef.current?.focus(), 0)
    return () => window.clearTimeout(t)
  }, [])

  const needsWikiSeed = wikiSeeded === false
  const showSeedBanner = needsWikiSeed && catalog.some((p) => p.id.startsWith('wiki:'))

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    let list = catalog.filter((p) => matchesCatalogFilter(p, filter))
    if (q) {
      list = list.filter((p) => {
        const hay = [
          p.label,
          p.description ?? '',
          p.path,
          p.id,
          p.source,
          p.kind ?? '',
          p.author ?? '',
          p.version ?? '',
          ...(p.tags ?? []),
        ]
          .join(' ')
          .toLowerCase()
        return hay.includes(q)
      })
    }
    // Always A–Z by label (including when filtering Recommended / Live / Wiki).
    list.sort((a, b) =>
      a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }),
    )
    return list
  }, [catalog, query, filter])

  const availableCount = useMemo(
    () => filtered.filter((p) => !alreadyPaths.has(p.path)).length,
    [filtered, alreadyPaths],
  )

  const filterCounts = useMemo(() => {
    const counts: Record<CatalogFilter, number> = {
      all: catalog.length,
      recommended: 0,
      live: 0,
      wiki: 0,
    }
    for (const p of catalog) {
      if (isRecommended(p)) counts.recommended += 1
      if (p.kind === 'live' || hasTag(p, 'live')) counts.live += 1
      if (hasTag(p, 'wiki') || p.id.startsWith('wiki:')) counts.wiki += 1
    }
    return counts
  }, [catalog])

  return (
    <>
      <DialogScrim
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby="context-catalog-title"
        className="w-[min(calc(44rem+100px),calc(100vw-2.5rem))] h-[min(calc(36rem+150px),calc(100vh-3rem))] max-h-[calc(100vh-3rem)] flex flex-col overflow-hidden"
        style={{
          padding: 0,
          fontFamily:
            "'MesloLGM Nerd Font', Menlo, Monaco, 'Cascadia Code', 'Fira Code', 'SF Mono', Consolas, monospace",
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div className="flex-shrink-0 px-5 pt-4 pb-3 border-b border-[var(--color-border)] space-y-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <h2
                id="context-catalog-title"
                className="text-[14px] font-semibold text-[var(--color-text-primary)]"
              >
                Context catalog
              </h2>
              <p className="text-[11px] text-[var(--color-text-muted)] mt-1 leading-snug">
                Packs and live layers available on this machine for always-on
                context. Start with <span className="text-[var(--color-accent)]">Recommended</span>{' '}
                for a nice K2 experience.
              </p>
            </div>
            <button
              type="button"
              onClick={onClose}
              className="flex-shrink-0 px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
              aria-label="Close"
            >
              Esc
            </button>
          </div>

          <div className="relative">
            <input
              ref={searchRef}
              type="search"
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search catalog…"
              aria-label="Search context catalog"
              className="w-full px-3 py-2 text-[12px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]/50 no-drag"
            />
            {query.trim() ? (
              <button
                type="button"
                onClick={() => {
                  setQuery('')
                  searchRef.current?.focus()
                }}
                className="absolute right-2 top-1/2 -translate-y-1/2 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] no-drag cursor-pointer px-1.5 py-0.5"
              >
                Clear
              </button>
            ) : null}
          </div>

          <div
            className="flex flex-wrap items-center gap-1.5"
            role="tablist"
            aria-label="Catalog filters"
          >
            {CATALOG_FILTERS.map((f) => {
              const active = filter === f.id
              const count = filterCounts[f.id]
              return (
                <button
                  key={f.id}
                  type="button"
                  role="tab"
                  aria-selected={active}
                  onClick={() => setFilter(f.id)}
                  className={`px-2.5 py-1 text-[10px] font-medium no-drag cursor-pointer border transition-colors ${
                    active
                      ? 'text-[var(--color-accent)] bg-[var(--color-accent)]/15 border-[var(--color-accent)]/40'
                      : 'text-[var(--color-text-secondary)] border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)]'
                  }`}
                >
                  {f.label}
                  <span className="ml-1 tabular-nums opacity-70">{count}</span>
                </button>
              )
            })}
          </div>

          {showSeedBanner && (
            <div className="flex items-center justify-between gap-2 text-[10px] leading-snug px-2.5 py-1.5 border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/50 text-[var(--color-text-secondary)]">
              <span>Wiki files aren’t seeded yet — seed before adding wiki layers.</span>
              <button
                type="button"
                onClick={onSeedWiki}
                disabled={seedingWiki}
                className="flex-shrink-0 px-2 py-0.5 text-[10px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-50"
              >
                {seedingWiki ? 'Seeding…' : 'Seed wiki'}
              </button>
            </div>
          )}
        </div>

        <div className="flex-1 min-h-0 overflow-y-auto overscroll-contain [scrollbar-gutter:stable]">
          {catalog.length === 0 ? (
            <div className="px-5 py-10 text-[12px] text-[var(--color-text-muted)] italic text-center">
              No catalog items available.
            </div>
          ) : filtered.length === 0 ? (
            <div className="px-5 py-10 text-[12px] text-[var(--color-text-muted)] text-center">
              {query.trim()
                ? `No catalog items match “${query.trim()}”.`
                : 'No items in this filter.'}
            </div>
          ) : (
            <ul className="divide-y divide-[var(--color-border)]">
              {filtered.map((p) => {
                const inStack = alreadyPaths.has(p.path)
                const isWiki = p.id.startsWith('wiki:')
                const blockedByWiki = isWiki && needsWikiSeed
                const canAdd = !inStack && !blockedByWiki && !adding
                const recommended = isRecommended(p)
                const metaBits = [
                  p.kind ? p.kind : null,
                  p.author ? p.author : null,
                  p.version ? `v${p.version}` : null,
                ].filter(Boolean) as string[]
                const displayTags = p.tags ?? []
                return (
                  <li
                    key={p.id}
                    className={`px-5 py-3.5 flex items-start gap-4 ${
                      inStack ? 'opacity-70' : 'hover:bg-[var(--color-bg-elevated)]/40'
                    }`}
                  >
                    <div className="flex-1 min-w-0">
                      <div className="flex items-baseline gap-2 min-w-0 flex-wrap">
                        <span className="text-[13px] font-medium text-[var(--color-text-primary)]">
                          {p.label}
                        </span>
                        {recommended ? (
                          <span className="px-1.5 py-0.5 text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/12 border border-[var(--color-accent)]/30">
                            Recommended
                          </span>
                        ) : null}
                        {metaBits.length > 0 ? (
                          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">
                            {metaBits.join(' · ')}
                          </span>
                        ) : null}
                      </div>
                      {p.description ? (
                        <p className="text-[11px] text-[var(--color-text-secondary)] mt-1 leading-snug">
                          {p.description}
                        </p>
                      ) : null}
                      {displayTags.length > 0 ? (
                        <div className="flex flex-wrap gap-1 mt-1.5">
                          {displayTags.map((tag) => (
                            <span
                              key={tag}
                              className="px-1.5 py-0.5 text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40"
                            >
                              {tag}
                            </span>
                          ))}
                        </div>
                      ) : null}
                      <div
                        className="text-[10px] font-mono text-[var(--color-text-muted)] mt-1.5 truncate"
                        title={p.path}
                      >
                        {p.path}
                      </div>
                    </div>
                    <div className="flex-shrink-0 pt-0.5">
                      {inStack ? (
                        <span className="inline-block px-2.5 py-1 text-[11px] text-[var(--color-text-muted)] border border-[var(--color-border)]">
                          In stack
                        </span>
                      ) : blockedByWiki ? (
                        <span
                          className="inline-block px-2.5 py-1 text-[11px] text-[var(--color-text-muted)] border border-[var(--color-border)]"
                          title="Seed the wiki first"
                        >
                          Needs wiki
                        </span>
                      ) : (
                        <button
                          type="button"
                          onClick={() => onAdd(p)}
                          disabled={!canAdd}
                          className="px-3 py-1.5 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/20 no-drag cursor-pointer disabled:opacity-50"
                        >
                          {adding ? '…' : 'Add'}
                        </button>
                      )}
                    </div>
                  </li>
                )
              })}
            </ul>
          )}
        </div>

        <div className="flex-shrink-0 px-5 py-3 border-t border-[var(--color-border)] flex items-center justify-between gap-3">
          <span className="text-[10px] text-[var(--color-text-muted)] tabular-nums">
            {query.trim() || filter !== 'all'
              ? `${filtered.length} match${filtered.length === 1 ? '' : 'es'}`
              : `${catalog.length} catalog item${catalog.length === 1 ? '' : 's'}`}
            {availableCount > 0 ? ` · ${availableCount} not in stack` : ''}
          </span>
          <button
            type="button"
            onClick={onClose}
            className="px-3 py-1.5 text-[11px] font-medium text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
          >
            Done
          </button>
        </div>
      </DialogFrame>
    </>
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
