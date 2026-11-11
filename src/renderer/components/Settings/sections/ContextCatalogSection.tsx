// Settings → Context Catalog: this server's library of context packs, the
// standing instructions a workspace's agents always keep in mind once the
// pack is added to that workspace (Workspaces / Agents → Context). Two
// groups: the packs people here wrote (New pack, Edit, Delete) and the ones
// K2 ships. Creating a pack never adds it to a workspace. Words and rules:
// context-catalog-copy.ts; dialogs: ContextCatalogDialogs.tsx.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import {
  contextErrorMessage,
  fetchContextCatalog,
  mergeContextCatalog,
  type ContextCatalogEntry,
  type CreatedCatalogPack,
} from '@/lib/context-stack'
import { Button, OverflowMenu, SearchField, Tag } from '@/components/ui'
import { useSettingsStore } from '@/stores/settings'
import type { SettingEntry } from '../searchManifest'
import { ContextCatalogCreator } from '../ContextCatalogCreator'
import { SETTING_ROW_HINT, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { DeletePackDialog, NewPackDialog } from './ContextCatalogDialogs'
import { isLivePack, isUserPack, packMatches, packSummary } from './context-catalog-copy'
import { useActiveHostRole } from './useActiveHostRole'

export const CONTEXT_CATALOG_MANIFEST: SettingEntry[] = [
  {
    id: 'context-catalog.library',
    section: 'context-catalog',
    label: 'Context Catalog',
    description: 'Ready-made instructions you can add to any workspace’s agents: the ones K2 ships and your own.',
    keywords: ['context', 'catalog', 'pack', 'library', 'layer', 'agents.md', 'instructions', 'standing orders'],
  },
  {
    id: 'context-catalog.add',
    section: 'context-catalog',
    label: 'New context pack',
    description: 'Write your own instructions once, with an agent’s help, then add them to any workspace.',
    keywords: ['add', 'create', 'new', 'pack', 'write', 'author', 'new context'],
  },
]

/** Your packs get a search field past this many. */
const SEARCH_FROM = 7

type Load = { kind: 'loading' } | { kind: 'error'; detail: string } | { kind: 'ready'; list: ContextCatalogEntry[] }
type Dialog = { kind: 'new' } | { kind: 'delete'; pack: ContextCatalogEntry } | null

const byName = (a: ContextCatalogEntry, b: ContextCatalogEntry): number =>
  a.label.localeCompare(b.label, undefined, { sensitivity: 'base' })

export function ContextCatalogSection(): React.JSX.Element {
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [query, setQuery] = useState('')
  const [dialog, setDialog] = useState<Dialog>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [editor, setEditor] = useState<{ dir: string; label: string } | null>(null)
  const { role } = useActiveHostRole()
  // Packs are the server's: an Owner or Admin adds, changes and deletes
  // them. A role not read yet (or this computer) is allowed; the server
  // still decides.
  const readOnly = role.kind === 'known' && role.role === 'member'

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const api = await fetchContextCatalog()
      setLoad({ kind: 'ready', list: mergeContextCatalog(api) })
    } catch (err) {
      setLoad((prev) =>
        prev.kind === 'ready' ? prev : { kind: 'error', detail: contextErrorMessage(err, 'The server didn’t answer') },
      )
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const list = load.kind === 'ready' ? load.list : []
  const yours = useMemo(() => list.filter(isUserPack).sort(byName), [list])
  const shownYours = useMemo(() => yours.filter((p) => packMatches(p, query)), [yours, query])
  const builtIn = useMemo(() => list.filter((p) => !isUserPack(p)), [list])

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

  const newButton = (
    <Button
      variant="accent"
      data-settings-id="context-catalog.add"
      disabled={readOnly}
      title={readOnly ? 'Only an Owner or Admin of this server can add packs.' : undefined}
      onClick={() => {
        setNotice(null)
        setDialog({ kind: 'new' })
      }}
    >
      New pack
    </Button>
  )

  const retry = (): void => {
    setLoad({ kind: 'loading' })
    void refresh()
  }

  return (
    <div className="h-full min-h-0 overflow-y-auto p-6 [scrollbar-gutter:stable]">
      <div className="w-full max-w-[52rem] space-y-6 pb-6">
        <header className="min-w-0" data-settings-id="context-catalog.library">
          <h2 className="type-title text-[var(--color-text-primary)]">Context Catalog</h2>
          <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
            Ready-made instructions a workspace’s agents always keep in mind, like how to keep the wiki tidy. A pack does
            nothing until you add it to a workspace.
          </p>
          <p className="type-hint text-[var(--color-text-secondary)] mt-1">
            Add a pack to a workspace in{' '}
            <button
              type="button"
              className="k2-focus underline hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
              onClick={() => useSettingsStore.getState().setSection('projects')}
            >
              Workspaces / Agents
            </button>{' '}
            → the workspace → Context.
          </p>
        </header>

        <SettingsGroup
          title="Your packs"
          hint={
            readOnly
              ? 'Packs people on this server wrote. Only an Owner or Admin can add, change or delete them.'
              : 'Packs you and others on this server wrote. Edit one to change what it says.'
          }
          action={load.kind === 'ready' ? newButton : undefined}
          card
          testId="context-catalog-yours"
          state={
            load.kind === 'loading' ? (
              <SettingsLoadState kind="loading" title="Loading the catalog…" testId="context-catalog-loading">
                Reading the packs on this server.
              </SettingsLoadState>
            ) : load.kind === 'error' ? (
              <SettingsLoadState
                kind="error"
                title="Couldn’t load the catalog"
                detail={load.detail}
                action={<Button onClick={retry}>Try again</Button>}
                testId="context-catalog-error"
              >
                Try again in a moment. Nothing in the catalog or in your workspaces has changed.
              </SettingsLoadState>
            ) : yours.length === 0 && !notice ? (
              <SettingsLoadState kind="empty" title="No packs of your own yet" testId="context-catalog-empty">
                {readOnly
                  ? 'An Owner or Admin can write one here.'
                  : 'Write instructions once, then add them to any workspace. Start with New pack.'}
              </SettingsLoadState>
            ) : undefined
          }
        >
          {notice ? (
            <div role="status" className="px-4 py-2.5 flex items-center justify-between gap-4" data-testid="context-catalog-notice">
              <span className="inline-flex items-center gap-1.5 type-hint text-[var(--color-status-ok-soft)]">
                <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
                  <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
                </svg>
                {notice}
              </span>
              <Button variant="quiet" onClick={() => setNotice(null)}>
                Dismiss
              </Button>
            </div>
          ) : null}
          {yours.length >= SEARCH_FROM ? (
            <div className="px-4 py-2.5">
              <SearchField
                className="w-full"
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder="Search your packs"
                aria-label="Search your packs"
              />
            </div>
          ) : null}
          {shownYours.map((p) => (
            <PackRow
              key={p.id}
              pack={p}
              readOnly={readOnly}
              onEdit={() => {
                if (!p.dir) return
                setNotice(null)
                setEditor({ dir: p.dir, label: p.label })
              }}
              onDelete={() => {
                setNotice(null)
                setDialog({ kind: 'delete', pack: p })
              }}
            />
          ))}
          {yours.length > 0 && shownYours.length === 0 ? (
            <p className={`px-4 py-3 ${SETTING_ROW_HINT}`}>No packs match “{query.trim()}”.</p>
          ) : null}
        </SettingsGroup>

        {load.kind === 'ready' && builtIn.length > 0 ? (
          <SettingsGroup
            title="Built into K2"
            hint="These come with K2 and stay up to date with it. Recommended ones suit most workspaces."
            card
            testId="context-catalog-builtin"
          >
            {builtIn.map((p) => (
              <PackRow key={p.id} pack={p} readOnly />
            ))}
          </SettingsGroup>
        ) : null}
      </div>

      {dialog?.kind === 'new' ? (
        <NewPackDialog
          taken={list.map((p) => p.id)}
          onCreated={() => void refresh()}
          onClose={(created) => {
            setDialog(null)
            if (created) {
              const name = created.entry.label
              setNotice(`${name} is in the catalog. Edit it to write what it says, then add it to a workspace.`)
            }
          }}
          onWrite={(created: CreatedCatalogPack) => {
            setDialog(null)
            setEditor({ dir: created.dir, label: created.entry.label })
          }}
        />
      ) : null}
      {dialog?.kind === 'delete' ? (
        <DeletePackDialog
          pack={dialog.pack}
          onClose={() => setDialog(null)}
          onDeleted={(p) => {
            setDialog(null)
            setNotice(`${p.label} was deleted from the catalog. Workspaces that use it keep their own copy.`)
            setLoad((prev) => (prev.kind === 'ready' ? { kind: 'ready', list: prev.list.filter((x) => x.id !== p.id) } : prev))
            void refresh()
          }}
        />
      ) : null}
    </div>
  )
}

/** One pack: its name and short name, what it does, and (for a pack someone
 *  here wrote) Edit and a menu with Delete. */
function PackRow({
  pack: p,
  readOnly,
  onEdit,
  onDelete,
}: {
  pack: ContextCatalogEntry
  readOnly: boolean
  onEdit?: () => void
  onDelete?: () => void
}): React.JSX.Element {
  const mine = isUserPack(p)
  const summary = packSummary(p)
  const tags = mine ? (p.tags ?? []) : []
  const lock = readOnly ? 'Only an Owner or Admin of this server can change packs.' : undefined
  return (
    <div
      className={`px-4 py-3 ${mine ? 'transition-colors hover:bg-[var(--color-wash-1)]' : ''}`}
      data-testid={`pack-row-${p.id}`}
    >
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className="type-name text-[var(--color-text-primary)]">{p.label}</span>
            <span className="type-hint text-[var(--color-text-secondary)] font-mono truncate">{p.id}</span>
            {p.recommended ? <Tag tone="accent">Recommended</Tag> : null}
            {!mine && isLivePack(p) ? (
              <Tag tone="muted" title="K2 rebuilds this list from current data each time it updates the agent’s instructions.">
                Updates itself
              </Tag>
            ) : null}
          </div>
          {summary || mine ? (
            <p className={SETTING_ROW_HINT}>{summary ?? 'Nothing written yet. Edit it to say what it asks of agents.'}</p>
          ) : null}
          {tags.length > 0 ? <p className={SETTING_ROW_HINT}>Tagged {tags.join(', ')}</p> : null}
        </div>
        {mine && onEdit && onDelete ? (
          <div className="flex items-center gap-2 flex-shrink-0">
            <Button
              onClick={onEdit}
              disabled={readOnly || !p.dir}
              title={lock ?? (!p.dir ? 'This server didn’t say where the pack is kept, so it can’t be opened here.' : undefined)}
              aria-label={`Edit ${p.label}`}
            >
              Edit
            </Button>
            <OverflowMenu
              label={`More actions for ${p.label}`}
              items={[
                {
                  id: 'delete',
                  label: 'Delete pack…',
                  danger: true,
                  disabled: readOnly,
                  hint: lock,
                  onSelect: onDelete,
                },
              ]}
            />
          </div>
        ) : null}
      </div>
    </div>
  )
}
