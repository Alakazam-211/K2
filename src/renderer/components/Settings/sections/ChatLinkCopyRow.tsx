// Settings → General → "Show a copy button beside links on hover" (Rosson
// 2026-11-04): brings back 0.46.7's hover copy icon beside links in chat
// markdown (Threads, Zen Conversation, project chat, tickets), off by
// default. Sits next to the font rows. Per client (localStorage,
// `stores/chat-link-copy`), not a daemon setting. Right-click → "Copy link"
// works either way.

import { Toggle } from '@/components/ui'
import { useChatLinkCopyStore } from '@/stores/chat-link-copy'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SettingSwitchRow } from '../controls/SettingControls'

export const CHAT_LINK_COPY_LABEL = 'Show a copy button beside links on hover'

export function ChatLinkCopyRow(): React.JSX.Element {
  const enabled = useChatLinkCopyStore((s) => s.enabled)
  const setEnabled = useChatLinkCopyStore((s) => s.setEnabled)
  return (
    <SettingSwitchRow
      settingId="general.link-copy-button"
      control={<Toggle checked={enabled} onChange={setEnabled} aria-label={CHAT_LINK_COPY_LABEL} />}
    >
      <div className={SETTING_ROW_LABEL}>{CHAT_LINK_COPY_LABEL}</div>
      <div className={SETTING_ROW_HINT}>In chats, Threads and tickets. Right-click a link to copy it either way.</div>
    </SettingSwitchRow>
  )
}
