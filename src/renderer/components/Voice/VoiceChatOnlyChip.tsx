// The "Voice & Chat only" chip (prd-voice-chat-only-members-v1 §8.2): on a
// server whose login may only join voice rooms, in the headset list and
// Settings → Connections. The tooltip says why the server never opens.

import { VOICE_CHAT_CHIP, VOICE_CHAT_LINE } from '@/lib/voice-chat-only'

export function VoiceChatOnlyChip(): React.JSX.Element {
  return (
    <span
      data-testid="voice-chat-only-chip"
      title={VOICE_CHAT_LINE}
      className="shrink-0 border border-[var(--color-border)] px-1 py-px text-[9px] normal-case tracking-normal text-[var(--color-text-secondary)]"
    >
      {VOICE_CHAT_CHIP}
    </span>
  )
}
