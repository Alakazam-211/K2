// A live tab on the shared shell: lays out what useLiveShell reads from one
// LiveSource (whom you follow, the say field, dismissed notices, the panel).
// The data stays in the source. The scene and the timeline differ only by
// their ShellConfig (scene/scene-shell.ts, timeline/timeline-shell.ts),
// which later phases grow with the picture and the lanes. The editor uses
// the same hook with its own layout (components/Editor).

import { SampleShellSource } from '@/lib/live-view/shared/sample-shell'
import type { LiveSource } from '@/lib/live-view/shared/source'
import { AvatarStack } from './AvatarStack'
import { ChangesList } from './ChangesList'
import { BackButton, FollowControl } from './FollowControl'
import { PresenceColours } from './LiveAvatar'
import { LiveHeader } from './LiveHeader'
import { LivePanel, LiveTabFrame } from './LiveTabFrame'
import { MessagesList } from './MessagesList'
import { NoticeBar } from './NoticeBar'
import { SayBar } from './SayBar'
import type { ShellConfig } from './shell-config'
import { ShellSampleBanner } from './ShellSampleBanner'
import { useLiveShell } from './useLiveShell'
import { WatchingList } from './WatchingList'

export function LiveShellView({ source, config, width, fileName }: { source: LiveSource; config: ShellConfig; width?: number; fileName?: string }): React.JSX.Element {
  const s = useLiveShell(source, config, width)
  const { snap, narrow, people, followingId, dispatchFollow, sayTo, offline, tabState, live } = s
  const sample = source instanceof SampleShellSource ? source : null
  const name = fileName ?? sample?.sample.fileName ?? ''
  const back = s.follow.control === 'back' && !config.backInHeader ? <BackButton label={s.follow.label} disabled={offline} onBack={() => dispatchFollow({ type: 'back' })} /> : null

  const OwnPanel = config.Panel && config.Panel.when(snap) ? config.Panel.View : null
  const panel = OwnPanel ? (
    <OwnPanel snap={snap} followingId={followingId} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} offline={offline} narrow={narrow} />
  ) : (
    <>
      {config.panel.map((p) =>
        p.id === 'watching' ? (
          <WatchingList key={p.id} title={p.title} people={s.panelPeople} followingId={followingId} stale={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} />
        ) : p.id === 'messages' ? (
          <MessagesList key={p.id} title={p.title} messages={snap.messages} people={people} now={s.clock} />
        ) : (
          <ChangesList key={p.id} title={p.title} changes={s.changes} people={people} now={s.clock} onGoThere={() => dispatchFollow({ type: 'look-away' })} />
        ),
      )}
    </>
  )

  const say =
    s.showSay && sayTo ? (
      <SayBar
        toName={sayTo.name}
        address={s.sayAddress ? { label: s.sayAddress.label, title: s.sayAddress.title } : null}
        state={s.say}
        offline={offline}
        paused={tabState === 'paused'}
        onDraft={(text) => s.dispatchSay({ type: 'draft', text })}
        onSend={() => s.send(false)}
        onSendAgain={() => s.send(true)}
        onClose={s.messageTo && !followingId ? () => s.setMessageTo(null) : undefined}
      />
    ) : null
  // A view that places the say row and the panel itself gets them as nodes; the frame then holds neither.
  const placed = !!config.placesSayAndPanel
  const placedPanel =
    placed ? (
      <LivePanel open={s.panelOpen} narrow={narrow} onClose={() => s.setPanelOpen(false)}>
        {panel}
      </LivePanel>
    ) : null

  const Hero = config.Hero
  // Avatars the view draws from its own data (the scene's outliner, its activity) take the same presence colours as the stack.
  const presence = new Map(people.flatMap((p) => (p.presence && !p.self ? [[p.colour.toLowerCase(), p.presence] as const] : [])))
  return (
    <PresenceColours.Provider value={presence}>
      <div ref={s.root} className="flex h-full min-h-0 flex-col">
        <LiveTabFrame
          sample={source.kind === 'sample'}
          narrow={narrow}
          panelOpen={placed ? false : s.panelOpen}
          onClosePanel={() => s.setPanelOpen(false)}
          header={
            <LiveHeader
              fileName={name}
              fileGlyph={config.fileGlyph}
              kindLabel={narrow ? undefined : config.kindLabel}
              state={tabState}
              sentence={live ? s.sentence : null}
              sentenceRow={narrow && config.sentenceRowWhenNarrow}
              panelOpen={s.panelOpen}
              onTogglePanel={() => s.setPanelOpen((o) => !o)}
              right={
                live ? (
                  <>
                    <AvatarStack people={people} followingId={followingId} grey={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} />
                    <FollowControl view={s.follow} backHere={config.backInHeader} disabled={offline} onFollow={(id) => dispatchFollow({ type: 'follow', who: id })} onStop={() => dispatchFollow({ type: 'stop' })} onBack={() => dispatchFollow({ type: 'back' })} />
                  </>
                ) : null
              }
            />
          }
          banner={sample ? <ShellSampleBanner player={sample.player} line={config.bannerLine} /> : null}
          notice={live ? <NoticeBar notice={s.notice} ctx={{ fileName: name, kept: config.kept }} onAction={s.onNotice} /> : null}
          hero={
            <Hero
              snap={snap}
              state={tabState}
              offscreen={s.offscreen}
              onJump={s.onJump}
              jumped={s.jumped}
              back={back}
              narrow={narrow}
              follow={{ id: followingId, state: s.followState, dispatch: dispatchFollow }}
              setSayAt={s.setSayAt}
              onAction={(id) => s.onNotice(id, null)}
              fileName={name}
              refusal={s.heroRefusal}
              {...(placed ? { say, panel: placedPanel } : {})}
            />
          }
          foot={placed ? null : say}
          panel={panel}
        />
      </div>
    </PresenceColours.Provider>
  )
}
