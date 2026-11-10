// Each feature's one icon (Rosson 2026-11-09: "The icon for board needs to
// be a different icon. It can't be the same one as Design."). The top bar's
// page tabs, the tab strip, the + menu and the split-pane rows all read their
// icon here, so two features can't drift onto the same one; the test beside
// this file fails if two entries share a component or a drawing.
//
// The page tabs are K2Icons (16px A3 set). The tab kinds keep their own
// glyphs where they have them (the shell window, the diff, the globe), and a
// Board file's tab draws `board` through `SetiFileIcon`.

import { BrowserGlobe } from '@/components/TabBar/BrowserTabGlyph'
import { ChangesTabIcon } from '@/components/TabBar/ChangesTabIcon'
import { ShellTabIcon } from '@/components/TabBar/ShellTabIcon'
import {
  AgentsIcon,
  BoardIcon,
  HomeIcon,
  OrgChartIcon,
  PaletteIcon,
  ProjectsIcon,
  SettingsIcon,
  SmartphoneIcon,
  TicketsIcon,
  WikiIcon,
  type K2IconProps,
} from './K2Icons'

export const FEATURE_ICONS = {
  // Page tabs (the top bar's page selector).
  home: HomeIcon,
  agents: AgentsIcon,
  projects: ProjectsIcon,
  tickets: TicketsIcon,
  wiki: WikiIcon,
  orgChart: OrgChartIcon,
  settings: SettingsIcon,
  // Tab kinds (the tab strip, the + menu, the split-pane rows).
  terminal: ShellTabIcon,
  browser: BrowserGlobe,
  design: PaletteIcon,
  board: BoardIcon,
  mobile: SmartphoneIcon,
  changes: ChangesTabIcon,
} as const satisfies Record<string, (props: K2IconProps) => React.JSX.Element>

export type FeatureIconId = keyof typeof FEATURE_ICONS
