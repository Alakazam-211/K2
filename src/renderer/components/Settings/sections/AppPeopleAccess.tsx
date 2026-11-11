// Settings → Apps → <App> → People (app-mobile-access-v1, 0.46.3): which
// agents each person reaches, and what they may do there, in one grid; the
// App's suggested roles (chips, only while it has no roles yet); its
// devices that stay signed in; "Stay signed in" and discovery. Thin: the
// daemon builds the grid and owns every rule (app-people-api.ts).

import React, { useCallback, useEffect, useState } from 'react'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import { Button, Toggle } from '@/components/ui'
import {
  addRoleFromTemplate,
  cellSummary,
  fetchAccess,
  fetchTemplates,
  revokeDevice,
  setListed,
  setStaySignedIn,
  type AppAccess,
  type RoleTemplate,
} from './app-people-api'

const STAY_CHOICES = [
  { value: '0', label: 'Off (7-day login)' },
  { value: '7', label: '7 days' },
  { value: '30', label: '30 days' },
  { value: '60', label: '60 days' },
  { value: '90', label: '90 days' },
]

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export function AppPeopleAccess({
  app,
  hasRoles,
  onRolesChanged,
}: {
  /** `host` or a published service id. */
  app: string
  /** The App has at least one role (the suggestions show only before). */
  hasRoles: boolean
  onRolesChanged: () => void
}): React.JSX.Element {
  const [access, setAccess] = useState<AppAccess | null>(null)
  const [templates, setTemplates] = useState<RoleTemplate[]>([])
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  const load = useCallback(async () => {
    setError(null)
    try {
      setAccess(await fetchAccess(app))
    } catch (e) {
      setError(errText(e))
    }
  }, [app])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    if (hasRoles) return
    let live = true
    fetchTemplates()
      .then((t) => live && setTemplates(t))
      .catch((e) => live && setError(errText(e)))
    return () => {
      live = false
    }
  }, [hasRoles])

  const run = useCallback(
    async (key: string, f: () => Promise<void>) => {
      setBusy(key)
      setError(null)
      try {
        await f()
        await load()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [load],
  )

  return (
    <SettingsGroup
      card
      title="People"
      hint="Who has this App, and what each person may do with each agent."
    >
      <div data-settings-id="skin-access.people" className="divide-y divide-[var(--color-border)]">
        {error ? (
          <p role="alert" className="px-4 py-3 type-hint text-[var(--color-status-error-soft)]">
            That didn&rsquo;t work. <span className="text-[var(--color-text-muted)]">The server said: {error.trim().replace(/[.\s]+$/, '')}.</span>
          </p>
        ) : null}

        {!hasRoles && templates.length > 0 ? (
          <div className="px-4 py-3 space-y-1" aria-label="Suggested roles">
            <p className="type-hint text-[var(--color-text-muted)]">
              Suggested roles: pick one to start from, then edit it. Nothing is made until you click.
            </p>
            <div className="flex flex-wrap gap-1">
              {templates.map((t) => (
                <button
                  key={t.id}
                  type="button"
                  title={t.note ? `${t.description}\n\n${t.note}` : t.description}
                  disabled={busy != null}
                  onClick={() =>
                    void run(`tpl:${t.id}`, async () => {
                      await addRoleFromTemplate(app, t.id)
                      onRolesChanged()
                    })
                  }
                  data-k2-chamfer="control"
                  className="k2-focus px-3 py-1 type-control bg-[var(--color-bg-elevated)] border border-[var(--color-control-edge)] text-[var(--color-text-secondary)] hover:bg-[var(--color-border)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer disabled:opacity-60"
                >
                  + {t.label}
                </button>
              ))}
            </div>
          </div>
        ) : null}

        {access == null ? (
          <p className="px-4 py-3 type-hint text-[var(--color-text-secondary)]">Loading people…</p>
        ) : access.people.length === 0 ? (
          <p className="px-4 py-3 type-hint text-[var(--color-text-secondary)]">Nobody has this App yet. Add someone under Users.</p>
        ) : (
          <div className="px-4 py-3 overflow-x-auto">
            <table className="type-hint border-collapse" aria-label="People and agents">
              <thead>
                <tr>
                  <th className="text-left pr-3 py-1 font-normal text-[var(--color-text-muted)]">Person</th>
                  {access.agents.map((a) => (
                    <th key={a.room} scope="col" className="text-left pr-3 py-1 font-normal text-[var(--color-text-primary)]">
                      {a.color ? (
                        <span className="inline-block w-1.5 h-1.5 mr-1 align-middle" style={{ background: a.color }} />
                      ) : null}
                      {a.displayName}
                    </th>
                  ))}
                </tr>
              </thead>
              <tbody>
                {access.people.map((p) => (
                  <tr key={p.principalId} className="border-t border-[var(--color-border)]">
                    <th scope="row" className="text-left pr-3 py-1 font-normal text-[var(--color-text-primary)]">
                      {p.fullName ?? p.username}
                      {p.kind === 'agent' ? ' · agent' : ''}
                      <span className="block text-[var(--color-text-muted)]">
                        {p.enabled ? (p.role ?? 'rooms only (Thread)') : 'off'}
                      </span>
                    </th>
                    {access.agents.map((a) => {
                      const c = p.cells.find((x) => x.room === a.room)
                      const folders = c?.folders
                        ? [...c.folders.read.map((f) => `read ${f}`), ...c.folders.write.map((f) => `write ${f}`)].join(', ')
                        : ''
                      return (
                        <td
                          key={a.room}
                          data-testid={`cell-${p.username}-${a.handle}`}
                          className="pr-3 py-1 align-top text-[var(--color-text-secondary)]"
                          title={c ? c.caps.join(', ') : 'no access'}
                        >
                          {c ? cellSummary(c) : '—'}
                          {folders ? <span className="block text-[var(--color-text-muted)]">{folders}</span> : null}
                        </td>
                      )
                    })}
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        {access ? (
          <>
            <SettingRow label="Stay signed in" hint="How long a phone stays signed in without being used.">
              <SettingDropdown
                ariaLabel="Stay signed in"
                value={String(access.staySignedInDays)}
                options={STAY_CHOICES}
                onChange={(v) => void run('stay', () => setStaySignedIn(app, Number(v)))}
                menuAlign="right"
              />
            </SettingRow>
            {app !== 'host' ? (
              <SettingRow
                label="Listed for sign-in"
                hint="On: an app can find this App and see only its name and how to sign in. Off: the server answers as if nothing were here."
              >
                <Toggle
                  aria-label="Listed for sign-in"
                  checked={access.listed}
                  disabled={busy != null}
                  onChange={(on: boolean) => void run('listed', () => setListed(app, on))}
                />
              </SettingRow>
            ) : null}
            {access.devices.length > 0 ? (
              <div className="px-4 py-3 space-y-1" aria-label="Signed-in devices">
                {access.devices.map((d) => (
                  <div key={d.id} className="flex items-center justify-between gap-2 type-hint">
                    <span className="text-[var(--color-text-secondary)]">
                      {d.username} · {d.name}
                      {d.via === 'pairing' ? (
                        <span
                          title="Signed in by QR from the App's web page"
                          className="ml-1 px-1 border border-[var(--color-border)] text-[var(--color-text-muted)]"
                        >
                          Paired
                        </span>
                      ) : null}
                    </span>
                    <Button
                      variant="danger-quiet"
                      aria-label={`Sign out ${d.name}`}
                      disabled={busy != null}
                      onClick={() => void run(`dev:${d.id}`, () => revokeDevice(d.id))}
                    >
                      Sign out
                    </Button>
                  </div>
                ))}
              </div>
            ) : null}
          </>
        ) : null}
      </div>
    </SettingsGroup>
  )
}
