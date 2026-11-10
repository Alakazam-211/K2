// Settings → General → "Show a copy button beside links on hover" (Rosson
// 2026-11-04): brings back 0.46.7's hover copy icon beside links in chat
// markdown (Threads, Zen Conversation, project chat, tickets), off by
// default. Sits next to the font rows. Per client (localStorage,
// `stores/chat-link-copy`), not a daemon setting. Right-click → "Copy link"
// works either way.

import { Toggle } from '@/components/ui'
import { readChatLinkCopyButton, useChatLinkCopyStore } from '@/stores/chat-link-copy'
import { SETTING_ROW_HINT, SETTING_ROW_LABEL, SaveError, SaveNote, SettingSwitchRow, useSaveStatus } from '../controls/SettingControls'

export const CHAT_LINK_COPY_LABEL = 'Show a copy button beside links on hover'

export function ChatLinkCopyRow(): React.JSX.Element {
  const enabled = useChatLinkCopyStore((s) => s.enabled)
  const setEnabled = useChatLinkCopyStore((s) => s.setEnabled)
  const save = useSaveStatus()
  const flip = (next: boolean): void => {
    setEnabled(next)
    // Read back: blocked storage keeps it for this window only.
    if (readChatLinkCopyButton() === next) save.saved()
    else save.failed("this computer's storage is blocked, so it lasts until K2 closes")
  }
  return (
    <SettingSwitchRow
      settingId="general.link-copy-button"
      control={
        <>
          <SaveNote state={save.state} />
          <Toggle checked={enabled} onChange={flip} aria-label={CHAT_LINK_COPY_LABEL} />
        </>
      }
    >
      <div className={SETTING_ROW_LABEL}>{CHAT_LINK_COPY_LABEL}</div>
      <div className={SETTING_ROW_HINT}>In chats, Threads and tickets. Right-click a link to copy it either way.</div>
      <SaveError state={save.state} />
    </SettingSwitchRow>
  )
}
