// The call's video, once per window (mounted at the app's root, as K2
// Companion's `VoiceMiniTile` and `VoiceCallScreen`): while video is on in a
// call, either the call view (VideoStage, over the current page) or the
// floating tile (CallTile) in a corner. Clicking the tile opens the call
// view; its Hide returns to the page and the tile. When the video stops, both
// go and the call's controls are back in the top bar (VoiceButton).
//
// Only where calls happen: the app's native engine with video rooms on. The
// web client's calls are seats with no media, so it never shows either.
// Everything it shows comes from the voice and video stores (the engine's
// truth); the only state here is this window's open/closed call view.

import { useEffect } from 'react'
import { createPortal } from 'react-dom'
import { useSettingsStore } from '@/stores/settings'
import { useCallScreen } from '@/stores/call-screen'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import CallTile, { useCallVideoLive } from './CallTile'
import VideoStage from './VideoStage'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'
import { MicIcon, MicOffIcon, PhoneOffIcon } from '@/components/icons/K2Icons'

const NO_PEOPLE: CallPerson[] = []

/** The floating tile is up in this window: the top bar's call controls give
 *  way to the tile's strip. */
export function useCallTileShown(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const open = useCallScreen((s) => s.open)
  const live = useCallVideoLive(video, call?.participants ?? NO_PEOPLE)
  return call !== null && native && preview && live && !open && !settingsOpen
}

const actions = {
  setMuted: (m: boolean) => voice.setMuted(m),
  setDeafened: (d: boolean) => voice.setDeafened(d),
  leave: () => voice.leave(),
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const open = useCallScreen((s) => s.open)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const live = useCallVideoLive(video, call.participants)
  // The video stopped: the call view goes with it, and next time the tile
  // comes first again.
  useEffect(() => {
    if (!live && open) useCallScreen.getState().close()
  }, [live, open])
  // The call ended (this unmounts): the same.
  useEffect(() => () => useCallScreen.getState().close(), [])

  if (open && live) {
    return createPortal(
      <VideoStage
        ctl={video}
        people={call.participants}
        title={call.roomName}
        onHide={() => useCallScreen.getState().close()}
        footerStart={
          <CallPanelButton testId="video-stage-mute" label={call.muted ? 'Unmute' : 'Mute'} on={call.muted} onPress={() => voice.setMuted(!call.muted)}>
            {call.muted ? <MicOffIcon size={CALL_PANEL_ICON} /> : <MicIcon size={CALL_PANEL_ICON} />}
          </CallPanelButton>
        }
        footer={
          <CallPanelButton testId="video-stage-leave" label="Leave call" danger onPress={() => void voice.leave()}>
            <PhoneOffIcon size={CALL_PANEL_ICON} />
          </CallPanelButton>
        }
      />,
      document.body,
    )
  }
  if (live && !settingsOpen) return <CallTile call={call} video={video} actions={actions} />
  return null
}

export default function CallLayer(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  if (!call || !native || !preview) return null
  return <InCall call={call} />
}
