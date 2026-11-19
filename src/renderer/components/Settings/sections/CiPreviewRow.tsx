// Settings → Compute → Machines → "CI on K2 Compute (preview)" (0.46.17).
//
// The server switch `ciPreview`: on, K2 checks every pushed commit an agent
// set up with `k2 ci` as compute jobs on this server's machines. Owner only
// (the daemon's `OWNER_ONLY_KEYS`); everyone else sees it read-only. An agent
// asks for it with the `ci.preview` permission card. It takes effect at
// once, but only while compute is on.

import React, { useEffect, useState } from 'react'
import type { SettingEntry } from '../searchManifest'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'
import { Toggle } from '@/components/ui'
import { PRODUCT_NAME } from '@/lib/brand'
import type { HostRole } from './host-role'
import { ciPreviewErrorText, fetchCiPreview, fetchCiPreviewRole, saveCiPreview } from './ci-preview-api'

export const CI_PREVIEW_LABEL = `CI on ${PRODUCT_NAME} Compute (preview)`

export const CI_PREVIEW_ENTRY: SettingEntry = {
  id: 'compute.ci-preview',
  section: 'compute',
  label: CI_PREVIEW_LABEL,
  description: `${PRODUCT_NAME} checks every pushed commit on your compute machines (k2 ci). Owner only`,
  keywords: ['ci', 'k2 ci', 'ciPreview', 'continuous integration', 'checks', 'push', 'commit', 'github status'],
}

type Value = { kind: 'loading' } | { kind: 'ready'; on: boolean } | { kind: 'error'; message: string }

export function CiPreviewRow({ computeOn }: { computeOn: boolean }): React.JSX.Element {
  const [value, setValue] = useState<Value>({ kind: 'loading' })
  const [role, setRole] = useState<HostRole | null>(null)
  const [busy, setBusy] = useState(false)
  const save = useSaveStatus()

  useEffect(() => {
    let live = true
    void fetchCiPreviewRole().then((r) => {
      if (live) setRole(r)
    })
    fetchCiPreview()
      .then((on) => {
        if (live) setValue({ kind: 'ready', on })
      })
      .catch((err: unknown) => {
        if (live) setValue({ kind: 'error', message: ciPreviewErrorText(err) })
      })
    return () => {
      live = false
    }
  }, [])

  const isOwner = role === 'owner'

  const flip = async (next: boolean): Promise<void> => {
    if (value.kind !== 'ready' || busy || !isOwner) return
    const prev = value.on
    setBusy(true)
    save.reset()
    setValue({ kind: 'ready', on: next }) // optimistic
    try {
      const stored = await saveCiPreview(next)
      setValue({ kind: 'ready', on: stored })
      save.saved()
    } catch (err) {
      setValue({ kind: 'ready', on: prev })
      save.failed(new Error(ciPreviewErrorText(err)))
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingSwitchRow
      settingId={CI_PREVIEW_ENTRY.id}
      testId="ci-preview-row"
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle
            checked={value.kind === 'ready' && value.on}
            onChange={(v: boolean) => void flip(v)}
            disabled={!isOwner || busy || value.kind !== 'ready'}
            aria-label={CI_PREVIEW_LABEL}
            data-testid="ci-preview-switch"
          />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>{CI_PREVIEW_LABEL}</div>
      <p className={SETTING_ROW_HINT}>
        {PRODUCT_NAME} checks every pushed commit on your compute machines, for the repos agents set up with k2 ci. CI checks go
        ahead of other jobs in the queue, so other jobs may wait longer. Changes take effect at once.
      </p>
      {!computeOn ? (
        <p className={SETTING_ROW_HINT} data-testid="ci-preview-needs-compute">
          It works only while Let agents use your machines is on.
        </p>
      ) : null}
      {role !== null && !isOwner ? (
        <p className={SETTING_ROW_HINT} data-testid="ci-preview-owner-only">
          Only this server&rsquo;s owner can turn this on or off. An agent can ask the owner on a permission card.
        </p>
      ) : null}
      {value.kind === 'error' ? (
        <p role="alert" className="type-hint text-[var(--color-status-error-soft)] mt-1">
          Couldn&rsquo;t read this setting: {value.message}
        </p>
      ) : null}
      <SaveError state={save.state} />
    </SettingSwitchRow>
  )
}
