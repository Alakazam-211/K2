// "Ask Sam" on a refusal's row (editors-notes.md v3, l08): one line under the
// header with a message to the holder, already written and yours to change,
// sent through the file's say route (`POST /cli/docs/say`; an agent also gets
// it in its inbox). Nothing goes until you press Send. The full say row on
// the text, with Follow, is E4b.

import { useState } from 'react'
import { Button } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import { failureReason } from '@/lib/editor/save-model/write'
import { PRODUCT_NAME } from '@/lib/brand'

export interface Ask {
  to: { id: string; name: string }
  /** What they hold, in words ("Configuration"); empty when unknown. */
  what: string
}

export const askDraft = (a: Ask): string => (a.what ? `Could you tell me when you’re done with ${a.what}? I’d like to edit it.` : 'Could you tell me when you’re done? I’d like to edit it.')

export function AskBar({ scope, filePath, ask, onClose }: { scope: ServerScope; filePath: string; ask: Ask; onClose: () => void }): React.JSX.Element {
  const [text, setText] = useState(() => askDraft(ask))
  const [state, setState] = useState<{ kind: 'draft' | 'sending' | 'sent' } | { kind: 'failed'; reason: string }>({ kind: 'draft' })
  const send = (): void => {
    const t = text.trim()
    if (!t || state.kind === 'sending') return
    setState({ kind: 'sending' })
    daemonCliPost(scope, 'docs/say', { path: filePath, text: t, to: ask.to.id }).then(
      () => setState({ kind: 'sent' }),
      (e: unknown) => setState({ kind: 'failed', reason: failureReason(e) }),
    )
  }
  return (
    <div role="group" aria-label={`Ask ${ask.to.name}`} data-testid="ed-ask" data-k2-chamfer="surface" className="lv-notice mx-2.5 mt-2 flex flex-none flex-wrap items-center gap-x-2.5 gap-y-1.5 rounded-[var(--radius-box)] border px-3 py-2" data-tone="info">
      {state.kind === 'sent' ? (
        <p className="type-row m-0 flex-1 text-[var(--color-text-primary)]">
          <b className="font-semibold">Sent to {ask.to.name}.</b> <span className="text-[var(--color-text-secondary)]">Their answer comes to you in {PRODUCT_NAME}.</span>
        </p>
      ) : (
        <label className="type-row flex min-w-[240px] flex-1 items-center gap-2 text-[var(--color-text-primary)]">
          <b className="flex-none font-semibold">Ask {ask.to.name}</b>
          <input
            data-k2-focus=""
            value={text}
            maxLength={1000}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') send()
              if (e.key === 'Escape') onClose()
            }}
            className="min-w-0 flex-1 rounded-[var(--radius-field)] border border-[var(--color-control-edge)] bg-[var(--color-bg-inset)] px-2 py-1 text-[var(--color-text-primary)]"
          />
        </label>
      )}
      {state.kind === 'failed' ? <span className="type-hint text-[var(--color-status-error-text)]">Not sent: {state.reason}.</span> : null}
      <div className="ml-auto flex flex-none items-center gap-1.5">
        {state.kind === 'sent' ? null : (
          <Button variant="accent" onClick={send} aria-disabled={state.kind === 'sending' || undefined}>
            {state.kind === 'sending' ? 'Sending' : state.kind === 'failed' ? 'Send again' : 'Send'}
          </Button>
        )}
        <Button onClick={onClose} className="text-[var(--color-text-primary)]">
          {state.kind === 'sent' ? 'Done' : 'Cancel'}
        </Button>
      </div>
    </div>
  )
}
