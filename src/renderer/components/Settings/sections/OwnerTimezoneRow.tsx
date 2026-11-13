// Settings → General: the zone a person's "13:00" means on this server, for
// reminders and mail check windows. The daemon owns it, so a headless box on
// UTC still gets the owner's 9 AM. Existing heartbeats don't move.

import React, { useEffect, useState } from 'react'
import { Button, Callout, Input } from '@/components/ui'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { daemonErrorText } from '@/lib/daemon-error'
import { SettingRow } from '../controls/SettingControls'

export function OwnerTimezoneRow(): React.JSX.Element {
  const [value, setValue] = useState('')
  const [error, setError] = useState<string | null>(null)
  const deviceZone = Intl.DateTimeFormat().resolvedOptions().timeZone

  useEffect(() => {
    void daemonCliGet<{ ownerTimezone?: string | null }>(primaryScope(), 'settings/get').then(
      (s) => setValue(s.ownerTimezone ?? ''),
      (e) => setError(`Couldn't read your timezone: ${daemonErrorText(e)}`),
    )
  }, [])

  const save = async (next: string): Promise<void> => {
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { ownerTimezone: next })
      setValue(next)
      setError(null)
    } catch (e) {
      setError(`Couldn't save your timezone: ${daemonErrorText(e)}`)
    }
  }

  return (
    <SettingRow
      label="Your timezone"
      settingId="general.owner-timezone"
      testId="owner-timezone"
      hint={
        <>
          What a reminder&apos;s &ldquo;13:00&rdquo; and a mail check window mean. Empty: this server&apos;s own clock.
          {deviceZone && value !== deviceZone && (
            <span className="block mt-1.5">
              {/* A long zone name wraps rather than running off the card. */}
              <Button className="max-w-full !whitespace-normal text-left" onClick={() => void save(deviceZone)}>
                Use this device&apos;s zone ({deviceZone})
              </Button>
            </span>
          )}
        </>
      }
      below={
        error ? (
          <Callout tone="error" role="alert">
            <p className="type-hint">{error}</p>
          </Callout>
        ) : undefined
      }
      field
    >
      <Input
        value={value}
        onChange={(e) => setValue(e.target.value)}
        onBlur={(e) => void save(e.target.value.trim())}
        placeholder="America/Denver"
        aria-label="Your timezone"
        className="w-full"
      />
    </SettingRow>
  )
}
