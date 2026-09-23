// Settings → K2 Server → People. Lists skin principals (username + full name).
// Reads principals via GET /cli/skin/users. Not a people table. Not Server
// Access (Connect users) and not Sidecars → Apps.

import React, { useCallback, useEffect, useState } from 'react'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import type { SettingEntry } from '../searchManifest'

export const PEOPLE_MANIFEST: SettingEntry[] = [
  {
    id: 'people.roster',
    section: 'people',
    label: 'People',
    description: 'Skin principals on this box — username and full name. Not Server Access.',
    keywords: ['people', 'full name', 'principal', 'guest', 'username'],
    group: 'People',
  },
]

export type PersonRow = {
  username: string
  fullName: string | null
}

function asRecord(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

function asString(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

export function parsePeople(raw: unknown): PersonRow[] {
  const list = asRecord(raw).users
  if (!Array.isArray(list)) return []
  return list.flatMap((row) => {
    const rec = asRecord(row)
    const username = asString(rec.username)
    if (!username) return []
    return [{
      username,
      fullName: asString(rec.fullName) ?? asString(rec.full_name),
    }]
  })
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

const INPUT_CLS =
  'flex-1 min-w-[8rem] px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag'

export function PeopleSection(): React.JSX.Element {
  const [people, setPeople] = useState<PersonRow[]>([])
  const [drafts, setDrafts] = useState<Record<string, string>>({})
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    setError(null)
    try {
      const roster = await daemonCliGet<unknown>('skin/users')
      setPeople(parsePeople(roster))
    } catch (e) {
      setError(errText(e))
      setPeople([])
    }
    setLoading(false)
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const save = useCallback(
    async (username: string, fullName: string) => {
      setBusy(username)
      setError(null)
      try {
        await daemonCliPost('skin/users/full-name', { username, fullName })
        setDrafts((prev) => {
          const next = { ...prev }
          delete next[username]
          return next
        })
        await refresh()
      } catch (e) {
        setError(errText(e))
      } finally {
        setBusy(null)
      }
    },
    [refresh],
  )

  return (
    <div className="max-w-2xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">People</h2>
      <p className="text-[10px] text-[var(--color-text-muted)] mb-4 leading-relaxed">
        Username and full name for app guests on this box. This reads principals.
        It is not Server Access and not a separate people list.
      </p>
      <div data-settings-id="people.roster" className="space-y-2">
        {error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error)]">
            {error}
          </p>
        )}
        {loading ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">Loading…</p>
        ) : people.length === 0 ? (
          <p className="text-[10px] text-[var(--color-text-muted)]">
            No skin users yet. Add them under Sidecars → Apps.
          </p>
        ) : (
          <div className="divide-y divide-[var(--color-border)]">
            {people.map((person) => {
              const value = drafts[person.username] ?? person.fullName ?? ''
              return (
                <form
                  key={person.username}
                  className="flex flex-wrap items-center gap-2 py-2"
                  onSubmit={(e) => {
                    e.preventDefault()
                    void save(person.username, value.trim())
                  }}
                >
                  <span className="w-28 text-xs font-mono text-[var(--color-text-primary)] truncate">
                    {person.username}
                  </span>
                  <input
                    className={INPUT_CLS}
                    aria-label={`${person.username} full name`}
                    placeholder="full name"
                    value={value}
                    disabled={busy === person.username}
                    onChange={(e) =>
                      setDrafts((prev) => ({ ...prev, [person.username]: e.target.value }))
                    }
                  />
                  <button
                    type="submit"
                    aria-label={`Save ${person.username} full name`}
                    disabled={busy === person.username}
                    className="text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer disabled:opacity-40"
                  >
                    Save
                  </button>
                  {person.fullName ? (
                    <button
                      type="button"
                      aria-label={`Clear ${person.username} full name`}
                      disabled={busy === person.username}
                      onClick={() => void save(person.username, '')}
                      className="text-[10px] text-[var(--color-text-muted)] hover:underline no-drag cursor-pointer disabled:opacity-40"
                    >
                      Clear
                    </button>
                  ) : null}
                </form>
              )
            })}
          </div>
        )}
      </div>
    </div>
  )
}
