// Settings → Apps → <App> → People (app-mobile-access-v1, 0.46.3): which
// agents each person reaches, and what they may do there, in one grid; the
// App's suggested roles (chips, only while it has no roles yet); its
// devices that stay signed in; "Stay signed in" and discovery. Thin: the
// daemon builds the grid and owns every rule (app-people-api.ts).

import React, { useCallback, useEffect, useState } from 'react'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
import { Toggle } from '@/components/ui'
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
    <SettingsGroup title="People">
      <div data-settings-id="skin-access.people" className="space-y-3">
        {error ? (
          <p role="alert" className="text-[10px] text-[var(--color-status-error-soft)]">
            {error}
          </p>
        ) : null}

        {!hasRoles && templates.length > 0 ? (
          <div className="space-y-1" aria-label="Suggested roles">
            <p className="text-[10px] text-[var(--color-text-muted)]">
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
                  className="px-2 py-0.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-accent)] no-drag cursor-pointer disabled:opacity-60"
                >
                  + {t.label}
                </button>
              ))}
            </div>
          </div>
        ) : null}

        {access == null ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">Loading people…</p>
        ) : access.people.length === 0 ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">Nobody has this App yet.</p>
        ) : (
          <div className="overflow-x-auto">
            <table className="text-[10px] border-collapse" aria-label="People and agents">
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
            <p className="text-[10px] text-[var(--color-text-muted)]">
              Stay signed in: how long a phone that signs in stays signed in without being used.
            </p>
            <SettingRow label="Stay signed in">
              <SettingDropdown
                ariaLabel="Stay signed in"
                value={String(access.staySignedInDays)}
                options={STAY_CHOICES}
                onChange={(v) => void run('stay', () => setStaySignedIn(app, Number(v)))}
                menuAlign="right"
              />
            </SettingRow>
            {app !== 'host' ? (
              <p className="text-[10px] text-[var(--color-text-muted)]">
                Listed: an app can find this App by its id and see only its name and how to sign in. Off,
                the server answers as if nothing were here.
              </p>
            ) : null}
            {app !== 'host' ? (
              <SettingRow label="Listed for sign-in">
                <Toggle
                  aria-label="Listed for sign-in"
                  checked={access.listed}
                  disabled={busy != null}
                  onChange={(on: boolean) => void run('listed', () => setListed(app, on))}
                />
              </SettingRow>
            ) : null}
            {access.devices.length > 0 ? (
              <div className="space-y-1" aria-label="Signed-in devices">
                {access.devices.map((d) => (
                  <div key={d.id} className="flex items-center justify-between gap-2 text-[10px]">
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
                    <button
                      type="button"
                      aria-label={`Sign out ${d.name}`}
                      disabled={busy != null}
                      onClick={() => void run(`dev:${d.id}`, () => revokeDevice(d.id))}
                      className="text-[var(--color-text-muted)] hover:text-[var(--color-status-error-soft)] hover:underline no-drag cursor-pointer"
                    >
                      Sign out
                    </button>
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
