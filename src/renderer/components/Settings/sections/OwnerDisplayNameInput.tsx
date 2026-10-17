// The owner's display name input ("Your name"): the name K2 agents call
// the owner when they message an agent. The daemon resolves the `from`
// attribution from app_settings (`ownerDisplayName`), never from the
// request body (D3), and falls back to "owner" when blank.
//
// Shared by Settings → General → You and the profile modal, so both edit
// the same store value the same way: type freely, commit (sanitized) on
// blur or Enter; the store saves optimistically and rolls back on a
// refusal (`setOwnerDisplayName`).

import React, { useCallback, useEffect, useState } from 'react'
import { useSettingsStore, sanitizeOwnerDisplayName, OWNER_DISPLAY_NAME_MAX } from '@/stores/settings'

export function OwnerDisplayNameInput({ className }: { className?: string }): React.JSX.Element {
  const ownerDisplayName = useSettingsStore((s) => s.ownerDisplayName)
  const setOwnerDisplayName = useSettingsStore((s) => s.setOwnerDisplayName)
  const [draft, setDraft] = useState<string>(ownerDisplayName)

  useEffect(() => {
    setDraft(ownerDisplayName)
  }, [ownerDisplayName])

  const commit = useCallback(() => {
    const next = sanitizeOwnerDisplayName(draft)
    setDraft(next)
    if (next !== ownerDisplayName) {
      void setOwnerDisplayName(next)
    }
  }, [draft, ownerDisplayName, setOwnerDisplayName])

  return (
    <input
      type="text"
      aria-label="Your name"
      data-testid="owner-display-name-input"
      value={draft}
      placeholder="owner"
      maxLength={OWNER_DISPLAY_NAME_MAX}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.currentTarget.blur()
        }
      }}
      className={`${className ?? 'w-40'} px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag`}
    />
  )
}
