import React, { useEffect, useState } from 'react'

import { Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingSwitchRow } from '../controls/SettingControls'
import { daemonCliGet } from '@/lib/daemon-cli'
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import type { ServerScope } from '@/kessel/server-scope'

// Settings → General → Server: "Allow cloning agents to people's personal
// computers" (prd-clone-carries-threads-and-vault-v1 §4.2; D2 as decided:
// default ON). A daemon-owned, Owner-only key (`allowClonePersonalComputers`)
// on the FOCUSED server; the daemon's clone gate is the real check, this row
// only shows and sets it. Admins and Members see it read-only.

export const CLONE_SWITCH_LABEL = "Allow cloning agents to people's personal computers"
export const CLONE_SWITCH_OWNER_HINT = 'Only the owner of this server can change this.'

export interface WhoamiLike {
  owner?: boolean
  role?: string
}

/** The Owner (owner token or an Owner-role login) may change the switch. */
export function canSetCloneSwitch(w: WhoamiLike | null): boolean {
  return !!w && (w.owner === true || w.role === 'owner')
}

/** Absent on an older daemon (or unset) reads as ON, the default. */
export function cloneSwitchValue(settings: Record<string, unknown> | null): boolean {
  return settings?.allowClonePersonalComputers !== false
}

/** `scope` = the focused server (General passes its primary scope). */
export function ClonePersonalComputersRow({ scope }: { scope: ServerScope }): React.JSX.Element {
  const [on, setOn] = useState(true)
  const [who, setWho] = useState<WhoamiLike | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const [w, s] = await Promise.all([
          daemonCliGet<WhoamiLike>(scope, 'auth/whoami'),
          settingsGet(),
        ])
        if (cancelled) return
        setWho(w)
        setOn(cloneSwitchValue(s as unknown as Record<string, unknown>))
      } catch (e) {
        if (!cancelled) setError(`Couldn't read this setting: ${e instanceof Error ? e.message : String(e)}`)
      }
    })()
    return () => {
      cancelled = true
    }
    // Load once per mount (Settings remounts on a server switch).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const editable = canSetCloneSwitch(who)

  const change = async (next: boolean): Promise<void> => {
    if (!editable || busy) return
    const prev = on
    setOn(next) // optimistic
    setBusy(true)
    setError(null)
    try {
      const s = await settingsUpdate({ allowClonePersonalComputers: next })
      setOn(cloneSwitchValue(s as unknown as Record<string, unknown>))
    } catch (e) {
      setOn(prev)
      setError(`Couldn't change it: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      settingId="general.clone-personal-computers"
      control={
        <Toggle
          checked={on}
          onChange={(v: boolean) => void change(v)}
          disabled={!editable || busy}
          aria-label={CLONE_SWITCH_LABEL}
        />
      }
    >
      <div className={SETTING_ROW_LABEL}>{CLONE_SWITCH_LABEL}</div>
      <p className={SETTING_ROW_HINT}>
        When off, Admins and Members can&apos;t clone agents from this server to their own computers. The owner
        always can. {CLONE_SWITCH_OWNER_HINT}
      </p>
      {error && (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
          {error}
        </p>
      )}
    </SettingSwitchRow>
  )
}
