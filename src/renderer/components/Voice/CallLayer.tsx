// The call, once per window (mounted at the app's root): the rule for how
// this window shows it, by what it carries, and full screen (CallExpanded,
// ⌃⌘V / Ctrl+Alt+V), the recording notice and Back to chat's hand-back.
// - Audio only (nobody's camera, no share): the top bar's slim bar only
//   (TopCallBar). No tile and no stage: it takes no room from the page.
// - Video (anyone's camera or a share, yours or theirs): the floating tile
//   (CallTile) in its corner, drawn here over every page.
// - Expand (the tile's, the slim bar's): the stage in the call's place
//   (CallPlace), a DM's at the top of its conversation, a voice room's at
//   the top of the page. It holds through the video stopping and starting
//   again until Float or the call ends; away from its conversation the
//   slim bar has the call, with Back to call.
// - Pop out: the call's own window, until it closes or the call ends.
// Settings open and full screen give the tile and the stage a rest.
//
// One call, one home window (stores/call-home.ts): only the home window
// draws the tile or the stage and opens video sinks (the camera's self-view
// included); every other K2 window has the slim bar with Show call here.
// Audio is the native engine's, in the app's Rust process: it plays once,
// whichever window is home.
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
import { callHasConversation, callPlaceKey, expandCallStage, floatCall, useCallDocked, useCallPlace } from '@/stores/call-place'
import { noteCallWindowClosed, useIsCallHome, watchCallHome } from '@/stores/call-home'
import { useSettingsStore } from '@/stores/settings'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import { useCallVideoLive } from './callHooks'
import VideoStage from './VideoStage'
import CallExpanded from './CallExpanded'
import PopOutError from './PopOutError'
import RecordingNotice from './RecordingNotice'
import { CallRecordingBadge } from './RecordingBadge'
import { callTitle } from '@/lib/home-chat/call-names'
import CallControls, { CallControlBar } from './CallControls'
import { FloatButton, FullScreenButton, PopOutButton, useFullScreenChordLabel } from './CallViewButtons'
import CallTile from './CallTile'

const NO_PEOPLE: CallPerson[] = []

/** The call shows in this window: the app's engine with video rooms on,
 *  this window its home (one call, one home window), the call not in its
 *  own window. */
export function useCallShowsHere(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const poppedOut = useCallPopout((s) => s.open)
  const home = useIsCallHome()
  return call !== null && native && preview && !poppedOut && home
}

/** The call's video, as this window can show it: the call has video and
 *  shows here. */
export function useCallVideoHere(): boolean {
  const call = useVoice((s) => s.call)
  const live = useCallVideoLive(video, call?.participants ?? NO_PEOPLE)
  return useCallShowsHere() && live
}

/** How the call shows in this window, by what it carries: `stage` once
 *  the person chose Expand (until Float or the call ends), the floating
 *  `tile` while it has video, else the top bar's slim `bar`. */
export type CallView = 'bar' | 'tile' | 'stage'

export function useCallView(): CallView {
  const here = useCallShowsHere()
  const videoHere = useCallVideoHere()
  const choice = useCallPlace((s) => s.choice)
  if (!here) return 'bar'
  if (choice === 'stage') return 'stage'
  return videoHere ? 'tile' : 'bar'
}

/** Settings or full screen is up: the tile and the stage rest (full screen
 *  draws the video itself, and one track has one sink). */
function useCallResting(): boolean {
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const fullScreen = useCallExpand((s) => s.expanded)
  return settingsOpen || fullScreen
}

/** The stage is up in this window, in the call's place (CallPlace): its
 *  conversation while that is on screen here, the page's top for a voice
 *  room. The top bar's slim bar gives way to it. */
export function useCallStageShown(): boolean {
  const view = useCallView()
  const call = useVoice((s) => s.call)
  const docked = useCallDocked(call)
  const resting = useCallResting()
  const placed = call !== null && (!callHasConversation(call) || docked)
  return view === 'stage' && placed && !resting
}

/** The floating tile is up in this window. The top bar's slim bar gives way
 *  to it. */
export function useCallTileShown(): boolean {
  const view = useCallView()
  const resting = useCallResting()
  return view === 'tile' && !resting
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

/** The stage's header: Float, Pop out, Full screen (with video). */
export function CallStageActions(): React.JSX.Element {
  const chord = useFullScreenChordLabel()
  const videoHere = useCallVideoHere()
  return (
    <>
      <FloatButton testId="video-stage-float" video={videoHere} />
      <PopOutButton testId="video-stage-popout" />
      {videoHere ? <FullScreenButton testId="video-stage-fullscreen" chord={chord} /> : null}
    </>
  )
}

/** The call's stage, its controls on the call's bar (as full screen and the
 *  call's own window have them). */
export function CallStageView({ call, className }: { call: VoiceCall; className?: string }): React.JSX.Element {
  const noiseRemoval = useCallNoiseRemoval()
  return (
    <VideoStage
      ctl={video}
      people={call.participants}
      title={callTitle(call)}
      className={className}
      actions={<CallStageActions />}
      status={call.recording ? <CallRecordingBadge call={call} testId="call-stage-recording" /> : undefined}
      notice={<PopOutError />}
      controls={
        <CallControlBar>
          <CallControls variant="overlay" call={call} actions={callActions} video={video} noiseRemoval={noiseRemoval} />
        </CallControlBar>
      }
    />
  )
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const noiseRemoval = useCallNoiseRemoval()
  const here = useCallVideoHere()
  const tileShown = useCallTileShown()
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
    // Closed by the OS too: the stage comes back without waiting out the
    // call window's lease.
    noteCallWindowClosed()
    loadSharePick(call.callId)
    if (takeCallHandBack(call.callId, getWindowLabel(), Date.now())) expandCallStage()
  }, [poppedOut, call.callId])
  // Which window is the call's home (one call, one home window). No window
  // left to show it: the call ends rather than going on unseen.
  const placeKey = callPlaceKey(call)
  useEffect(() => watchCallHome(placeKey, () => void voice.leave()), [placeKey])
  // The call ended (this unmounts): the next one follows what it carries.
  useEffect(() => () => floatCall(), [])
  // The call's own window counts its time from the same second.
  useEffect(() => {
    if (call.connectedAt !== null && call.callId) noteCallConnected(call.callId, call.connectedAt)
  }, [call.callId, call.connectedAt])

  // The stage itself is drawn in the call's place (CallPlace); the floating
  // tile and full screen are drawn here, over the window.
  if (!here) return null
  return (
    <>
      <CallExpanded call={call} video={video} actions={callActions} noiseRemoval={noiseRemoval} />
      {tileShown ? <CallTile call={call} video={video} actions={callActions} noiseRemoval={noiseRemoval} /> : null}
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
