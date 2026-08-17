import React, { useCallback, useEffect, useMemo, useState } from 'react'
import {
  contextErrorMessage,
  createContextCatalogPack,
  DEFAULT_CATALOG_ENTRIES,
  deleteContextCatalogPack,
  fetchContextCatalog,
  mergeContextCatalog,
  type ContextCatalogEntry,
} from '@/lib/context-stack'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import type { SettingEntry } from '../searchManifest'
import { ContextCatalogCreator } from '../ContextCatalogCreator'

export const CONTEXT_CATALOG_MANIFEST: SettingEntry[] = [
  {
    id: 'context-catalog.library',
    section: 'context-catalog',
    label: 'Context Catalog',
    description: 'Host library of context packs (builtins + user packs). Does not stack on a workspace.',
    keywords: ['context', 'catalog', 'pack', 'library', 'layer', 'agents.md'],
  },
  {
    id: 'context-catalog.add',
    section: 'context-catalog',
    label: 'Add context',
    description: 'Stub a user pack (pack.toml + layer.md) and author it with the AI File Editor',
    keywords: ['add', 'create', 'pack', 'author', 'new context'],
  },
]

type CatalogFilter = 'all' | 'live' | 'static' | 'user'

const FILTERS: { id: CatalogFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'live', label: 'Live' },
  { id: 'static', label: 'Static' },
  { id: 'user', label: 'User' },
]

function hasTag(p: ContextCatalogEntry, tag: string): boolean {
  return (p.tags ?? []).some((t) => t.toLowerCase() === tag.toLowerCase())
}

function isUserPack(p: ContextCatalogEntry): boolean {
  return p.source.startsWith('catalog:user:') || p.id.startsWith('user:')
}

function isLiveRoster(p: ContextCatalogEntry): boolean {
  return (
    p.kind === 'live' ||
    hasTag(p, 'live') ||
    p.source.endsWith('-roster') ||
    p.id.endsWith(':roster')
  )
}

function catalogOrigin(p: ContextCatalogEntry): string {
  if (isUserPack(p)) return `user (${p.source})`
  if (p.source.startsWith('catalog:wiki')) return p.source
  return p.source
}

function matchesFilter(p: ContextCatalogEntry, filter: CatalogFilter): boolean {
  switch (filter) {
    case 'all':
      return true
    case 'live':
      return isLiveRoster(p)
    case 'static':
      return p.kind === 'static' || p.kind === 'path'
    case 'user':
      return isUserPack(p)
    default:
      return true
  }
}

