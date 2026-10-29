// Saving into the prompt stash from a Thread (spec-prompt-stash-v1):
//   - "Save to stash" in an agent message's menu (right-click, or the ⋯
//     beside "author · time");
//   - the `stash.offer` card: an agent offers a prompt, the person's tap
//     saves it on this device. Agents never write the stash themselves,
//     and the tap sends nothing back to the daemon.

import { useState, type JSX } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayStashOffer } from '@/components/SessionView/overlayThread'
import { stashSourceFor } from '@/lib/prompt-stash/compose'
import type { StashSource } from '@/lib/prompt-stash/record'
import { useContextMenuStore } from '@/stores/context-menu'
import { saveToStash } from '@/stores/prompt-stash'
import { useToastStore } from '@/stores/toast'
import { StashGlyph } from './PromptStash'

function sourceFor(scope: ServerScope | null, agent: string, agentName: string): StashSource {
  if (scope) return stashSourceFor(scope, agent, agentName)
  return { server_id: '', server_label: '', user: '', agent, agent_name: agentName }
}

/** Save an agent's message text to the stash; true when it was saved. */
export async function stashThreadText(args: {
  scope: ServerScope | null
  text: string
  /** The agent's address (the message's `from`). */
  agent: string
  agentName: string
}): Promise<boolean> {
  const text = args.text.trim()
  if (!text) return false
  try {
    const r = await saveToStash({ text, source: sourceFor(args.scope, args.agent, args.agentName) })
    if (r.status !== 'saved') return false
    useToastStore.getState().addToast('Saved to your stash', 'success', 2500)
    return true
  } catch (err) {
    useToastStore.getState().addToast(`Couldn't save to your stash: ${err instanceof Error ? err.message : String(err)}`, 'error')
    return false
  }
}

/** An agent message's menu at (x, y). */
export async function openThreadMessageMenu(
  x: number,
  y: number,
  args: { scope: ServerScope | null; text: string; agent: string; agentName: string },
): Promise<void> {
  const picked = await useContextMenuStore.getState().show(x, y, [
    { id: 'stash', label: 'Save to stash', icon: <StashGlyph size={12} />, enabled: args.text.trim().length > 0 },
  ])
  if (picked === 'stash') await stashThreadText(args)
}

/** The ⋯ beside an agent message's "author · time": opens its menu. An SVG
 *  (no text), so the meta line reads the same everywhere. */
export function ThreadMessageMoreButton({
  onOpen,
}: {
  onOpen: (x: number, y: number) => void
}): JSX.Element {
  return (
    <button
      type="button"
      data-zen-message-more=""
      aria-label="Message menu"
      title="More"
      onClick={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        onOpen(r.left, r.bottom)
      }}
      className="cursor-pointer"
      style={{ background: 'none', border: 'none', padding: '0 4px', marginLeft: 4, color: 'inherit', verticalAlign: 'middle' }}
    >
      <svg width="12" height="12" viewBox="0 0 24 24" fill="currentColor" aria-hidden style={{ display: 'inline-block' }}>
        <circle cx="5" cy="12" r="2" />
        <circle cx="12" cy="12" r="2" />
        <circle cx="19" cy="12" r="2" />
      </svg>
    </button>
  )
}

/** The `stash.offer` card inside an agent's bubble. */
export function StashOfferCard({
  offer,
  scope,
  agent,
  agentName,
}: {
  offer: OverlayStashOffer
  scope: ServerScope | null
  agent: string
  agentName: string
}): JSX.Element {
  const [state, setState] = useState<'idle' | 'saving' | 'saved'>('idle')
  const save = async (): Promise<void> => {
    if (state !== 'idle') return
    setState('saving')
    const ok = await stashThreadText({ scope, text: offer.text, agent, agentName })
    setState(ok ? 'saved' : 'idle')
  }
  return (
    <div
      data-testid="stash-offer-card"
      className="mt-1 box-border flex w-full flex-col gap-1.5 border px-2 py-1.5"
      style={{
        borderColor: 'var(--thread-card-border, var(--color-border))',
        background: 'var(--thread-card-bg, var(--color-bg))',
        borderRadius: 'var(--thread-card-radius, 0px)',
        color: 'var(--thread-card-text, var(--color-text-primary))',
      }}
    >
      {offer.title ? (
        <div data-stash-offer-title="" style={{ fontWeight: 600, fontSize: '0.92em' }}>
          {offer.title}
        </div>
      ) : null}
      <div
        data-stash-offer-text=""
        className="selectable-copy"
        style={{ whiteSpace: 'pre-wrap', fontSize: '0.88em', maxHeight: '9em', overflowY: 'auto', overflowWrap: 'anywhere' }}
      >
        {offer.text}
      </div>
      <button
        type="button"
        data-testid="stash-offer-save"
        disabled={state !== 'idle'}
        onClick={() => void save()}
        className="inline-flex cursor-pointer items-center justify-center gap-1.5 self-start border px-2 py-1 text-[11px] font-medium disabled:cursor-default"
        style={{
          borderColor: 'var(--thread-card-accent, var(--color-accent))',
          borderRadius: 'var(--thread-card-radius, 0px)',
          color: 'var(--thread-card-text, var(--color-text-primary))',
          background: 'none',
          opacity: state === 'saving' ? 0.6 : 1,
        }}
      >
        <StashGlyph size={11} />
        {state === 'saved' ? 'Saved to your stash' : 'Save to stash'}
      </button>
    </div>
  )
}
