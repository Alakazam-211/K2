// The live 3D scene tab on the shared shell: the header, the notices and the
// say row while following; the side panel is the scene's own (S2: Activity,
// People and holds, Outliner, Properties, from the scene model); the hero is
// the picture on its dark stage (S3). Later phases (build-live-scene.md
// S4–S8) add the overlays, following and the real sources.

import { SCENE_VIEW } from '@/lib/live-view/scene/view-address'
import { personById } from '@/lib/live-view/shared/people'
import { CubeGlyph } from '../shell/glyphs'
import type { ShellConfig } from '../shell/shell-config'
import { SceneHero } from './SceneHero'
import { ScenePanel, scenePanelFor } from './ScenePanel'

export const SCENE_SHELL: ShellConfig = {
  kind: 'scene',
  kindLabel: '3D scene',
  fileGlyph: <CubeGlyph size={15} />,
  bannerLine: 'Played from K2’s sample scene; nothing here comes from the file you opened.',
  panel: [
    { id: 'changes', title: 'Activity' },
    { id: 'messages', title: 'Messages' },
    { id: 'watching', title: 'People and holds' },
  ],
  narrowBelow: 1000,
  sentenceRowWhenNarrow: false,
  backInHeader: true,
  sayAlways: false,
  kept: 'who’s here, the changes and your view',
  sayAddress: (snap, to) => {
    const p = personById(snap.people, to)
    if (!p) return null
    const label = p.place ? (p.self ? p.place : `${p.name}’s view`) : 'the scene'
    return { label, title: p.place ?? 'the whole scene', address: p.viewAddress ?? SCENE_VIEW }
  },
  Hero: SceneHero,
  // The scene's own panel (Activity, People and holds, Outliner, Properties) for
  // snapshots that carry the scene; the shell's sections otherwise.
  Panel: { when: scenePanelFor, View: ScenePanel },
}
