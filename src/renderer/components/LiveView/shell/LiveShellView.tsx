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
import { LiveHeader } from './LiveHeader'
import { LiveTabFrame } from './LiveTabFrame'
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

  const panel = (
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

  const Hero = config.Hero
  return (
    <div ref={s.root} className="flex h-full min-h-0 flex-col">
      <LiveTabFrame
        sample={source.kind === 'sample'}
        narrow={narrow}
        panelOpen={s.panelOpen}
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
        hero={<Hero snap={snap} state={tabState} offscreen={s.offscreen} onJump={s.onJump} jumped={s.jumped} back={back} narrow={narrow} />}
        foot={
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
            />
          ) : null
        }
        panel={panel}
      />
    </div>
  )
}
