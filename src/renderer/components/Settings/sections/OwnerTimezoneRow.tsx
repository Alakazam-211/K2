// Settings → General: the zone a person's "13:00" means on this server, for
// reminders and mail check windows. The daemon owns it, so a headless box on
// UTC still gets the owner's 9 AM. Existing heartbeats don't move.

import React, { useEffect, useState } from 'react'
import { Button, Input } from '@/components/ui'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { daemonErrorText } from '@/lib/daemon-error'

export function OwnerTimezoneRow(): React.JSX.Element {
  const [value, setValue] = useState('')
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    void daemonCliGet<{ ownerTimezone?: string | null }>(primaryScope(), 'settings/get').then(
      (s) => setValue(s.ownerTimezone ?? ''),
      (e) => setError(daemonErrorText(e)),
    )
  }, [])

  const save = async (next: string): Promise<void> => {
    try {
      await daemonCliPost(primaryScope(), 'settings/update', { ownerTimezone: next })
      setValue(next)
      setError(null)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }

  return (
    <div data-testid="owner-timezone" className="space-y-1">
      <div className="text-[11px] text-[var(--color-text-secondary)]">Your timezone</div>
      <div className="flex items-center gap-2">
        <Input
          value={value}
          onChange={(e) => setValue(e.target.value)}
          onBlur={(e) => void save(e.target.value.trim())}
          placeholder="America/Denver"
          aria-label="Your timezone"
        />
        <Button size="sm" onClick={() => void save(Intl.DateTimeFormat().resolvedOptions().timeZone)}>
          Use this device's zone
        </Button>
      </div>
      <div className="text-[10px] text-[var(--color-text-muted)]">
        What a reminder's "13:00" and a mail check window mean. Empty: this server's own clock.
      </div>
      {error && <div className="text-[10px] text-[var(--color-status-warn)]">{error}</div>}
    </div>
  )
}
