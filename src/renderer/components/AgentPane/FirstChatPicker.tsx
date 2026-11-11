// The first pinned chat's question (0.46.11): which LLM does this agent's
// chat start with? The daemon decides when to ask and lists the harnesses
// (`pinned_chat_choice.rs`); this draws them. Each row can install the CLI
// (`agents/ensure-cli`) or open the existing sign-in sheet on the room's
// server, then Start sends the pick back on `ensure-pinned-chat`.

import { useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { Button } from '@/components/ui'
import { ProviderIcon } from '@/components/AgentIcon/ProviderIcon'
import { LlmLoginSheet, type LoginSheetStart } from '@/components/Settings/shared/LlmLoginSheet'
import { ensureOneCli, isInstallableCli } from '@/lib/ensure-cli'
import { harnessStatusText, type FirstChatHarness } from '@/lib/first-chat-choice'

export function FirstChatPicker({
  scope,
  harnesses,
  starting,
  error,
  onStart,
  onRecheck,
}: {
  scope: ServerScope
  harnesses: FirstChatHarness[]
  /** The harness a Start is in flight for. */
  starting: string | null
  error: string | null
  onStart: (harness: string) => void
  /** Ask the daemon again (after an install or a sign-in). */
  onRecheck: () => void
}): React.JSX.Element {
  const [sheet, setSheet] = useState<LoginSheetStart | null>(null)
  const [installing, setInstalling] = useState<string | null>(null)
  const [installError, setInstallError] = useState<string | null>(null)
  const busy = starting !== null || installing !== null

  const install = async (h: FirstChatHarness): Promise<void> => {
    if (!isInstallableCli(h.harness)) return
    setInstalling(h.harness)
    setInstallError(null)
    try {
      await ensureOneCli(scope, h.harness)
      onRecheck()
    } catch (e) {
      setInstallError(`${h.label} didn't install: ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setInstalling(null)
    }
  }

  const shownError = error ?? installError
  return (
    <div className="flex-1 min-h-0 overflow-y-auto flex flex-col items-center justify-center gap-3 px-6 py-6" data-testid="first-chat-picker">
      <div className="text-xs font-semibold text-[var(--color-text-primary)] text-center">Choose the LLM this agent starts with</div>
      <div className="text-[11px] text-[var(--color-text-muted)] max-w-[52ch] text-center leading-relaxed">
        This agent's chat runs on the LLM you pick. Sign in to it here, or start and sign in inside the chat. To change it later, use "Move pinned chat to…" in the chat menu.
      </div>
      <div className="w-full max-w-md border border-[var(--color-border)]">
        {harnesses.length === 0 && (
          <div className="px-3 py-2.5 text-[11px] text-[var(--color-text-muted)]">
            No agent preset is turned on. Turn one on in Settings → Agents, then check again.
          </div>
        )}
        {harnesses.map((h, i) => (
          <div
            key={h.harness}
            data-testid={`first-chat-${h.harness}`}
            className={`flex items-center justify-between gap-3 px-3 py-2.5${i === harnesses.length - 1 ? '' : ' border-b border-[var(--color-border)]'}`}
          >
            <div className="flex items-start gap-2 min-w-0">
              <span className="mt-0.5 flex-shrink-0">
                <ProviderIcon provider={h.harness} size={14} />
              </span>
              <div className="min-w-0">
                <div className="text-xs text-[var(--color-text-primary)]">{h.label}</div>
                <div className="text-[11px] text-[var(--color-text-muted)]" data-testid={`first-chat-${h.harness}-status`}>
                  {installing === h.harness ? `Installing ${h.label}…` : harnessStatusText(h)}
                </div>
              </div>
            </div>
            <div className="flex items-center gap-1.5 flex-shrink-0">
              {!h.installed && h.install && (
                <Button disabled={busy} onClick={() => void install(h)} title={`Install ${h.label} on ${scope.label}`}>
                  Install
                </Button>
              )}
              {h.installed && !h.signedIn && h.signIn && (
                <Button disabled={busy} onClick={() => setSheet({ kind: 'add', tool: h.harness, toolLabel: h.label })}>
                  Sign in
                </Button>
              )}
              {h.installed && (
                <Button variant="accent" disabled={busy} onClick={() => onStart(h.harness)}>
                  {starting === h.harness ? 'Starting…' : `Start with ${h.label}`}
                </Button>
              )}
            </div>
          </div>
        ))}
      </div>
      {shownError && (
        <div className="text-[11px] text-[var(--color-status-error-text)] max-w-[52ch] text-center" role="alert">
          {shownError}
        </div>
      )}
      <Button variant="quiet" disabled={busy} onClick={onRecheck}>
        Check again
      </Button>
      {sheet && (
        <LlmLoginSheet
          scope={scope}
          start={sheet}
          onClose={() => {
            setSheet(null)
            onRecheck()
          }}
        />
      )}
    </div>
  )
}
