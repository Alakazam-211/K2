// Settings → Cores → Multiplayer (Beta), prd-multiplayer-cores-settings-v1 §1.
//
// One switch per core, rendered from the daemon's `GET /cli/docs/cores`
// (never a hard-coded list), so this page and `k2 doc cores` always say the
// same thing. A click posts `settings/update {multiplayerCores: {<id>: on}}`
// to the focused server; the page refetches after its own write and on every
// `doc_cores_changed`. Turning a core off while files are open live asks
// first. A Member sees the switches disabled.

import React, { useCallback, useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { onDocCoresChanged } from '@/stores/session-events'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { errorMessage } from '@/lib/daemon-http-error'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import {
  failedWriteBackLine,
  fetchDocCores,
  openNowLine,
  setDocCore,
  turnOffMessage,
  turnOffTitle,
  type DocCore,
  type DocCores,
} from './multiplayer-cores-api'

export const MULTIPLAYER_INTRO =
  "People and agents work in the same file at the same time, with presence, history and everyone's own undo. " +
  'Each kind of file has a core. When a core is on, files of that kind open in the multiplayer view on this server, ' +
  "for everyone, with the workspace's agent beside them. When it's off, they open the usual way. " +
  "The server's owner or an Admin changes these."

export const MULTIPLAYER_MEMBER_LINE = "Only the server's owner or an Admin can change these."

export const MULTIPLAYER_BETA_LINE = 'Beta: keep your work in git while multiplayer is new.'

export const MULTIPLAYER_MANIFEST: SettingEntry[] = [
  {
    id: 'multiplayer.cores',
    section: 'multiplayer',
    label: 'Multiplayer',
    description: 'Turn each multiplayer core on or off for this server: Markdown, Text and code, Any other file.',
    keywords: ['live docs', 'live editing', 'multiplayer', 'Markdown', 'cores', 'co-editing'],
  },
]

const muted = 'text-[var(--color-text-muted)]'

export function MultiplayerSection({ scope = primaryScope() }: { scope?: ServerScope }): React.JSX.Element {
  const [data, setData] = useState<DocCores | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const [rowErrors, setRowErrors] = useState<Record<string, string>>({})

  const load = useCallback(async () => {
    try {
      setData(await fetchDocCores(scope))
      setLoadError(null)
    } catch (err) {
      // The window switched servers mid-read: the next server's read lands.
      if (isHostSwitchedError(err)) return
      setLoadError(errorMessage(err))
    }
  }, [scope])

  useEffect(() => {
    void load()
  }, [load])

  // Another person (or `k2 doc cores`) flipped a core on this server.
  useEffect(() => onDocCoresChanged(scope, () => void load()), [scope, load])

  const flip = useCallback(
    async (core: DocCore) => {
      if (!data?.canChange || busy) return
      const next = !core.on
      if (!next && core.open.files > 0) {
        const yes = await useConfirmDialogStore.getState().confirm({
          title: turnOffTitle(core),
          message: turnOffMessage(core.open),
          confirmLabel: 'Turn off',
          destructive: true,
        })
        if (!yes) return
      }
      setBusy(core.id)
      setRowErrors(({ [core.id]: _gone, ...rest }) => rest)
      try {
        await setDocCore(core.id, next, scope)
      } catch (err) {
        setRowErrors((prev) => ({ ...prev, [core.id]: errorMessage(err) }))
      } finally {
        setBusy(null)
      }
      await load()
    },
    [busy, data, load, scope],
  )

  const later = data?.later ?? []

  return (
    <div className="max-w-2xl space-y-4" data-settings-id="multiplayer.cores" data-testid="multiplayer-section">
      <div>
        <h2 className="text-sm font-medium text-[var(--color-text-primary)]">Multiplayer</h2>
        <p className={`text-[11px] ${muted} mt-1 leading-relaxed`}>{MULTIPLAYER_INTRO}</p>
        {data && !data.canChange ? (
          <p className={`text-[11px] ${muted} mt-1`} data-testid="multiplayer-member-line">
            {MULTIPLAYER_MEMBER_LINE}
          </p>
        ) : null}
      </div>
      {loadError ? (
        <div role="alert" className="text-[11px] text-[var(--color-status-error-soft)]">
          {loadError}
        </div>
      ) : null}
      {data ? (
        <div className="space-y-2">
          {data.cores.map((core) => (
            <CoreRow
              key={core.id}
              core={core}
              canChange={data.canChange}
              busy={busy === core.id}
              error={rowErrors[core.id] ?? null}
              onFlip={() => void flip(core)}
            />
          ))}
        </div>
      ) : null}
      {later.length > 0 ? (
        <p className={`text-[11px] ${muted}`} data-testid="multiplayer-later">
          Coming later: {later.join(', ')}.
        </p>
      ) : null}
    </div>
  )
}

function CoreRow({
  core,
  canChange,
  busy,
  error,
  onFlip,
}: {
  core: DocCore
  canChange: boolean
  busy: boolean
  error: string | null
  onFlip: () => void
}): React.JSX.Element {
  const enabled = core.on
  const failed = !core.on && core.lastChange && !core.lastChange.on ? core.lastChange.failed : []
  return (
    <div className="border border-[var(--color-border)] p-3" data-settings-id={`multiplayer.${core.id}`}>
      <div className="flex items-start gap-3">
        <button
          type="button"
          onClick={onFlip}
          role="switch"
          aria-checked={enabled}
          aria-label={core.name}
          disabled={!canChange || busy}
          className={`mt-0.5 w-7 h-3.5 flex items-center transition-colors no-drag cursor-pointer flex-shrink-0 disabled:opacity-50 disabled:cursor-default ${
            enabled ? 'bg-[var(--color-accent)]' : 'bg-[var(--color-border)]'
          }`}
        >
          <span
            className={`w-2.5 h-2.5 bg-[var(--color-on-accent)] block transition-transform ${
              enabled ? 'translate-x-3.5' : 'translate-x-0.5'
            }`}
          />
        </button>
        <div className="flex-1 min-w-0">
          <div className="text-xs font-medium text-[var(--color-text-primary)] flex items-center gap-2">
            <span>{core.name}</span>
            {core.status === 'beta' ? (
              <span className="text-[8px] uppercase tracking-wider font-semibold px-1.5 py-0.5 bg-[var(--color-accent)]/15 text-[var(--color-accent)]">
                Beta
              </span>
            ) : null}
          </div>
          <div className={`text-[10px] ${muted} mt-0.5 leading-relaxed`}>{core.description}</div>
          {core.on ? <div className={`text-[10px] ${muted} mt-1`}>{MULTIPLAYER_BETA_LINE}</div> : null}
          {core.on && core.open.files > 0 ? (
            <div className="text-[10px] text-[var(--color-text-secondary)] mt-1" data-testid={`multiplayer-open-${core.id}`}>
              {openNowLine(core.open)}
            </div>
          ) : null}
          {failed.map((f) => (
            <div
              key={f.path}
              className="text-[10px] text-[var(--color-status-warn-amber-soft)] mt-1"
              data-testid={`multiplayer-failed-${core.id}`}
            >
              {failedWriteBackLine(core, f)}
            </div>
          ))}
          {error ? (
            <div role="alert" className="text-[10px] text-[var(--color-status-error-soft)] mt-1">
              {error}
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}
