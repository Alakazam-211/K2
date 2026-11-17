// The call's video, once per window (mounted at the app's root): while video
// is on in a call, its stage (VideoStage: the tiles, with the call's
// controls), unless this window has it folded into the slim bar (Compact),
// full screen (CallExpanded, ⌃⌘V / Ctrl+Alt+V), or the call is in its own
// window (Pop out). There is no floating tile and no corner. When the video
// stops, the stage goes and the call's controls are back in the top bar
// (VoiceButton).
//
// Only where calls happen: the app's native engine with video rooms on. The
// web client's calls are seats with no media, so it never shows either.
// Everything it shows comes from the voice and video stores (the engine's
// truth); the only state here is this window's view of it. Back to chat in
// the call's own window brings the call back to the window that popped it
// out, as its stage with the same share in the middle.

import { useEffect, useMemo, useRef } from 'react'
import { createPortal } from 'react-dom'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { isCallExpandChord, isCallWindow, noteCallConnected, takeCallHandBack } from '@/lib/voice/call-view'
import { getWindowLabel } from '@/lib/window-chrome'
import { toggleCallExpand, useCallExpand } from '@/stores/call-expand'
import { loadSharePick } from '@/stores/call-share'
import { useCallPopout, watchCallPopout } from '@/stores/call-popout'
import { expandCallStage, useCallPlace } from '@/stores/call-place'
import { useSettingsStore } from '@/stores/settings'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import { useCallVideoLive } from './callHooks'
import VideoStage from './VideoStage'
import CallExpanded from './CallExpanded'
import RecordingNotice from './RecordingNotice'
import { callTitle } from '@/lib/home-chat/call-names'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'
import { CompactButton, FullScreenButton, PopOutButton, useFullScreenChordLabel } from './CallViewButtons'
import { MicIcon, MicOffIcon, PhoneOffIcon } from '@/components/icons/K2Icons'

const NO_PEOPLE: CallPerson[] = []

/** The call's video, as this window can show it: the call has video and
 *  isn't in its own window. */
export function useCallVideoHere(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const live = useCallVideoLive(video, call?.participants ?? NO_PEOPLE)
  const poppedOut = useCallPopout((s) => s.open)
  return call !== null && native && preview && live && !poppedOut
}

/** The stage is up in this window: the top bar's call controls give way to
 *  the stage's own. */
export function useCallStageShown(): boolean {
  const here = useCallVideoHere()
  const compact = useCallPlace((s) => s.compact)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  return here && !compact && !settingsOpen
}

/** This webview is the call's own window (it draws the call itself). */
export function inCallWindow(): boolean {
  return isCallWindow(window.location.hash, getWindowLabel())
}

/** The Full screen chord, while the call has video in this window. */
export function useCallExpandChord(active: boolean): void {
  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.repeat || !isCallExpandChord(e, useSettingsStore.getState().keybindings ?? {}, isMacPlatform())) return
      e.preventDefault()
      e.stopPropagation()
      toggleCallExpand()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active])
}

export const callActions = {
  setMuted: (m: boolean) => voice.setMuted(m),
  setDeafened: (d: boolean) => voice.setDeafened(d),
  leave: () => voice.leave(),
  startRecording: () => voice.startRecording(),
  stopRecording: () => voice.stopRecording(),
  setNoiseRemoval: (on: boolean) => voice.setNoiseRemoval(on),
}

/** The wave button's state: Settings → Voice → Noise removal (on unless
 *  turned off), with the native engine only; null on a seat-only call. */
export function useCallNoiseRemoval(): boolean | null {
  return useVoice((s) => (s.engineKind === 'native' ? s.prefs.noiseRemoval : null))
}

/** K2's recording notice, in every window, video or not. */
function NoticeLayer({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const agentReaders = useVoice((s) => s.rooms[call.hostKey]?.list?.agentReaders === true)
  if (!call.recordingNotice) return null
  return createPortal(
    <RecordingNotice call={call} agentReaders={agentReaders} onAnswer={(a) => void voice.answerRecordingNotice(a)} />,
    document.body,
  )
}

/** The stage's header: Pop out, Full screen, Compact. */
export function CallStageActions(): React.JSX.Element {
  const chord = useFullScreenChordLabel()
  return (
    <>
      <PopOutButton testId="video-stage-popout" />
      <FullScreenButton testId="video-stage-fullscreen" chord={chord} />
      <CompactButton testId="video-stage-compact" compact={false} />
    </>
  )
}

/** The call's stage, with Mute and Leave call beside its camera and share. */
export function CallStageView({ call, className }: { call: VoiceCall; className?: string }): React.JSX.Element {
  return (
    <VideoStage
      ctl={video}
      people={call.participants}
      title={callTitle(call)}
      className={className}
      actions={<CallStageActions />}
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
    />
  )
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const noiseRemoval = useCallNoiseRemoval()
  const here = useCallVideoHere()
  const stageShown = useCallStageShown()
  const expanded = useCallExpand((s) => s.expanded)
  const poppedOut = useCallPopout((s) => s.open)
  useCallExpandChord(here)
  // The call's window closed. Back to chat (not the OS's close) hands the
  // call back to the window that popped it out: its stage opens, and every
  // window takes up the share that was in the middle there.
  const wasOut = useRef(poppedOut)
  useEffect(() => {
    const was = wasOut.current
    wasOut.current = poppedOut
    if (!was || poppedOut) return
    loadSharePick(call.callId)
    if (takeCallHandBack(call.callId, getWindowLabel(), Date.now())) expandCallStage()
  }, [poppedOut, call.callId])
  // The call ended (this unmounts): next call starts with its stage open.
  useEffect(() => () => expandCallStage(), [])
  // The call's own window counts its time from the same second.
  useEffect(() => {
    if (call.connectedAt !== null && call.callId) noteCallConnected(call.callId, call.connectedAt)
  }, [call.callId, call.connectedAt])

  if (!here) return null
  const host = <CallExpanded call={call} video={video} actions={callActions} noiseRemoval={noiseRemoval} />
  // Full screen: the video is drawn there; the stage waits (one sink per
  // track).
  if (expanded || !stageShown) return host
  return (
    <>
      {host}
      {createPortal(
        <div className="fixed bottom-3 right-3 z-40 flex max-h-[calc(var(--k2-vh)*0.8)] w-[min(380px,calc(var(--k2-vw)-24px))] flex-col">
          <CallStageView call={call} />
        </div>,
        document.body,
      )}
    </>
  )
}

export default function CallLayer(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const callWindow = useMemo(inCallWindow, [])
  // Whether the call's window is open, followed from the first render (it
  // may have opened before this window did).
  useEffect(() => (callWindow ? undefined : watchCallPopout()), [callWindow])
  const notice = call && native ? <NoticeLayer call={call} /> : null
  if (callWindow || !call || !native || !preview) return notice
  return (
    <>
      {notice}
      <InCall call={call} />
    </>
  )
}
