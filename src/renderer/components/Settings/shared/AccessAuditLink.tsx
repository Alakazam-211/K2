// Settings → Access pages (0.45.7): one line under each page's title that
// opens Logs → Access Audit, the login history. Access Audit itself stays in
// Logs; this is only a way there from the pages that decide who gets in.

import React from 'react'
import { useSettingsStore } from '@/stores/settings'

export function AccessAuditLink({ className = '' }: { className?: string }): React.JSX.Element {
  return (
    <p className={`text-[10px] text-[var(--color-text-muted)] ${className}`.trim()} data-access-audit-link="">
      Who signed in, from where and when:{' '}
      <button
        type="button"
        className="underline hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
        onClick={() => useSettingsStore.getState().setSection('access-audit')}
      >
        Logs → Access Audit
      </button>
    </p>
  )
}
