// Settings → Access → API Keys: the keys other apps and services use to
// reach this server through K2's API (`/cli/api-keys/*`, Owner only). One
// column at the Settings content width, like Admin Access:
//
//   header        what a key is for, and the Access Audit link
//   API off       one banner when the public API is off, with a way there
//   Keys          one row per key: its name, what it can reach, when it was
//                 made; a "…" menu with App access (hire keys), Turn off /
//                 Turn back on, and Revoke (asks first; permanent)
//   Revoked       keys that no longer work, kept so you can see who had access
//
// Create API key opens the shared dialog (ApiKeyDialogs.tsx); the new key's
// secret shows once, in that dialog. A change says what it did at the top of
// the Keys card; a failed one says what failed on its row.

import React, { useCallback, useEffect, useMemo, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { daemonErrorText } from '@/lib/daemon-error'
import { httpStatusOf } from '@/lib/daemon-http-error'
import { useProjectsStore } from '@/stores/projects'
import { useSettingsStore } from '@/stores/settings'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import type { SettingEntry } from '../searchManifest'
import {
  capsSummary,
  fetchCeilingOptions,
  formatGrant,
  hireWords,
  keyDates,
  keyGrantsWorkspace,
  keyName,
  keyState,
  reachWords,
  ceilingSummary,
  workspaceGrantSlug,
  type ApiKeyRow,
  type CeilingOptions,
} from './api-keys-api'
import { primaryScope } from '@/kessel/server-scope'
import { Button, Callout, OverflowMenu, Spinner, Tag, type OverflowMenuItem } from '@/components/ui'
import { isAirgap } from '@/lib/airgap'
import { SETTING_LIST_ROW_HOVER, SETTING_ROW_HINT, SettingsGroup, SettingsLoadState } from '../controls/SettingControls'
import { AccessAuditLink } from '../shared/AccessAuditLink'
import { ConfirmSheet } from './AccessDialogs'
import { AppAccessDialog, CreateKeyDialog, type CreatedKey } from './ApiKeyDialogs'
import { useActiveHostRole } from './useActiveHostRole'

export { hireConfirmCopy } from './ApiKeyDialogs'

export const API_TOKENS_MANIFEST: SettingEntry[] = [
  {
    id: 'api-tokens.inventory',
    section: 'api-tokens',
    label: 'API Keys',
    description: 'Create, turn off and revoke the keys your apps and services use to reach this server',
    keywords: [
      'api',
      'api key',
      'k2sk',
      'token',
      'host-sessions',
      'revoke',
      'disable',
      'public api',
      'v1',
      'hire',
      'hire agents',
      'database',
    ],
  },
]

/** Where the Public API switch lives: Network → Tunnel's policies, or
 *  Connected Servers' under air-gap (Tunnel is hidden then;
 *  `K2ConnectSettingsShell`). */
export function publicApiSection(airgap: boolean = isAirgap()): 'k2-connect' | 'connections' {
  return airgap ? 'connections' : 'k2-connect'
}

/** Open the page with the Public API switch and bring the switch into view,
 *  the way Settings search does. */
export function openPublicApiSetting(): void {
  useSettingsStore.getState().setSection(publicApiSection())
  let tries = 0
  const reveal = (): void => {
    const el = document.querySelector<HTMLElement>('[data-settings-id="k2-connect.public-api"]')
    if (!el) {
      if (++tries < 30) requestAnimationFrame(reveal)
      return
    }
    el.scrollIntoView?.({ behavior: 'smooth', block: 'center' })
    el.classList.add('settings-search-pulse')
    window.setTimeout(() => el.classList.remove('settings-search-pulse'), 1500)
  }
  requestAnimationFrame(reveal)
}

type ListResponse = { keys?: ApiKeyRow[] }
type Load = { kind: 'loading' } | { kind: 'error'; detail: string } | { kind: 'not-owner' } | { kind: 'ready' }
type Dialog = { kind: 'create' } | { kind: 'revoke'; key: ApiKeyRow } | { kind: 'app-access'; key: ApiKeyRow } | null
type Action = 'disable' | 'enable' | 'revoke'

/** A row change that failed: what, what to do now, the server's reason. */
interface RowFail {
  what: string
  next: string
  detail: string
}

function rowFail(action: Action, name: string, detail: string): RowFail {
  if (action === 'revoke') return { what: `Couldn’t revoke ${name}.`, next: 'It still works. Try again in a moment.', detail }
  if (action === 'disable') return { what: `Couldn’t turn off ${name}.`, next: 'It still works. Try again in a moment.', detail }
  return { what: `Couldn’t turn ${name} back on.`, next: 'It’s still off. Try again in a moment.', detail }
}

function doneNote(action: Action, name: string): string {
  if (action === 'revoke') return `${name} is revoked. Apps using it can’t reach this server any more.`
  if (action === 'disable') return `${name} is off. Apps using it stop working until you turn it back on.`
  return `${name} is on again. Apps using it work as before.`
}

/** A key's state, when it isn't simply working. */
function StateTag({ k }: { k: ApiKeyRow }): React.JSX.Element | null {
  const state = keyState(k)
  if (state === 'revoked') return <Tag tone="muted">Revoked</Tag>
  if (state === 'disabled') return <Tag tone="warn">Off</Tag>
  return null
}

/** A thrown error's status, when the server answered one. */
const statusOf = (e: unknown): number | null => httpStatusOf(e)

export function ApiTokensSection(): React.JSX.Element {
  const projects = useProjectsStore((s) => s.projects)
  const apiEnabled = useSettingsStore((s) => s.apiEnabled)
  const { role } = useActiveHostRole()
  // Keys are the Owner's (route_policy: Owner). A role the server reported
  // below Owner never asks; an unread role tries and the server decides.
  const roleBelowOwner = role.kind === 'known' && role.role !== 'owner'
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [load, setLoad] = useState<Load>({ kind: 'loading' })
  const [ceilingOpts, setCeilingOpts] = useState<CeilingOptions>({ apps: [], rooms: [] })
  const [ceilingOptsError, setCeilingOptsError] = useState<string | null>(null)
  const [dialog, setDialog] = useState<Dialog>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  const [fails, setFails] = useState<Record<string, RowFail>>({})

  const refresh = useCallback(async (): Promise<void> => {
    try {
      const d = await daemonCliGet<ListResponse>(primaryScope(), 'api-keys/list')
      setKeys(Array.isArray(d.keys) ? d.keys : [])
      setLoad({ kind: 'ready' })
    } catch (e) {
      const status = statusOf(e)
      setLoad((prev) =>
        status === 401 || status === 403
          ? { kind: 'not-owner' }
          : prev.kind === 'ready'
            ? prev
            : { kind: 'error', detail: daemonErrorText(e) },
      )
      return
    }
    try {
      setCeilingOpts(await fetchCeilingOptions())
      setCeilingOptsError(null)
    } catch (e) {
      setCeilingOptsError(daemonErrorText(e))
    }
  }, [])

  useEffect(() => {
    if (roleBelowOwner) return
    void refresh()
  }, [refresh, roleBelowOwner])

  const retry = (): void => {
    setLoad({ kind: 'loading' })
    void refresh()
  }

  const act = async (k: ApiKeyRow, action: Action): Promise<void> => {
    const name = keyName(k)
    setBusyId(k.id)
    setNotice(null)
    setFails(({ [k.id]: _gone, ...rest }) => rest)
    try {
      await daemonCliPost(primaryScope(), `api-keys/${action}`, { id: k.id })
      setNotice(doneNote(action, name))
      await refresh()
    } catch (e) {
      setFails((prev) => ({ ...prev, [k.id]: rowFail(action, name, daemonErrorText(e)) }))
    } finally {
      setBusyId(null)
    }
  }

  const live = useMemo(() => keys.filter((k) => keyState(k) !== 'revoked'), [keys])
  const revoked = useMemo(() => keys.filter((k) => keyState(k) === 'revoked'), [keys])
  const workspaces = useMemo(
    () => projects.map((p) => ({ slug: workspaceGrantSlug(p), label: p.name || p.path })),
    [projects],
  )

  const notOwner = roleBelowOwner || load.kind === 'not-owner'
  const createButton = (
    <Button
      variant="accent"
      onClick={() => {
        setNotice(null)
        setDialog({ kind: 'create' })
      }}
    >
      Create API key
    </Button>
  )

  return (
    <div className="w-full max-w-[52rem] space-y-6 pb-6">
      <header className="min-w-0" data-settings-id="api-tokens.inventory">
        <h2 className="type-title text-[var(--color-text-primary)]">API Keys</h2>
        <p className="type-hint text-[var(--color-text-secondary)] mt-1 leading-relaxed max-w-[75ch]">
          Keys let your own apps and services use this server: run agent sessions, send messages, use sandboxes and
          databases, or hire agents. Each key reaches only the workspaces and actions you choose.
        </p>
        <AccessAuditLink className="mt-1" />
      </header>

      {!apiEnabled && !notOwner ? (
        <Callout tone="warn" role="status" className="type-hint" data-testid="api-surface-off">
          <div className="flex items-center justify-between gap-4">
            <div className="min-w-0 space-y-0.5">
              <p className="font-medium">The public API is off, so no key works yet.</p>
              <p className="text-[var(--color-text-secondary)]">
                Apps can’t use their keys until it’s on. Your keys stay as they are.
              </p>
            </div>
            <Button className="flex-shrink-0" onClick={openPublicApiSetting}>
              Open the Public API setting
            </Button>
          </div>
        </Callout>
      ) : null}

      {notOwner ? (
        <SettingsLoadState kind="empty" title="Only an Owner can manage API keys" testId="api-keys-not-owner">
          A key lets another app reach this server, so only its Owners can see, create or revoke keys. If an app you use
          needs one, ask an Owner.
        </SettingsLoadState>
      ) : (
        <SettingsGroup
          title="Keys"
          hint="Turn a key off to pause it for a while. Revoke it to stop it for good."
          action={load.kind === 'ready' && live.length > 0 ? createButton : undefined}
          card
          testId="api-keys"
          state={
            load.kind === 'loading' ? (
              <SettingsLoadState kind="loading" title="Loading your API keys…" testId="api-keys-loading">
                Reading the keys on this server.
              </SettingsLoadState>
            ) : load.kind === 'error' ? (
              <SettingsLoadState
                kind="error"
                title="Couldn’t load your API keys"
                detail={load.detail}
                action={<Button onClick={retry}>Try again</Button>}
                testId="api-keys-error"
              >
                Your keys keep working as they were. Try again in a moment.
              </SettingsLoadState>
            ) : live.length === 0 && !notice ? (
              <SettingsLoadState kind="empty" title="No API keys yet" action={createButton} testId="api-keys-empty">
                Create a key when an app or service needs to use this server, like a website chat or a build script.
              </SettingsLoadState>
            ) : undefined
          }
        >
          {notice ? (
            <div role="status" className="px-4 py-2.5 flex items-center justify-between gap-4" data-testid="api-keys-notice">
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
          {live.map((k) => (
            <KeyRow
              key={k.id}
              k={k}
              busy={busyId === k.id}
              fail={fails[k.id] ?? null}
              ceilingOpts={ceilingOpts}
              onAction={(a) => (a === 'revoke' ? setDialog({ kind: 'revoke', key: k }) : void act(k, a))}
              onAppAccess={() => setDialog({ kind: 'app-access', key: k })}
            />
          ))}
          {live.length === 0 ? (
            <div className="px-4 py-3 flex items-center justify-between gap-6">
              <p className={SETTING_ROW_HINT}>No working keys left. Create one when an app needs to reach this server.</p>
              {createButton}
            </div>
          ) : null}
        </SettingsGroup>
      )}

      {!notOwner && load.kind === 'ready' && revoked.length > 0 ? (
        <SettingsGroup
          title="Revoked"
          hint="These keys no longer work. They stay listed so you can see what had access."
          card
          testId="api-keys-revoked"
        >
          {revoked.map((k) => (
            <KeyRow key={k.id} k={k} busy={false} fail={null} ceilingOpts={ceilingOpts} />
          ))}
        </SettingsGroup>
      ) : null}

      {dialog?.kind === 'create' ? (
        <CreateKeyDialog
          workspaces={workspaces}
          ceilingOpts={ceilingOpts}
          ceilingOptsError={ceilingOptsError}
          apiOff={!apiEnabled}
          onCreated={() => void refresh()}
          onClose={(created: CreatedKey | null) => {
            setDialog(null)
            if (created) setNotice(`${created.name} is ready. Its key won’t be shown again.`)
          }}
        />
      ) : dialog?.kind === 'revoke' ? (
        <ConfirmSheet
          title={`Revoke ${keyName(dialog.key)}?`}
          message="Apps using this key stop working right away, for good. A revoked key can’t be turned back on: to let an app in again, create a new key."
          confirmLabel="Revoke key"
          destructive
          testId="revoke-confirm-dialog"
          onCancel={() => setDialog(null)}
          onConfirm={() => {
            const k = dialog.key
            setDialog(null)
            void act(k, 'revoke')
          }}
        />
      ) : dialog?.kind === 'app-access' ? (
        <AppAccessDialog
          keyRow={dialog.key}
          ceilingOpts={ceilingOpts}
          ceilingOptsError={ceilingOptsError}
          onClose={() => setDialog(null)}
          onSaved={() => {
            setNotice(`App access for ${keyName(dialog.key)} is saved.`)
            setDialog(null)
            void refresh()
          }}
        />
      ) : null}
    </div>
  )
}

/** The width of a row's "…" menu: a revoked row keeps it free so every
 *  row's text ends at the same edge. */
const MENU_SLOT = 'w-8 flex-shrink-0'

/** One key: its name and state, what it can reach, when it was made, and
 *  (while it works) the "…" menu. Errors go under the text, so nothing
 *  moves when one shows. */
function KeyRow({
  k,
  busy,
  fail,
  ceilingOpts,
  onAction,
  onAppAccess,
}: {
  k: ApiKeyRow
  busy: boolean
  fail: RowFail | null
  ceilingOpts: CeilingOptions
  onAction?: (a: Action) => void
  onAppAccess?: () => void
}): React.JSX.Element {
  const state = keyState(k)
  const name = keyName(k)
  const status = k.scope === 'status'
  const hires = !status && !!k.capabilities?.hire && !!k.hireLimits
  const dates = keyDates(k)
  const reach = reachWords(k, hires)

  const items: OverflowMenuItem[] = []
  if (onAction && state !== 'revoked') {
    if (hires && onAppAccess) {
      items.push({ id: 'app-access', label: 'App access…', hint: 'Which App logins its hired agents may get.', onSelect: onAppAccess })
    }
    // A status key reads this server's status only: it is revoked, not paused.
    if (!status) {
      items.push(
        state === 'active'
          ? { id: 'disable', label: 'Turn off', hint: 'Apps using it stop working until you turn it back on.', onSelect: () => onAction('disable') }
          : { id: 'enable', label: 'Turn back on', hint: 'Apps using it work again right away.', onSelect: () => onAction('enable') },
      )
    }
    items.push({ id: 'revoke', label: 'Revoke…', hint: 'Stops it for good.', danger: true, group: items.length > 0, onSelect: () => onAction('revoke') })
  }

  return (
    <div
      className={`px-4 py-3 ${state === 'revoked' ? '' : SETTING_LIST_ROW_HOVER}`}
      data-testid={`api-key-row-${k.id}`}
      title={`Key ID: ${k.id}`}
    >
      <div className="flex items-start justify-between gap-6">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 min-w-0 flex-wrap">
            <span className={`type-name ${state === 'revoked' ? 'text-[var(--color-text-secondary)]' : 'text-[var(--color-text-primary)]'}`}>{name}</span>
            {status ? <Tag tone="neutral">Status only</Tag> : null}
            <StateTag k={k} />
          </div>
          {reach ? (
            <p className={SETTING_ROW_HINT} data-testid={`api-key-reach-${k.id}`}>
              {reach}
            </p>
          ) : null}
          {hires ? (
            <p className={SETTING_ROW_HINT} data-testid={`hire-limits-${k.id}`}>
              {hireWords(k.hireLimits)}
            </p>
          ) : null}
          {hires ? (
            <p className={SETTING_ROW_HINT} data-testid={`hire-ceiling-${k.id}`}>
              App access: {ceilingSummary(k.hireLimits?.apps, ceilingOpts)}
            </p>
          ) : null}
          {dates ? <p className={SETTING_ROW_HINT}>{dates}</p> : null}
          {busy ? (
            <p className={`${SETTING_ROW_HINT} flex items-center gap-1.5`} role="status">
              <Spinner />
              Saving…
            </p>
          ) : null}
          {fail ? (
            <div role="alert" data-save-error="" className={`${SETTING_ROW_HINT} mt-1`}>
              <p className="text-[var(--color-status-error-soft)]">
                {fail.what} {fail.next}
              </p>
              <p>The server said: {fail.detail.trim().replace(/[.\s]+$/, '')}.</p>
            </div>
          ) : null}
        </div>
        {items.length > 0 ? (
          <OverflowMenu label={`More actions for ${name}`} items={items} />
        ) : (
          <span aria-hidden="true" className={MENU_SLOT} />
        )}
      </div>
    </div>
  )
}

/** Workspace settings → API tab: keys that grant this workspace only. */
export function WorkspaceApiKeysPanel({
  workspaceSlug,
}: {
  workspaceSlug: string
}): React.JSX.Element {
  const openGlobal = () => useSettingsStore.getState().setSection('api-tokens')
  const [keys, setKeys] = useState<ApiKeyRow[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const d = await daemonCliGet<ListResponse>(primaryScope(), 'api-keys/list')
      const all = Array.isArray(d.keys) ? d.keys : []
      setKeys(all.filter((k) => keyGrantsWorkspace(k, workspaceSlug)))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setKeys([])
    } finally {
      setLoading(false)
    }
  }, [workspaceSlug])

  useEffect(() => {
    setLoading(true)
    void refresh()
  }, [refresh])

  const act = useCallback(
    async (id: string, action: Action) => {
      if (action === 'revoke') {
        const ok = await useConfirmDialogStore.getState().confirm({
          title: 'Revoke this API key?',
          message: 'Anything using it stops working at once. A revoked key can’t be turned back on; you’d create a new one in Settings → API Keys.',
          confirmLabel: 'Revoke key',
          destructive: true,
        })
        if (!ok) return
      }
      setBusyId(id)
      try {
        await daemonCliPost(primaryScope(), `api-keys/${action}`, { id })
        await refresh()
      } catch (e) {
        setError(e instanceof Error ? e.message : String(e))
      } finally {
        setBusyId(null)
      }
    },
    [refresh],
  )

  return (
    <SettingsGroup
      title="API keys"
      card
      testId="workspace-api-keys"
      hint="Keys that let other apps call this workspace through the K2 API. A key reaches it when it’s given this workspace or every workspace."
      action={<Button variant="quiet" onClick={openGlobal}>Manage all keys</Button>}
    >
      {error && (
        <p role="alert" className="px-4 py-3 type-hint text-[var(--color-status-error-soft)]">
          Couldn&rsquo;t load or change the keys: {error.replace(/[.\s]+$/, '')}. Try again, or open Settings &rarr; API Keys.
        </p>
      )}
      {loading ? (
        <div className="px-4 py-3 flex items-center gap-2 type-hint text-[var(--color-text-secondary)]" data-testid="workspace-api-keys-loading">
          <Spinner />
          Loading keys&hellip;
        </div>
      ) : keys.length === 0 ? (
        <div className="px-4 py-3 flex items-start justify-between gap-6" data-testid="workspace-api-keys-empty">
          <div className="flex-1 min-w-0">
            <div className="type-row text-[var(--color-text-primary)]">No keys reach this workspace yet</div>
            <p className="type-hint text-[var(--color-text-secondary)] mt-0.5 leading-relaxed max-w-[75ch]">
              A key you give this workspace, or every workspace, shows here. Create a key opens API Keys,
              where you choose what it can reach.
            </p>
          </div>
          <Button variant="quiet" className="flex-shrink-0" onClick={openGlobal}>Create a key</Button>
        </div>
      ) : (
        keys.map((k) => {
          const state = keyState(k)
          const busy = busyId === k.id
          return (
            <div key={k.id} className="px-4 py-3 flex items-start justify-between gap-6">
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="flex items-center gap-2">
                  <span className="type-name text-[var(--color-text-primary)]">{keyName(k)}</span>
                  <StateTag k={k} />
                </div>
                <p className="type-hint font-mono text-[var(--color-text-secondary)] break-all">{k.id}</p>
                <p className="type-hint text-[var(--color-text-secondary)]">
                  {formatGrant(k.allowedWorkspaces)} · {capsSummary(k.capabilities)}
                </p>
              </div>
              <div className="flex items-center gap-2 flex-shrink-0">
                {state === 'active' && (
                  <Button variant="quiet" disabled={busy} onClick={() => void act(k.id, 'disable')}>
                    Turn off
                  </Button>
                )}
                {state === 'disabled' && (
                  <Button variant="quiet" disabled={busy} onClick={() => void act(k.id, 'enable')}>
                    Turn on
                  </Button>
                )}
                {state !== 'revoked' && (
                  <Button variant="danger-muted" disabled={busy} onClick={() => void act(k.id, 'revoke')}>
                    Revoke
                  </Button>
                )}
              </div>
            </div>
          )
        })
      )}
    </SettingsGroup>
  )
}
