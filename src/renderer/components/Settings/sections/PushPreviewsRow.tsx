// Settings → Notifications → Message previews (0.46.1).
//
// The owner's server-wide `pushMessagePreviews` switch (default on). On, each
// phone's own preview choice decides whether a Thread push shows the start of
// the agent's message; off, every phone gets the generic body. The daemon owns
// the value (`/cli/settings`, Admin floor in `REMOTE_ACCESS_KEYS`); this row
// renders it and sends the click.
//
// Shown only when the focused server's `/cli/settings/get` carries the key, so
// it appears once the daemon side lands and stays hidden on older servers. The
// caller (NotificationsSection) mounts it for the Owner and Admins only.

import React, { useEffect, useState } from 'react'

import { Toggle } from '@/components/ui'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, SettingsGroup, useSaveStatus } from '../controls/SettingControls'
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
import { PRODUCT_NAME } from '@/lib/brand'

export const PUSH_PREVIEWS_KEY = 'pushMessagePreviews'
export const PUSH_PREVIEWS_LABEL = 'Show message text in push notifications'
export const PUSH_PREVIEWS_HINT =
  `With previews on, the sender’s name and the start of each message go through ${PRODUCT_NAME}'s push relay and Apple’s and Google’s push services to reach phones. Turn previews off to send every phone on this server only “New message in Thread”.`

/** The switch's value, or `null` when this server doesn't have the key (an
 *  older daemon): the row hides. */
export function pushPreviewsValue(settings: Record<string, unknown> | null): boolean | null {
  const v = settings?.[PUSH_PREVIEWS_KEY]
  return typeof v === 'boolean' ? v : null
}

export function PushPreviewsRow(): React.JSX.Element | null {
  const [on, setOn] = useState<boolean | null>(null)
  const { state: saved, saved: markSaved, failed } = useSaveStatus()
  const [busy, setBusy] = useState(false)

  useEffect(() => {
    let cancelled = false
    settingsGet().then(
      (s) => {
        if (!cancelled) setOn(pushPreviewsValue(s as unknown as Record<string, unknown>))
      },
      (e: unknown) => {
        // Unknown means hidden: never show a switch whose value we can't read.
        console.warn('[notifications] settings read failed:', e)
      },
    )
    return () => {
      cancelled = true
    }
  }, [])

  if (on === null) return null

  const change = async (next: boolean): Promise<void> => {
    if (busy) return
    const prev = on
    setOn(next) // optimistic
    setBusy(true)
    try {
      const s = await settingsUpdate({ [PUSH_PREVIEWS_KEY]: next })
      const v = pushPreviewsValue(s as unknown as Record<string, unknown>)
      setOn(v ?? next)
      markSaved()
    } catch (e) {
      setOn(prev)
      failed(e)
    } finally {
      setBusy(false)
    }
  }

  return (
    <SettingsGroup title="Message previews" card>
      <SettingSwitchRow
        settingId="notifications.previews"
        testId="notifications-push-previews"
        control={
          <>
            <SaveNote state={saved} />
            <Toggle aria-label={PUSH_PREVIEWS_LABEL} checked={on} disabled={busy} onChange={(v: boolean) => void change(v)} />
          </>
        }
      >
        <div className={SETTING_ROW_LABEL}>{PUSH_PREVIEWS_LABEL}</div>
        <p className={SETTING_ROW_HINT}>{PUSH_PREVIEWS_HINT}</p>
        <SaveError state={saved} />
      </SettingSwitchRow>
    </SettingsGroup>
  )
}
