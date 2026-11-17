// Settings → General → Server → Postal (Beta) (prd-postal-in-k2-v1 §5.5):
// the server-wide switch, off by default, and "Admins may accept friend
// requests". Owner only (the daemon enforces it; everyone else sees the
// state read-only). `GET/POST /cli/postal/settings` work while the switch is
// off. After a flip the window's `/boot-status` is read again, so each
// workspace's Postal tab appears or goes (postal-api `refreshPostalFeature`).
//
// A server without Postal (an older daemon: 404) shows no group at all.

import React, { useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, SettingsGroup, useSaveStatus } from '../controls/SettingControls'
import { BetaTag, Toggle } from '@/components/ui'
import { httpStatusOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import {
  fetchPostalSettings,
  fetchPostalViewerRole,
  postalErrorText,
  refreshPostalFeature,
  savePostalSettings,
  type PostalSettings,
} from './postal-api'

export const POSTAL_SWITCH_LABEL = 'Postal (Beta): let agents have friends on other machines'
export const POSTAL_ADMINS_LABEL = 'Admins may accept friend requests'

export const POSTAL_SERVER_MANIFEST: SettingEntry[] = [
  { id: 'general.postal', section: 'general', group: 'Server', label: 'Postal', description: 'Let agents have friends on other machines (addresses like scout::acme.postal.bot). Off by default; owner only', keywords: ['postal', 'postal.bot', 'friends', 'other machines', 'address', 'invite', 'p5'], beta: true },
]

type State = { kind: 'loading' } | { kind: 'hidden' } | { kind: 'ready'; value: PostalSettings } | { kind: 'error'; message: string }

export function PostalServerSwitch({ scope }: { scope: ServerScope }): React.JSX.Element | null {
  const [state, setState] = useState<State>({ kind: 'loading' })
  const [role, setRole] = useState<string | null>(null)
  const [busy, setBusy] = useState<keyof PostalSettings | null>(null)
  const mainSave = useSaveStatus()
  const adminsSave = useSaveStatus()

  useEffect(() => {
    let live = true
    void fetchPostalViewerRole(scope).then((r) => {
      if (live) setRole(r)
    })
    fetchPostalSettings(scope)
      .then((v) => {
        if (live) setState({ kind: 'ready', value: v })
      })
      .catch((err) => {
        if (!live) return
        // No Postal on this server (older daemon), or this login may not read it.
        const status = httpStatusOf(err)
        if (status === 404 || status === 403) setState({ kind: 'hidden' })
        else setState({ kind: 'error', message: postalErrorText(err) })
      })
    return () => {
      live = false
    }
  }, [scope])

  if (state.kind === 'hidden' || state.kind === 'loading') return null

  const isOwner = role === 'owner'

  const flip = async (key: keyof PostalSettings, next: boolean): Promise<void> => {
    if (state.kind !== 'ready' || busy || !isOwner) return
    const save = key === 'enabled' ? mainSave : adminsSave
    const prev = state.value
    setBusy(key)
    save.reset()
    setState({ kind: 'ready', value: { ...prev, [key]: next } }) // optimistic
    try {
      const stored = await savePostalSettings(scope, { [key]: next })
      setState({ kind: 'ready', value: stored })
      save.saved()
      if (key === 'enabled') await refreshPostalFeature(scope)
    } catch (err) {
      setState({ kind: 'ready', value: prev })
      save.failed(new Error(postalErrorText(err)))
    } finally {
      setBusy(null)
    }
  }

  return (
    <SettingsGroup title="Postal" badge={<BetaTag />} card testId="postal-server-switch">
      {state.kind === 'error' ? (
        <div className="px-4 py-3">
          <div className={SETTING_ROW_LABEL}>{POSTAL_SWITCH_LABEL}</div>
          <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
            Couldn&rsquo;t read this setting: {state.message}
          </p>
        </div>
      ) : (
        <>
          <SettingSwitchRow
            settingId="general.postal"
            control={
              <>
                <SaveNote state={mainSave.state} />
                <Toggle
                  checked={state.value.enabled}
                  onChange={(v: boolean) => void flip('enabled', v)}
                  disabled={!isOwner || busy !== null}
                  aria-label={POSTAL_SWITCH_LABEL}
                  data-testid="postal-switch"
                />
              </>
            }
          >
            <div className={SETTING_ROW_LABEL}>{POSTAL_SWITCH_LABEL}</div>
            <div className={SETTING_ROW_HINT}>
              On: each workspace gets a Postal tab where its agent can claim an address (like scout::acme.postal.bot),
              invite friends and answer requests. The Postal network connection comes in a later release, so nothing
              leaves this server yet.
            </div>
            {role !== null && !isOwner ? (
              <div className={SETTING_ROW_HINT}>Only this server&rsquo;s owner can turn this on or off.</div>
            ) : null}
            <SaveError state={mainSave.state} />
          </SettingSwitchRow>
          {state.value.enabled ? (
            <SettingSwitchRow
              settingId="general.postal-admins-accept"
              control={
                <>
                  <SaveNote state={adminsSave.state} />
                  <Toggle
                    size="sm"
                    checked={state.value.adminsMayAccept}
                    onChange={(v: boolean) => void flip('adminsMayAccept', v)}
                    disabled={!isOwner || busy !== null}
                    aria-label={POSTAL_ADMINS_LABEL}
                    data-testid="postal-admins-accept"
                  />
                </>
              }
            >
              <div className={SETTING_ROW_LABEL}>{POSTAL_ADMINS_LABEL}</div>
              <div className={SETTING_ROW_HINT}>Off: only you accept new friends for this server&rsquo;s agents.</div>
              <SaveError state={adminsSave.state} />
            </SettingSwitchRow>
          ) : null}
        </>
      )}
    </SettingsGroup>
  )
}
