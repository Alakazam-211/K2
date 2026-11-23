# Switch timing: where a switch's time goes

`window.k2SwitchTiming` (`src/renderer/lib/switch-timing.ts`) traces every switch
between agents, and between Home and Agents, in this window.

## Turn it on

In the window's web inspector console:

```js
k2SwitchTiming.enable()      // remembered: localStorage['k2.debug.switchTiming'] = '1'
k2SwitchTiming.enable(false) // off
```

Then switch. One line per switch, printed when it settles (terminal live and
Thread drawn, or 3 s after the click):

```
[switch-timing] #12 workspace 7f3c…:a91e… handler=1 store=2 restore=3 terminal_frame=21(webgl-reused ready) terminal_live=21(webgl-reused) thread_paint=19(some 25) first_content=19 cli=[POST projects/activate 9ms, GET thread 14ms]
```

All numbers are ms after the click (the input event's own time stamp).

| Stage | Meaning |
|---|---|
| `handler` | click to the switch body running (input queue delay) |
| `store` | the projects store holds the new workspace |
| `restore` | `restoreWorkspace` resolved (its tabs are in the store) |
| `page_paint` | first frame after a Home <-> Agents page change |
| `terminal_frame` | first frame a terminal on screen painted; note = painter (`webgl-reused`, `webgl-new`, `dom`) and the pane's phase |
| `terminal_live` | a terminal on screen painted with its grid socket attached (`ready`) |
| `thread_paint` | first frame the Thread drew its messages (`some N`) or its empty start |
| `first_content` | the earliest of the paint stages: click to first visible new content |
| `cli=[…]` | the `/cli` calls that started during the switch (from `k2CliTiming`'s ring), total ms and queue wait (`qN`) |

Other helpers: `k2SwitchTiming.table()`, `.summary()` (medians over the last 50
switches), `.last()`, `.list()`, `.clear()`. Recording is always on and costs a
few number writes per switch; only the console line needs the setting.

## Targets (0.46.18)

With the daemon healthy: click to first visible new content under 100 ms, click
to an interactive terminal (`terminal_live`) under 200 ms, at the median.

## What makes a switch fast (and must stay true)

- Only the pinned Chat is kept warm across workspaces (`PinnedChatRetainer`).
  Its pane stays mounted with its grid; its canvas does not.
- A hidden pane holds no WebGL context. Its surface becomes the one spare
  (`glSurfacePool.ts`, emptied, freed after 30 s) that the next pane to show
  takes, and the glyph atlas is one per font (`atlasCache.ts`). A show reusing
  them costs ~2 ms in headless WebKit against ~15-19 ms building a context,
  programs and atlas (`webglPainter.showCost.wktest.ts`).
- The tab on screen mounts first; the workspace's other tabs load after it has
  painted (`components/Terminal/backgroundTabs.ts`).
- The Thread paints its cached copy in the first frame (`threadViewCache.ts`).
- Nothing in the switch body awaits the network: `projects/activate`, the
  settings write and the session-events socket all run behind the paint.
