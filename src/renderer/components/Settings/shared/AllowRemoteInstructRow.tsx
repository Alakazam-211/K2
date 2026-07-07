import React from 'react'
import { useCallback } from 'react'
import { useSettingsStore } from '@/stores/settings'

// ── Composer 1c (D4) — "Allow remote instruct" ─────────────────────────
// Per-host opt-in that lets REMOTE PRINCIPALS message this host's agents:
//   1. CONNECT-USERS (people who signed into this host over K2 Connect)
//      via the message composer.
//   2. Paired FEDERATION servers — inbound cross-server messages are
//      DECLINED (delivered:false) unless this host (or the addressed
//      workspace) opted in; this is the delivery-consent half of
//      federation, next to the "Enable federation" master that turns the
//      federation surface on at all.
// The OWNER (you, on this machine) can always message agents regardless.
// DEFAULTS OFF: a remote message instructs an agent running
// --dangerously-skip-permissions (= full shell + filesystem access), so a
// host must be explicitly opted in. The daemon enforces this server-side
// (composer drain-then-403; federation inbound decline); this toggle sets
// the host flag AND drives the renderer-hide of the composer for
// non-owner principals. Per-workspace override: Workspaces → (workspace)
// → "Let remote users message this workspace" — this app-level switch
// opts in ALL workspaces at once (back-compat master).
//
// Home: Settings → K2 Connect, directly beneath "Enable federation" —
// both remote-access switches live together (the federation-toggle
// topology note: this row moved here FROM General so enabling federation
// and granting delivery consent are one surface, not two pages).
export function AllowRemoteInstructRow(): React.JSX.Element {
  const allow = useSettingsStore((s) => s.allowRemoteInstruct)
  const setAllow = useSettingsStore((s) => s.setAllowRemoteInstruct)

  const toggle = useCallback(() => {
    void setAllow(!allow)
  }, [allow, setAllow])

  return (
    <div className="flex items-center justify-between py-2">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">
          Let remote users message agents
        </span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {allow
            ? 'People signed into this host over K2 Connect — and, when federation is on, paired servers — can send messages to your agents. You (the owner) can always message agents.'
            : 'Off (recommended): only you can message agents here. Turn on to let K2 Connect users and paired federation servers message agents on this host — they run with full shell and filesystem access. Each workspace can also opt in individually under Workspaces.'}
        </p>
      </div>
      <button
        onClick={toggle}
        className="no-drag cursor-pointer flex-shrink-0 relative"
        data-settings-id="k2-connect.allow-remote-instruct"
        style={{
          width: 36,
          height: 20,
          backgroundColor: allow ? 'var(--color-accent)' : '#333',
          border: 'none',
          transition: 'background-color 150ms',
        }}
      >
        <span
          style={{
            position: 'absolute',
            top: 2,
            left: allow ? 18 : 2,
            width: 16,
            height: 16,
            backgroundColor: 'var(--color-on-accent)',
            transition: 'left 150ms',
          }}
        />
      </button>
    </div>
  )
}
