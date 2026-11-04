// Settings → General → "Show a copy button beside links on hover" (Rosson
// 2026-11-04): brings back 0.46.7's hover copy icon beside links in chat
// markdown (Threads, Zen Conversation, project chat, tickets), off by
// default. Sits next to the font rows. Per client (localStorage,
// `stores/chat-link-copy`), not a daemon setting. Right-click → "Copy link"
// works either way.

import { Toggle } from '@/components/ui'
import { useChatLinkCopyStore } from '@/stores/chat-link-copy'

export const CHAT_LINK_COPY_LABEL = 'Show a copy button beside links on hover'

export function ChatLinkCopyRow(): React.JSX.Element {
  const enabled = useChatLinkCopyStore((s) => s.enabled)
  const setEnabled = useChatLinkCopyStore((s) => s.setEnabled)
  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.link-copy-button"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">{CHAT_LINK_COPY_LABEL}</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          In chats, Threads and tickets. Right-click a link to copy it either way. On this computer only.
        </p>
      </div>
      <Toggle checked={enabled} onChange={setEnabled} aria-label={CHAT_LINK_COPY_LABEL} />
    </div>
  )
}
