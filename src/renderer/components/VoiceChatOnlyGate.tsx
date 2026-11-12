// The Voice & Chat only gate screen (prd-voice-chat-only-members-v1 §8.3).
//
// The one guard behind every "switch to" path (the switcher, a window
// restored on a server, New Window, a deep link, an old hint): when the
// window's own server says this login is `voice_chat`, ConnectionGate shows
// this instead of mounting the app. The daemon refuses everything else
// anyway (403 `role_required`); this keeps the window from showing a
// broken app.
//
// Desktop: Open voice rooms (switch to This computer and open the headset,
// where the server's rooms are listed) and Switch to This computer. The web
// client can't carry a call: it says so and offers Sign out.

import { useEffect, useRef, useState } from 'react'
import GateChrome from './TopBar/GateChrome'
import { voiceChatGateText } from '@/lib/voice-chat-only'

/** How often the screen re-reads the role, so an upgrade back to Member
 *  opens the server with no sign-in (§7). */
export const VOICE_CHAT_GATE_RECHECK_MS = 30_000

const BUTTON: React.CSSProperties = {
  padding: '0.5rem 1.25rem',
  fontSize: '0.85rem',
  borderRadius: 0,
  border: '1px solid var(--color-border, rgba(255,255,255,0.15))',
  background: 'var(--color-bg-elevated, rgba(255,255,255,0.05))',
  color: 'inherit',
  cursor: 'pointer',
}

export interface VoiceChatOnlyGateProps {
  /** The server's name (its saved label). */
  server: string
  /** The hosted web client (no calls, no This computer). */
  web: boolean
  onOpenVoiceRooms: () => void
  onSwitchToThisComputer: () => void
  onSignOut: () => void | Promise<void>
  /** Re-read the role (whoami). Called on mount and every
   *  {@link VOICE_CHAT_GATE_RECHECK_MS}. */
  recheck: () => void
}

export function VoiceChatOnlyGate({
  server,
  web,
  onOpenVoiceRooms,
  onSwitchToThisComputer,
  onSignOut,
  recheck,
}: VoiceChatOnlyGateProps): React.ReactElement {
  const [busy, setBusy] = useState(false)
  // The gate re-renders on every health poll: keep the latest callback in a
  // ref so the timer isn't reset (and whoami not re-sent) each time.
  const recheckRef = useRef(recheck)
  recheckRef.current = recheck
  useEffect(() => {
    recheckRef.current()
    const id = setInterval(() => recheckRef.current(), VOICE_CHAT_GATE_RECHECK_MS)
    return () => clearInterval(id)
  }, [])
  return (
    <div
      data-testid="voice-chat-only-gate"
      style={{
        width: 'var(--k2-vw)',
        height: 'var(--k2-vh)',
        display: 'flex',
        flexDirection: 'column',
        background: 'var(--color-bg, #0a0a0a)',
        color: 'var(--color-text-primary, #e0e0e0)',
        cursor: 'default',
      }}
    >
      <GateChrome />
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          flexDirection: 'column',
          gap: '1.25rem',
          fontFamily: 'var(--font-ui)',
        }}
      >
        <div style={{ fontSize: '1rem', fontWeight: 500 }}>Voice &amp; Chat only</div>
        <p
          data-testid="voice-chat-only-gate-text"
          style={{ fontSize: '0.85rem', opacity: 0.8, maxWidth: '460px', textAlign: 'center', lineHeight: 1.5 }}
        >
          {voiceChatGateText(server, web)}
        </p>
        <div style={{ display: 'flex', gap: '0.75rem' }}>
          {web ? (
            <button
              type="button"
              data-testid="voice-chat-only-sign-out"
              style={BUTTON}
              disabled={busy}
              onClick={() => {
                setBusy(true)
                void Promise.resolve(onSignOut()).finally(() => setBusy(false))
              }}
            >
              Sign out
            </button>
          ) : (
            <>
              <button type="button" data-testid="voice-chat-only-open-rooms" style={BUTTON} onClick={onOpenVoiceRooms}>
                Open voice rooms
              </button>
              <button type="button" data-testid="voice-chat-only-switch" style={BUTTON} onClick={onSwitchToThisComputer}>
                Switch to This computer
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  )
}