export function ContextCatalogSection(): React.JSX.Element {
  const [catalog, setCatalog] = useState<ContextCatalogEntry[]>(DEFAULT_CATALOG_ENTRIES)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [filter, setFilter] = useState<CatalogFilter>('all')
  const [adding, setAdding] = useState(false)
  const [slugDraft, setSlugDraft] = useState('')
  const [slugOpen, setSlugOpen] = useState(false)
  const [editor, setEditor] = useState<{ dir: string; label: string } | null>(null)
  const confirm = useConfirmDialogStore((s) => s.confirm)

  const refresh = useCallback(async () => {
    try {
      const api = await fetchContextCatalog()
      setCatalog(mergeContextCatalog(api))
      setError(null)
    } catch (err) {
      setError(contextErrorMessage(err, 'Failed to load catalog'))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    let list = catalog.filter((p) => matchesFilter(p, filter))
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
    list.sort((a, b) => a.label.localeCompare(b.label, undefined, { sensitivity: 'base' }))
    return list
  }, [catalog, query, filter])

  const filterCounts = useMemo(() => {
    const counts: Record<CatalogFilter, number> = {
      all: catalog.length,
      live: 0,
      static: 0,
      user: 0,
    }
    for (const p of catalog) {
      if (isLiveRoster(p)) counts.live += 1
      if (p.kind === 'static' || p.kind === 'path') counts.static += 1
      if (isUserPack(p)) counts.user += 1
    }
    return counts
  }, [catalog])

  const handleCreate = useCallback(async () => {
    const raw = slugDraft.trim()
    if (!raw) return
    setAdding(true)
    setError(null)
    try {
      const id = raw.startsWith('user:') ? raw : `user:${raw}`
      const created = await createContextCatalogPack({ id })
      setSlugOpen(false)
      setSlugDraft('')
      setEditor({ dir: created.dir, label: created.entry.label || id })
    } catch (err) {
      setError(contextErrorMessage(err, 'Could not create pack'))
    } finally {
      setAdding(false)
    }
  }, [slugDraft])

  const handleDelete = useCallback(
    async (p: ContextCatalogEntry) => {
      const ok = await confirm({
        title: 'Delete catalog pack',
        message: `Remove ${p.id} from the host library? Workspace stacks that already added it are left alone.`,
        confirmLabel: 'Delete',
        destructive: true,
      })
      if (!ok) return
      try {
        await deleteContextCatalogPack(p.id)
        await refresh()
      } catch (err) {
        setError(contextErrorMessage(err, 'Could not delete pack'))
      }
    },
    [confirm, refresh],
  )

  if (editor) {
    return (
      <div className="absolute inset-0 overflow-hidden bg-[var(--color-bg)]">
        <ContextCatalogCreator
          packDir={editor.dir}
          title={editor.label}
          onClose={() => {
            setEditor(null)
            void refresh()
          }}
        />
      </div>
    )
  }

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex-shrink-0 px-6 pt-5 pb-3 border-b border-[var(--color-border)] space-y-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0" data-settings-id="context-catalog.library">
            <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Context Catalog</h2>
            <p className="text-[11px] text-[var(--color-text-muted)] mt-1 leading-snug">
              Host library of context packs. Creating a pack does not stack it on a workspace —
              apply later via Browse catalog or <span className="font-mono">k2 agent context add</span>.
            </p>
          </div>
          <button
            type="button"
            data-settings-id="context-catalog.add"
            onClick={() => setSlugOpen(true)}
            className="flex-shrink-0 px-3 py-1.5 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 hover:bg-[var(--color-accent)]/20 no-drag cursor-pointer"
          >
            Add context
          </button>
        </div>

        {slugOpen && (
          <form
            className="flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              void handleCreate()
            }}
          >
            <input
              autoFocus
              value={slugDraft}
              onChange={(e) => setSlugDraft(e.target.value)}
              placeholder="slug (becomes user:&lt;slug&gt;)"
              aria-label="Pack slug"
              className="flex-1 px-3 py-2 text-[12px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]/50 no-drag"
            />
            <button
              type="submit"
              disabled={adding || !slugDraft.trim()}
              className="px-3 py-1.5 text-[11px] font-medium text-[var(--color-on-accent)] bg-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-50"
            >
              {adding ? 'Creating…' : 'Create'}
            </button>
            <button
              type="button"
              onClick={() => setSlugOpen(false)}
              className="px-2 py-1.5 text-[11px] text-[var(--color-text-secondary)] no-drag cursor-pointer"
            >
              Cancel
            </button>
          </form>
        )}

        <input
          type="search"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder="Search catalog…"
          aria-label="Search context catalog"
          className="w-full px-3 py-2 text-[12px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]/50 no-drag"
        />

        <div className="flex flex-wrap items-center gap-1.5" role="tablist" aria-label="Catalog filters">
          {FILTERS.map((f) => {
            const active = filter === f.id
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
                <span className="ml-1 tabular-nums opacity-70">{filterCounts[f.id]}</span>
              </button>
            )
          })}
        </div>

        {error ? (
          <p className="text-[11px] text-[var(--color-status-error-soft)]">{error}</p>
        ) : null}
      </div>

      <div className="flex-1 min-h-0 overflow-y-auto">
        {loading ? (
          <div className="px-6 py-10 text-[12px] text-[var(--color-text-muted)]">Loading catalog…</div>
        ) : filtered.length === 0 ? (
          <div className="px-6 py-10 text-[12px] text-[var(--color-text-muted)] text-center">
            {query.trim() ? `No catalog items match “${query.trim()}”.` : 'No items in this filter.'}
          </div>
        ) : (
          <ul className="divide-y divide-[var(--color-border)]">
            {filtered.map((p) => {
              const live = isLiveRoster(p)
              const user = isUserPack(p)
              const metaBits = [
                p.kind ?? null,
                catalogOrigin(p),
                p.author ?? null,
                p.version ? `v${p.version}` : null,
              ].filter(Boolean) as string[]
              return (
                <li key={p.id} className="px-6 py-3.5 flex items-start gap-4">
                  <div className="flex-1 min-w-0">
                    <div className="flex items-baseline gap-2 min-w-0 flex-wrap">
                      <span className="text-[13px] font-medium text-[var(--color-text-primary)]">
                        {p.label}
                      </span>
                      {p.recommended ? (
                        <span className="px-1.5 py-0.5 text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/12 border border-[var(--color-accent)]/30">
                          Recommended
                        </span>
                      ) : null}
                      {live ? (
                        <span className="px-1.5 py-0.5 text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)]">
                          Live · view only
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
                    {(p.tags ?? []).length > 0 ? (
                      <div className="flex flex-wrap gap-1 mt-1.5">
                        {(p.tags ?? []).map((tag) => (
                          <span
                            key={tag}
                            className="px-1.5 py-0.5 text-[9px] text-[var(--color-text-muted)] border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/40"
                          >
                            {tag}
                          </span>
                        ))}
                      </div>
                    ) : null}
                    <div className="text-[10px] font-mono text-[var(--color-text-muted)] mt-1.5 truncate" title={p.id}>
                      {p.id}
                    </div>
                  </div>
                  <div className="flex-shrink-0 pt-0.5 flex items-center gap-1.5">
                    {user ? (
                      <>
                        <button
                          type="button"
                          disabled={!p.dir}
                          onClick={() => {
                            if (!p.dir) return
                            setEditor({ dir: p.dir, label: p.label })
                          }}
                          className="px-2.5 py-1 text-[11px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 border border-[var(--color-accent)]/30 no-drag cursor-pointer disabled:opacity-40"
                        >
                          Edit
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleDelete(p)}
                          className="px-2.5 py-1 text-[11px] text-[var(--color-text-secondary)] border border-[var(--color-border)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer"
                        >
                          Delete
                        </button>
                      </>
                    ) : (
                      <span className="text-[10px] text-[var(--color-text-muted)]">
                        {live ? 'View only' : 'Built-in'}
                      </span>
                    )}
                  </div>
                </li>
              )
            })}
          </ul>
        )}
      </div>
    </div>
  )
}
