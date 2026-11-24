import React from 'react'
import { useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import type { ProjectWithWorkspaces } from '@/stores/projects'

// Moved out of ProjectsSection.tsx as it was (0.46.18 queue, ratchet).
// How many hosted addresses this workspace's agent may create (0.46.18):
// the daemon's own line from GET /cli/mail/config?workspace=
// (`workspace.addressCap.text`), e.g. "Addresses: unlimited (this agent
// manages email hosting)". Read only; the owner sets a cap with
// `k2 hostmail config --workspace <ws> --address-cap <n>|none`. Hidden
// where hosted mail can't run. Read again when "manage hosted mail" flips.
export function MailAddressCapLine({
  project,
}: {
  project: ProjectWithWorkspaces
}): React.JSX.Element | null {
  const [text, setText] = useState<string | null>(null)
  const manages = project.mailManageEnabled ?? 0

  useEffect(() => {
    let cancelled = false
    daemonCliGet<{
      supported?: boolean
      workspace?: { addressCap?: { text?: string } }
    }>(primaryScope(), 'mail/config', { workspace: project.path })
      .then((d) => {
        if (cancelled) return
        setText(d.supported === true ? (d.workspace?.addressCap?.text ?? null) : null)
      })
      .catch(() => {
        if (!cancelled) setText(null)
      })
    return () => {
      cancelled = true
    }
  }, [project.path, manages])

  if (!text) return null
  return (
    <div data-testid="mail-address-cap" className="px-4 pb-3 type-hint text-[var(--color-text-secondary)]">
      {text}
    </div>
  )
}
