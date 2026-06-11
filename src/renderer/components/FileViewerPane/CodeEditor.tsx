import { useEffect, useRef } from 'react'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, rectangularSelection, highlightWhitespace, gutter, GutterMarker, Decoration, ViewPlugin, ViewUpdate, type DecorationSet, scrollPastEnd } from '@codemirror/view'
import { EditorState, Compartment, RangeSet, type Extension } from '@codemirror/state'
import type { EditorSettingsBackend, EditorThemeId } from '@shared/types'
import { invoke } from '@tauri-apps/api/core'
// Plan B — git diff (for the gutter) is host-aware daemon data: route it
// through the `/cli/git/*` HTTP layer (local OR remote). GET params are
// snake_case (`path`, `file_path`). `format_file` stays on Tauri invoke
// (host-only formatter).
import { daemonCliGet } from '@/lib/daemon-cli'
import { useSettingsStore } from '@/stores/settings'
import { useCustomThemesStore } from '@/stores/custom-themes'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { oneDarkHighlightStyle } from '@codemirror/theme-one-dark'
import { tags } from '@lezer/highlight'
import {
  syntaxHighlighting, defaultHighlightStyle, bracketMatching, indentOnInput,
  foldGutter, codeFolding, foldKeymap, StreamLanguage, indentUnit, syntaxTree,
  HighlightStyle,
} from '@codemirror/language'
import { search, searchKeymap, highlightSelectionMatches, selectNextOccurrence } from '@codemirror/search'
import { autocompletion, closeBrackets, closeBracketsKeymap } from '@codemirror/autocomplete'
import { indentationMarkers } from '@replit/codemirror-indentation-markers'
import { showMinimap } from '@replit/codemirror-minimap'
import { vim } from '@replit/codemirror-vim'

// ── Official language packages ────────────────────────────────────────
import { javascript } from '@codemirror/lang-javascript'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { css } from '@codemirror/lang-css'
import { html } from '@codemirror/lang-html'
import { json } from '@codemirror/lang-json'
import { markdown } from '@codemirror/lang-markdown'
import { cpp } from '@codemirror/lang-cpp'
import { java } from '@codemirror/lang-java'
import { go } from '@codemirror/lang-go'
import { sql } from '@codemirror/lang-sql'
import { xml } from '@codemirror/lang-xml'
import { yaml } from '@codemirror/lang-yaml'
import { php } from '@codemirror/lang-php'

// ── Legacy-mode languages (proper grammars for previously faked langs) ─
import { ruby } from '@codemirror/legacy-modes/mode/ruby'
import { shell } from '@codemirror/legacy-modes/mode/shell'
import { dockerFile } from '@codemirror/legacy-modes/mode/dockerfile'
import { toml } from '@codemirror/legacy-modes/mode/toml'
import { lua } from '@codemirror/legacy-modes/mode/lua'
import { swift } from '@codemirror/legacy-modes/mode/swift'
import { haskell } from '@codemirror/legacy-modes/mode/haskell'
import { erlang } from '@codemirror/legacy-modes/mode/erlang'
import { perl } from '@codemirror/legacy-modes/mode/perl'
import { r } from '@codemirror/legacy-modes/mode/r'
import { powerShell } from '@codemirror/legacy-modes/mode/powershell'
import { clojure } from '@codemirror/legacy-modes/mode/clojure'
import { scheme } from '@codemirror/legacy-modes/mode/scheme'
import { commonLisp } from '@codemirror/legacy-modes/mode/commonlisp'
import { elm } from '@codemirror/legacy-modes/mode/elm'
import { protobuf } from '@codemirror/legacy-modes/mode/protobuf'
import { nginx } from '@codemirror/legacy-modes/mode/nginx'
import { cmake } from '@codemirror/legacy-modes/mode/cmake'
import { groovy } from '@codemirror/legacy-modes/mode/groovy'
import { pascal } from '@codemirror/legacy-modes/mode/pascal'
import { fortran } from '@codemirror/legacy-modes/mode/fortran'
import { coffeeScript } from '@codemirror/legacy-modes/mode/coffeescript'
import { crystal } from '@codemirror/legacy-modes/mode/crystal'
import { julia } from '@codemirror/legacy-modes/mode/julia'
import { sass as sassMode } from '@codemirror/legacy-modes/mode/sass'
import { diff } from '@codemirror/legacy-modes/mode/diff'
import { kotlin, scala, csharp, dart } from '@codemirror/legacy-modes/mode/clike'
import { properties } from '@codemirror/legacy-modes/mode/properties'
import { wast } from '@codemirror/legacy-modes/mode/wast'
import { octave } from '@codemirror/legacy-modes/mode/octave'
import { vb } from '@codemirror/legacy-modes/mode/vb'
import { oCaml, fSharp } from '@codemirror/legacy-modes/mode/mllike'
import { d } from '@codemirror/legacy-modes/mode/d'
import { haxe } from '@codemirror/legacy-modes/mode/haxe'
import { pug } from '@codemirror/legacy-modes/mode/pug'
import { stylus } from '@codemirror/legacy-modes/mode/stylus'
import { tcl } from '@codemirror/legacy-modes/mode/tcl'
import { verilog } from '@codemirror/legacy-modes/mode/verilog'
import { vhdl } from '@codemirror/legacy-modes/mode/vhdl'
import { puppet } from '@codemirror/legacy-modes/mode/puppet'
import { jinja2 } from '@codemirror/legacy-modes/mode/jinja2'
import { smalltalk } from '@codemirror/legacy-modes/mode/smalltalk'
import { gherkin } from '@codemirror/legacy-modes/mode/gherkin'
import { cobol } from '@codemirror/legacy-modes/mode/cobol'
import { cypher } from '@codemirror/legacy-modes/mode/cypher'
import { dylan } from '@codemirror/legacy-modes/mode/dylan'
import { eiffel } from '@codemirror/legacy-modes/mode/eiffel'
import { forth } from '@codemirror/legacy-modes/mode/forth'
import { gas } from '@codemirror/legacy-modes/mode/gas'
import { http } from '@codemirror/legacy-modes/mode/http'
import { idl } from '@codemirror/legacy-modes/mode/idl'
import { liveScript } from '@codemirror/legacy-modes/mode/livescript'
import { mathematica } from '@codemirror/legacy-modes/mode/mathematica'
import { oz } from '@codemirror/legacy-modes/mode/oz'
import { q } from '@codemirror/legacy-modes/mode/q'
import { sas } from '@codemirror/legacy-modes/mode/sas'
import { spreadsheet } from '@codemirror/legacy-modes/mode/spreadsheet'
import { stex } from '@codemirror/legacy-modes/mode/stex'
import { turtle } from '@codemirror/legacy-modes/mode/turtle'
import { webIDL } from '@codemirror/legacy-modes/mode/webidl'
import { xQuery } from '@codemirror/legacy-modes/mode/xquery'
import { z80 } from '@codemirror/legacy-modes/mode/z80'
import { nsis } from '@codemirror/legacy-modes/mode/nsis'
import { dtd } from '@codemirror/legacy-modes/mode/dtd'
import { apl } from '@codemirror/legacy-modes/mode/apl'
import { sparql } from '@codemirror/legacy-modes/mode/sparql'
import { gdscript } from '@gdquest/codemirror-gdscript'
import { zig } from 'codemirror-lang-zig'
import { hcl } from 'codemirror-lang-hcl'
import { nix } from '@replit/codemirror-lang-nix'
import { solidity } from '@replit/codemirror-lang-solidity'
import { elixir } from 'codemirror-lang-elixir'
import { svelte } from '@replit/codemirror-lang-svelte'

// ── Language detection ──────────────────────────────────────────────

type LanguageFn = () => Extension

const sl = (mode: Parameters<typeof StreamLanguage.define>[0]) => () => StreamLanguage.define(mode)

const EXT_LANG_MAP: Record<string, LanguageFn> = {
  // JavaScript / TypeScript
  js: () => javascript(), mjs: () => javascript(), cjs: () => javascript(),
  jsx: () => javascript({ jsx: true }),
  ts: () => javascript({ typescript: true }), mts: () => javascript({ typescript: true }), cts: () => javascript({ typescript: true }),
  tsx: () => javascript({ jsx: true, typescript: true }),
  // Web
  html: () => html(), htm: () => html(), svelte: () => svelte(), vue: () => html(), astro: () => html(),
  css: () => css(), scss: () => css(), less: () => css(),
  sass: sl(sassMode),
  // Data / Config
  json: () => json(), jsonc: () => json(), json5: () => json(),
  yaml: () => yaml(), yml: () => yaml(),
  toml: sl(toml),
  xml: () => xml(), svg: () => xml(), xsl: () => xml(), xsd: () => xml(), plist: () => xml(),
  ini: sl(properties), properties: sl(properties), conf: sl(properties),
  // Systems
  rs: () => rust(),
  go: () => go(),
  c: () => cpp(), cpp: () => cpp(), h: () => cpp(), hpp: () => cpp(), cc: () => cpp(), cxx: () => cpp(), hxx: () => cpp(),
  swift: sl(swift),
  // Scripting
  py: () => python(), pyw: () => python(), pyi: () => python(),
  rb: sl(ruby), rake: sl(ruby), gemspec: sl(ruby),
  lua: sl(lua),
  sh: sl(shell), bash: sl(shell), zsh: sl(shell), fish: sl(shell),
  pl: sl(perl), pm: sl(perl),
  r: sl(r), R: sl(r),
  ps1: sl(powerShell), psm1: sl(powerShell), psd1: sl(powerShell),
  coffee: sl(coffeeScript),
  // JVM
  java: () => java(),
  kt: sl(kotlin), kts: sl(kotlin),
  scala: sl(scala),
  groovy: sl(groovy), gradle: sl(groovy),
  clj: sl(clojure), cljs: sl(clojure), cljc: sl(clojure), edn: sl(clojure),
  // .NET
  cs: sl(csharp),
  dart: sl(dart),
  vb: sl(vb), vbs: sl(vb),
  fs: sl(fSharp), fsx: sl(fSharp), fsi: sl(fSharp),
  // Functional
  hs: sl(haskell), lhs: sl(haskell),
  erl: sl(erlang), hrl: sl(erlang),
  ex: () => elixir(), exs: () => elixir(),
  elm: sl(elm),
  ml: sl(oCaml), mli: sl(oCaml),
  scm: sl(scheme), rkt: sl(scheme),
  lisp: sl(commonLisp), cl: sl(commonLisp),
  jl: sl(julia),
  // Docs
  md: () => markdown(), mdx: () => markdown(), markdown: () => markdown(),
  // SQL
  sql: () => sql(),
  // PHP
  php: () => php(),
  // DevOps / Infra
  proto: sl(protobuf),
  cmake: sl(cmake),
  // Scientific
  m: sl(octave), // MATLAB / Octave
  f: sl(fortran), f90: sl(fortran), f95: sl(fortran),
  pas: sl(pascal),
  // Crystal
  cr: sl(crystal),
  // Game dev
  gd: () => gdscript(),
  // Modern systems
  zig: () => zig(),
  // Smart contracts
  sol: () => solidity(),
  // Infra / Config
  tf: () => hcl(), hcl: () => hcl(), tfvars: () => hcl(),
  nix: () => nix(),
  // D language
  d: sl(d),
  // Haxe
  hx: sl(haxe),
  // Templates
  pug: sl(pug), jade: sl(pug),
  styl: sl(stylus),
  j2: sl(jinja2), jinja: sl(jinja2), jinja2: sl(jinja2),
  // Hardware description
  v: sl(verilog), sv: sl(verilog),
  vhd: sl(vhdl), vhdl: sl(vhdl),
  // Other
  tcl: sl(tcl),
  pp: sl(puppet),
  st: sl(smalltalk),
  feature: sl(gherkin),
  // WebAssembly
  wat: sl(wast), wast: sl(wast),
  // Diff / Patch
  diff: sl(diff), patch: sl(diff),
  // Additional languages
  cob: sl(cobol), cbl: sl(cobol),
  ls: sl(liveScript),
  wl: sl(mathematica), nb: sl(mathematica),
  apl: sl(apl),
  sas: sl(sas),
  oz: sl(oz),
  forth: sl(forth), fth: sl(forth), '4th': sl(forth),
  s: sl(gas), S: sl(gas),
  eif: sl(eiffel),
  dylan: sl(dylan),
  nsi: sl(nsis), nsh: sl(nsis),
  tex: sl(stex), latex: sl(stex), sty: sl(stex),
  dtd: sl(dtd),
  ttl: sl(turtle),
  sparql: sl(sparql), rq: sl(sparql),
  xq: sl(xQuery), xqm: sl(xQuery), xquery: sl(xQuery),
  webidl: sl(webIDL),
  z80: sl(z80), asm: sl(z80),
  q: sl(q),
  cypher: sl(cypher), cql: sl(cypher),
  http: sl(http),
}

const FILENAME_LANG_MAP: Record<string, LanguageFn> = {
  'Dockerfile': sl(dockerFile),
  'Makefile': sl(shell),
  'Gemfile': sl(ruby),
  'Rakefile': sl(ruby),
  'CMakeLists.txt': sl(cmake),
  'Vagrantfile': sl(ruby),
  'Brewfile': sl(ruby),
  'nginx.conf': sl(nginx),
  '.gitignore': sl(properties),
  '.gitattributes': sl(properties),
  '.editorconfig': sl(properties),
  '.env': sl(properties),
  '.prettierrc': () => json(),
  '.eslintrc': () => json(),
  '.babelrc': () => json(),
  'tsconfig.json': () => json(),
  'package.json': () => json(),
  'Cargo.toml': sl(toml),
  'pyproject.toml': sl(toml),
  'Pipfile': sl(toml),
}

/** Markdown files always soft-wrap in the editor regardless of the
 *  global editor.wordWrap setting — prose shouldn't scroll
 *  horizontally. Code files keep the user's chosen behavior. */
function isMarkdownPath(filePath: string): boolean {
  const lower = filePath.toLowerCase()
  return lower.endsWith('.md') || lower.endsWith('.markdown') || lower.endsWith('.mdx')
}

function getLanguageExtension(filePath: string): Extension | null {
  const name = filePath.split('/').pop() || ''

  // Check full filename first
  if (FILENAME_LANG_MAP[name]) return FILENAME_LANG_MAP[name]()

  // Dockerfile variants (Dockerfile.dev, Dockerfile.prod)
  if (name.startsWith('Dockerfile')) return StreamLanguage.define(dockerFile)

  // .env files (.env.local, .env.production, etc.)
  if (name.startsWith('.env')) return StreamLanguage.define(properties)

  // Check extension
  const parts = name.split('.')
  if (parts.length > 1) {
    const ext = parts.pop()!.toLowerCase()
    if (EXT_LANG_MAP[ext]) return EXT_LANG_MAP[ext]()
  }

  return null
}

/** Get a human-readable language name for the status bar */
export function getLanguageName(filePath: string): string {
  const name = filePath.split('/').pop() || ''
  const ext = name.split('.').pop()?.toLowerCase() || ''

  const names: Record<string, string> = {
    js: 'JavaScript', mjs: 'JavaScript', cjs: 'JavaScript',
    jsx: 'JSX', tsx: 'TSX',
    ts: 'TypeScript', mts: 'TypeScript', cts: 'TypeScript',
    py: 'Python', pyw: 'Python', pyi: 'Python',
    rs: 'Rust', go: 'Go', c: 'C', cpp: 'C++', h: 'C/C++ Header', hpp: 'C++ Header',
    java: 'Java', kt: 'Kotlin', kts: 'Kotlin', scala: 'Scala',
    cs: 'C#', fs: 'F#', vb: 'Visual Basic',
    rb: 'Ruby', lua: 'Lua', swift: 'Swift',
    sh: 'Shell', bash: 'Bash', zsh: 'Zsh',
    html: 'HTML', htm: 'HTML', css: 'CSS', scss: 'SCSS', less: 'Less', sass: 'Sass',
    json: 'JSON', jsonc: 'JSONC', yaml: 'YAML', yml: 'YAML', toml: 'TOML', xml: 'XML',
    md: 'Markdown', mdx: 'MDX', sql: 'SQL', php: 'PHP',
    hs: 'Haskell', erl: 'Erlang', ex: 'Elixir', elm: 'Elm',
    clj: 'Clojure', scm: 'Scheme', lisp: 'Lisp', jl: 'Julia',
    r: 'R', R: 'R', m: 'MATLAB', pl: 'Perl',
    ps1: 'PowerShell', coffee: 'CoffeeScript', cr: 'Crystal',
    proto: 'Protocol Buffers', diff: 'Diff',
    svelte: 'Svelte', vue: 'Vue', astro: 'Astro',
    groovy: 'Groovy', gradle: 'Gradle',
    gd: 'GDScript', fs: 'F#', fsx: 'F#', ml: 'OCaml', mli: 'OCaml',
    d: 'D', hx: 'Haxe', pug: 'Pug', jade: 'Pug', styl: 'Stylus',
    tcl: 'Tcl', v: 'Verilog', sv: 'SystemVerilog', vhd: 'VHDL', vhdl: 'VHDL',
    pp: 'Puppet', st: 'Smalltalk', feature: 'Gherkin',
    j2: 'Jinja2', jinja: 'Jinja2',
    cob: 'COBOL', cbl: 'COBOL', ls: 'LiveScript',
    wl: 'Mathematica', nb: 'Mathematica', apl: 'APL', sas: 'SAS',
    oz: 'Oz', forth: 'Forth', eif: 'Eiffel', dylan: 'Dylan',
    nsi: 'NSIS', tex: 'LaTeX', latex: 'LaTeX',
    ttl: 'Turtle', sparql: 'SPARQL', xq: 'XQuery',
    z80: 'Z80 Assembly', asm: 'Assembly',
    q: 'Q/KDB+', cypher: 'Cypher', http: 'HTTP',
    zig: 'Zig', sol: 'Solidity', tf: 'Terraform', hcl: 'HCL', tfvars: 'Terraform',
    nix: 'Nix', svelte: 'Svelte',
    ex: 'Elixir', exs: 'Elixir',
    graphql: 'GraphQL', gql: 'GraphQL', prisma: 'Prisma',
  }

  if (name === 'Dockerfile' || name.startsWith('Dockerfile.')) return 'Dockerfile'
  if (name === 'Makefile') return 'Makefile'
  if (name.startsWith('.env')) return 'Environment'

  return names[ext] || ext.toUpperCase() || 'Plain Text'
}

// ── Editor theme definitions ─────────────────────────────────────────

interface ThemeColors {
  bg: string
  fg: string
  gutterBg: string
  gutterFg: string
  gutterBorder: string
  activeLine: string
  selection: string
  cursor: string
  bracket: string
  bracketOutline: string
  searchMatch: string
  searchMatchSelected: string
  accent: string
  panelBg: string
  panelBorder: string
  tooltipBg: string
  tooltipBorder: string
}

const THEME_COLORS: Record<string, ThemeColors> = {
  'k2so-dark': {
    bg: '#0a0a0a', fg: '#e4e4e7', gutterBg: '#0a0a0a', gutterFg: '#555', gutterBorder: '#1a1a1a',
    activeLine: '#ffffff08', selection: '#3b82f633', cursor: '#3b82f6', bracket: '#3b82f633',
    bracketOutline: '#3b82f666', searchMatch: '#b5890066', searchMatchSelected: '#b58900aa',
    accent: '#3b82f6', panelBg: '#141414', panelBorder: '#2a2a2a', tooltipBg: '#141414', tooltipBorder: '#2a2a2a',
  },
  'one-dark': {
    bg: '#282c34', fg: '#abb2bf', gutterBg: '#282c34', gutterFg: '#495162', gutterBorder: '#3b4048',
    activeLine: '#2c313c', selection: '#3e4451', cursor: '#528bff', bracket: '#528bff33',
    bracketOutline: '#528bff66', searchMatch: '#d19a6666', searchMatchSelected: '#d19a66aa',
    accent: '#61afef', panelBg: '#21252b', panelBorder: '#3b4048', tooltipBg: '#21252b', tooltipBorder: '#3b4048',
  },
  'dracula': {
    bg: '#282a36', fg: '#f8f8f2', gutterBg: '#282a36', gutterFg: '#6272a4', gutterBorder: '#44475a',
    activeLine: '#44475a44', selection: '#44475a', cursor: '#f8f8f2', bracket: '#bd93f933',
    bracketOutline: '#bd93f966', searchMatch: '#50fa7b44', searchMatchSelected: '#50fa7b88',
    accent: '#bd93f9', panelBg: '#21222c', panelBorder: '#44475a', tooltipBg: '#21222c', tooltipBorder: '#44475a',
  },
  'nord': {
    bg: '#2e3440', fg: '#d8dee9', gutterBg: '#2e3440', gutterFg: '#4c566a', gutterBorder: '#3b4252',
    activeLine: '#3b425244', selection: '#434c5e', cursor: '#88c0d0', bracket: '#88c0d033',
    bracketOutline: '#88c0d066', searchMatch: '#ebcb8b44', searchMatchSelected: '#ebcb8b88',
    accent: '#88c0d0', panelBg: '#292e39', panelBorder: '#3b4252', tooltipBg: '#292e39', tooltipBorder: '#3b4252',
  },
  'github-dark': {
    bg: '#0d1117', fg: '#c9d1d9', gutterBg: '#0d1117', gutterFg: '#484f58', gutterBorder: '#21262d',
    activeLine: '#161b2244', selection: '#264f78', cursor: '#58a6ff', bracket: '#58a6ff33',
    bracketOutline: '#58a6ff66', searchMatch: '#e3b34166', searchMatchSelected: '#e3b341aa',
    accent: '#58a6ff', panelBg: '#161b22', panelBorder: '#30363d', tooltipBg: '#161b22', tooltipBorder: '#30363d',
  },
  'gruvbox-dark': {
    bg: '#282828', fg: '#ebdbb2', gutterBg: '#282828', gutterFg: '#7c6f64', gutterBorder: '#3c3836',
    activeLine: '#3c383644', selection: '#504945', cursor: '#83a598', bracket: '#83a59833',
    bracketOutline: '#83a59866', searchMatch: '#fabd2e44', searchMatchSelected: '#fabd2e88',
    accent: '#83a598', panelBg: '#1d2021', panelBorder: '#3c3836', tooltipBg: '#1d2021', tooltipBorder: '#3c3836',
  },
  'ayu-mirage': {
    bg: '#242936', fg: '#cccac2', gutterBg: '#242936', gutterFg: '#5c6166', gutterBorder: '#2b313a',
    activeLine: '#2b313a44', selection: '#33415e', cursor: '#ffcc66', bracket: '#ffcc6633',
    bracketOutline: '#ffcc6666', searchMatch: '#e6b45066', searchMatchSelected: '#e6b450aa',
    accent: '#ffcc66', panelBg: '#1f2430', panelBorder: '#2b313a', tooltipBg: '#1f2430', tooltipBorder: '#2b313a',
  },
  'parchment': {
    bg: '#f4f0e8', fg: '#3b3228', gutterBg: '#ece8df', gutterFg: '#9e9688', gutterBorder: '#ddd8ce',
    activeLine: '#e8e3d888', selection: '#d4cfc4', cursor: '#3b3228', bracket: '#7b685433',
    bracketOutline: '#7b685466', searchMatch: '#e6c07b66', searchMatchSelected: '#e6c07baa',
    accent: '#7b6854', panelBg: '#ece8df', panelBorder: '#ddd8ce', tooltipBg: '#ece8df', tooltipBorder: '#ddd8ce',
  },
}

// ── Per-theme syntax highlight styles ────────────────────────────────
// Each theme gets its own HighlightStyle with colors from Zed/industry standards

const THEME_HIGHLIGHTS: Record<string, HighlightStyle> = {
  'k2so-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#c678dd' },
    { tag: tags.operator, color: '#56b6c2' },
    { tag: tags.string, color: '#98c379' },
    { tag: tags.number, color: '#d19a66' },
    { tag: tags.bool, color: '#d19a66' },
    { tag: tags.null, color: '#d19a66' },
    { tag: tags.comment, color: '#5c6370', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#61afef' },
    { tag: tags.typeName, color: '#e5c07b' },
    { tag: tags.className, color: '#e5c07b' },
    { tag: tags.definition(tags.variableName), color: '#e06c75' },
    { tag: tags.propertyName, color: '#e06c75' },
    { tag: tags.variableName, color: '#e4e4e7' },
    { tag: tags.attributeName, color: '#d19a66' },
    { tag: tags.tagName, color: '#e06c75' },
    { tag: tags.meta, color: '#abb2bf' },
    { tag: tags.regexp, color: '#98c379' },
    { tag: tags.punctuation, color: '#abb2bf' },
  ]),
  'one-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#c678dd' },
    { tag: tags.operator, color: '#56b6c2' },
    { tag: tags.string, color: '#98c379' },
    { tag: tags.number, color: '#d19a66' },
    { tag: tags.bool, color: '#d19a66' },
    { tag: tags.null, color: '#d19a66' },
    { tag: tags.comment, color: '#5c6370', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#61afef' },
    { tag: tags.typeName, color: '#e5c07b' },
    { tag: tags.className, color: '#e5c07b' },
    { tag: tags.definition(tags.variableName), color: '#e06c75' },
    { tag: tags.propertyName, color: '#e06c75' },
    { tag: tags.variableName, color: '#abb2bf' },
    { tag: tags.attributeName, color: '#d19a66' },
    { tag: tags.tagName, color: '#e06c75' },
    { tag: tags.meta, color: '#abb2bf' },
    { tag: tags.regexp, color: '#98c379' },
    { tag: tags.punctuation, color: '#abb2bf' },
  ]),
  'dracula': HighlightStyle.define([
    { tag: tags.keyword, color: '#ff79c6' },
    { tag: tags.operator, color: '#ff79c6' },
    { tag: tags.string, color: '#f1fa8c' },
    { tag: tags.number, color: '#bd93f9' },
    { tag: tags.bool, color: '#bd93f9' },
    { tag: tags.null, color: '#bd93f9' },
    { tag: tags.comment, color: '#6272a4', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#50fa7b' },
    { tag: tags.typeName, color: '#8be9fd', fontStyle: 'italic' },
    { tag: tags.className, color: '#8be9fd' },
    { tag: tags.definition(tags.variableName), color: '#f8f8f2' },
    { tag: tags.propertyName, color: '#66d9ef' },
    { tag: tags.variableName, color: '#f8f8f2' },
    { tag: tags.attributeName, color: '#50fa7b' },
    { tag: tags.tagName, color: '#ff79c6' },
    { tag: tags.meta, color: '#f8f8f2' },
    { tag: tags.regexp, color: '#f1fa8c' },
    { tag: tags.punctuation, color: '#f8f8f2' },
  ]),
  'nord': HighlightStyle.define([
    { tag: tags.keyword, color: '#81a1c1' },
    { tag: tags.operator, color: '#81a1c1' },
    { tag: tags.string, color: '#a3be8c' },
    { tag: tags.number, color: '#b48ead' },
    { tag: tags.bool, color: '#81a1c1' },
    { tag: tags.null, color: '#81a1c1' },
    { tag: tags.comment, color: '#616e88', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#88c0d0' },
    { tag: tags.typeName, color: '#8fbcbb' },
    { tag: tags.className, color: '#8fbcbb' },
    { tag: tags.definition(tags.variableName), color: '#d8dee9' },
    { tag: tags.propertyName, color: '#88c0d0' },
    { tag: tags.variableName, color: '#d8dee9' },
    { tag: tags.attributeName, color: '#8fbcbb' },
    { tag: tags.tagName, color: '#81a1c1' },
    { tag: tags.meta, color: '#d8dee9' },
    { tag: tags.regexp, color: '#ebcb8b' },
    { tag: tags.punctuation, color: '#eceff4' },
  ]),
  'github-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#ff7b72' },
    { tag: tags.operator, color: '#ff7b72' },
    { tag: tags.string, color: '#a5d6ff' },
    { tag: tags.number, color: '#79c0ff' },
    { tag: tags.bool, color: '#79c0ff' },
    { tag: tags.null, color: '#79c0ff' },
    { tag: tags.comment, color: '#8b949e', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#d2a8ff' },
    { tag: tags.typeName, color: '#ffa657' },
    { tag: tags.className, color: '#ffa657' },
    { tag: tags.definition(tags.variableName), color: '#ffa657' },
    { tag: tags.propertyName, color: '#79c0ff' },
    { tag: tags.variableName, color: '#c9d1d9' },
    { tag: tags.attributeName, color: '#79c0ff' },
    { tag: tags.tagName, color: '#7ee787' },
    { tag: tags.meta, color: '#c9d1d9' },
    { tag: tags.regexp, color: '#a5d6ff' },
    { tag: tags.punctuation, color: '#c9d1d9' },
  ]),
  'gruvbox-dark': HighlightStyle.define([
    { tag: tags.keyword, color: '#fb4934' },
    { tag: tags.operator, color: '#8ec07c' },
    { tag: tags.string, color: '#b8bb26' },
    { tag: tags.number, color: '#d3869b' },
    { tag: tags.bool, color: '#d3869b' },
    { tag: tags.null, color: '#d3869b' },
    { tag: tags.comment, color: '#928374', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#b8bb26' },
    { tag: tags.typeName, color: '#fabd2f' },
    { tag: tags.className, color: '#fabd2f' },
    { tag: tags.definition(tags.variableName), color: '#83a598' },
    { tag: tags.propertyName, color: '#83a598' },
    { tag: tags.variableName, color: '#ebdbb2' },
    { tag: tags.attributeName, color: '#fabd2f' },
    { tag: tags.tagName, color: '#8ec07c' },
    { tag: tags.meta, color: '#ebdbb2' },
    { tag: tags.regexp, color: '#b8bb26' },
    { tag: tags.punctuation, color: '#a89984' },
  ]),
  'ayu-mirage': HighlightStyle.define([
    { tag: tags.keyword, color: '#ffad66' },
    { tag: tags.operator, color: '#f29e74' },
    { tag: tags.string, color: '#d5ff80' },
    { tag: tags.number, color: '#dfbfff' },
    { tag: tags.bool, color: '#dfbfff' },
    { tag: tags.null, color: '#dfbfff' },
    { tag: tags.comment, color: '#5c6773', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#ffd173' },
    { tag: tags.typeName, color: '#73d0ff' },
    { tag: tags.className, color: '#73d0ff' },
    { tag: tags.definition(tags.variableName), color: '#73d0ff' },
    { tag: tags.propertyName, color: '#5ccfe6' },
    { tag: tags.variableName, color: '#cccac2' },
    { tag: tags.attributeName, color: '#ffd173' },
    { tag: tags.tagName, color: '#73d0ff' },
    { tag: tags.meta, color: '#cccac2' },
    { tag: tags.regexp, color: '#95e6cb' },
    { tag: tags.punctuation, color: '#b8cfe6' },
  ]),
  'parchment': HighlightStyle.define([
    { tag: tags.keyword, color: '#8b4513' },
    { tag: tags.operator, color: '#6b5344' },
    { tag: tags.string, color: '#2e7d32' },
    { tag: tags.number, color: '#7b1fa2' },
    { tag: tags.bool, color: '#7b1fa2' },
    { tag: tags.null, color: '#7b1fa2' },
    { tag: tags.comment, color: '#9e9688', fontStyle: 'italic' },
    { tag: tags.function(tags.variableName), color: '#1565c0' },
    { tag: tags.typeName, color: '#c56200' },
    { tag: tags.className, color: '#c56200' },
    { tag: tags.definition(tags.variableName), color: '#ad1457' },
    { tag: tags.propertyName, color: '#1565c0' },
    { tag: tags.variableName, color: '#3b3228' },
    { tag: tags.attributeName, color: '#c56200' },
    { tag: tags.tagName, color: '#ad1457' },
    { tag: tags.meta, color: '#6b5344' },
    { tag: tags.regexp, color: '#2e7d32' },
    { tag: tags.punctuation, color: '#6b5344' },
  ]),
}

function getHighlightExtension(themeId: string): Extension {
  if (themeId.startsWith('custom:')) {
    const custom = useCustomThemesStore.getState().getTheme(themeId)
    if (custom) {
      return [syntaxHighlighting(custom.highlight), syntaxHighlighting(defaultHighlightStyle, { fallback: true })]
    }
  }
  const hl = THEME_HIGHLIGHTS[themeId] || THEME_HIGHLIGHTS['k2so-dark']
  return [syntaxHighlighting(hl), syntaxHighlighting(defaultHighlightStyle, { fallback: true })]
}

export const EDITOR_THEMES: { id: string; label: string }[] = [
  { id: 'k2so-dark', label: 'K2 Dark' },
  { id: 'one-dark', label: 'One Dark' },
  { id: 'dracula', label: 'Dracula' },
  { id: 'nord', label: 'Nord' },
  { id: 'github-dark', label: 'GitHub Dark' },
  { id: 'gruvbox-dark', label: 'Gruvbox Dark' },
  { id: 'ayu-mirage', label: 'Ayu Mirage' },
  { id: 'parchment', label: 'Parchment' },
]

const LIGHT_THEMES = new Set(['parchment'])

export const EDITOR_FONTS: { id: string; label: string }[] = [
  { id: 'MesloLGM Nerd Font', label: 'MesloLGM Nerd Font' },
  { id: 'JetBrains Mono', label: 'JetBrains Mono' },
  { id: 'Fira Code', label: 'Fira Code' },
  { id: 'Lilex', label: 'Lilex' },
  { id: 'Menlo', label: 'Menlo' },
  { id: 'Monaco', label: 'Monaco' },
]

function buildEditorTheme(colors: ThemeColors, isLight = false): Extension {
  return EditorView.theme({
    '&': {
      backgroundColor: colors.bg,
      color: colors.fg,
      height: '100%',
    },
    '.cm-content': {
      caretColor: colors.cursor,
      padding: '8px 0',
    },
    '.cm-cursor, .cm-dropCursor': {
      borderLeftColor: colors.cursor,
      borderLeftWidth: '2px',
    },
    '.cm-selectionBackground, ::selection': {
      backgroundColor: `${colors.selection} !important`,
    },
    '.cm-activeLine': {
      backgroundColor: colors.activeLine,
    },
    '.cm-activeLineGutter': {
      backgroundColor: colors.activeLine,
      color: colors.fg,
    },
    '.cm-gutters': {
      backgroundColor: colors.gutterBg,
      color: colors.gutterFg,
      border: 'none',
      borderRight: `1px solid ${colors.gutterBorder}`,
    },
    '.cm-lineNumbers .cm-gutterElement': {
      padding: '0 8px 0 16px',
      minWidth: '3em',
      fontSize: '11px',
    },
    '.cm-scroller': {
      overflow: 'auto',
      lineHeight: '1.6',
    },
    '.cm-matchingBracket': {
      backgroundColor: colors.bracket,
      outline: `1px solid ${colors.bracketOutline}`,
    },
    '.cm-searchMatch': {
      backgroundColor: colors.searchMatch,
    },
    '.cm-searchMatch.cm-searchMatch-selected': {
      backgroundColor: colors.searchMatchSelected,
    },
    '.cm-selectionMatch': {
      backgroundColor: `${colors.accent}22`,
    },
    '.cm-foldGutter .cm-gutterElement': {
      padding: '0 4px', cursor: 'pointer', color: colors.gutterFg, fontSize: '11px', transition: 'color 0.15s',
    },
    '.cm-foldGutter .cm-gutterElement:hover': { color: colors.fg },
    '.cm-foldPlaceholder': {
      backgroundColor: colors.panelBg, border: `1px solid ${colors.panelBorder}`, color: '#888',
      padding: '0 6px', margin: '0 4px', borderRadius: '3px', cursor: 'pointer',
    },
    '.cm-tooltip': {
      backgroundColor: colors.tooltipBg, border: `1px solid ${colors.tooltipBorder}`,
      borderRadius: '4px', boxShadow: '0 4px 12px rgba(0,0,0,0.5)',
    },
    '.cm-tooltip.cm-tooltip-autocomplete': {
      backgroundColor: colors.tooltipBg, border: `1px solid ${colors.tooltipBorder}`,
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul': {
      fontSize: '11px', maxHeight: '200px',
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li': {
      padding: '3px 8px', color: colors.fg,
    },
    '.cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]': {
      backgroundColor: `${colors.accent}33`, color: '#fff',
    },
    '.cm-completionMatchedText': { color: colors.accent, textDecoration: 'none' },
    '.cm-panel.cm-search': {
      backgroundColor: colors.panelBg, borderBottom: `1px solid ${colors.panelBorder}`,
      padding: '4px 8px', gap: '4px', fontSize: '11px', color: colors.fg,
    },
    '.cm-panel.cm-search input': {
      backgroundColor: colors.bg, border: `1px solid ${colors.panelBorder}`, color: colors.fg,
      padding: '2px 6px', borderRadius: '3px', fontSize: '11px', outline: 'none',
    },
    '.cm-panel.cm-search input:focus': { borderColor: colors.accent },
    '.cm-panel.cm-search button': {
      backgroundColor: colors.panelBg, border: `1px solid ${colors.panelBorder}`,
      color: colors.fg, padding: '2px 8px', borderRadius: '3px', fontSize: '11px', cursor: 'pointer',
    },
    '.cm-panel.cm-search button:hover': { backgroundColor: colors.panelBorder, color: colors.fg },
    '.cm-panel.cm-search label': { color: colors.gutterFg, fontSize: '11px' },
    '.cm-indentation-marker': { opacity: '0.15' },
    '.cm-indentation-marker.active': { opacity: '0.35' },
    '&.cm-focused': { outline: 'none' },
  }, { dark: !isLight })
}

function getThemeExtension(themeId: string): Extension {
  if (themeId.startsWith('custom:')) {
    const custom = useCustomThemesStore.getState().getTheme(themeId)
    if (custom) {
      return buildEditorTheme(custom.colors, custom.isLight)
    }
  }
  const colors = THEME_COLORS[themeId] || THEME_COLORS['k2so-dark']
  return buildEditorTheme(colors, LIGHT_THEMES.has(themeId))
}

// ── Component ────────────────────────────────────────────────────────

interface ThemeOverride {
  colors: import('@/lib/editor-themes').ThemeColors
  highlight: import('@codemirror/language').HighlightStyle
  isLight?: boolean
}

interface CodeEditorProps {
  code: string
  filePath: string
  onSave: (content: string) => void
  onChange: (content: string) => void
  onCursorChange?: (line: number, col: number, selections: number) => void
  readOnly?: boolean
  /** Inject fake diff data for preview/demo purposes (skips real git polling) */
  demoLineChanges?: Map<number, 'added' | 'modified' | 'deleted'>
  /** Override theme from settings — used by the custom theme creator for live preview */
  themeOverride?: ThemeOverride
  /** Restore scroll position on mount (pixels). */
  initialScrollTop?: number
  /** Restore cursor position on mount (character offset). */
  initialCursorPos?: number
  /** Called when the editor is about to unmount, with current scroll/cursor state. */
  onPersistState?: (state: { scrollTop: number; cursorPos: number }) => void
}

// ── Compartments for live-reconfigurable settings ───────────────────
const wrapCompartment = new Compartment()
const tabSizeCompartment = new Compartment()
const whitespaceCompartment = new Compartment()
const indentGuidesCompartment = new Compartment()
const foldCompartment = new Compartment()
const autocompleteCompartment = new Compartment()
const lineNumbersCompartment = new Compartment()
const activeLineCompartment = new Compartment()
const bracketCompartment = new Compartment()
const fontSizeCompartment = new Compartment()
const fontFamilyCompartment = new Compartment()
const cursorCompartment = new Compartment()
const themeCompartment = new Compartment()
const scrollPastEndCompartment = new Compartment()
const vimCompartment = new Compartment()
const stickyScrollCompartment = new Compartment()
const minimapCompartment = new Compartment()
const gitGutterCompartment = new Compartment()
const highlightCompartment = new Compartment()

// ── Git gutter markers ──────────────────────────────────────────────

interface DiffHunk {
  oldStart: number
  oldCount: number
  newStart: number
  newCount: number
  lines: { kind: string; content: string }[]
}

class GitAddMarker extends GutterMarker {
  toDOM() {
    const el = document.createElement('div')
    el.style.cssText = 'width:3px;height:100%;background:#22c55e;border-radius:1px;'
    return el
  }
}

class GitModifyMarker extends GutterMarker {
  toDOM() {
    const el = document.createElement('div')
    el.style.cssText = 'width:3px;height:100%;background:#3b82f6;border-radius:1px;'
    return el
  }
}

class GitDeleteMarker extends GutterMarker {
  toDOM() {
    const el = document.createElement('div')
    el.style.cssText = 'width:0;height:0;border-left:4px solid transparent;border-right:4px solid transparent;border-top:4px solid #ef4444;margin-top:-2px;'
    return el
  }
}

const addMarker = new GitAddMarker()
const modifyMarker = new GitModifyMarker()
const deleteMarker = new GitDeleteMarker()

function hunksToLineMap(hunks: DiffHunk[]): Map<number, 'added' | 'modified' | 'deleted'> {
  const map = new Map<number, 'added' | 'modified' | 'deleted'>()
  for (const hunk of hunks) {
    let newLine = hunk.newStart
    const hasRemoves = hunk.lines.some(l => l.kind === 'remove')
    const hasAdds = hunk.lines.some(l => l.kind === 'add')

    if (hasRemoves && !hasAdds) {
      // Pure deletion — mark at the line where content was removed
      map.set(hunk.newStart, 'deleted')
      continue
    }

    for (const line of hunk.lines) {
      if (line.kind === 'add') {
        map.set(newLine, hasRemoves ? 'modified' : 'added')
        newLine++
      } else if (line.kind === 'context') {
        newLine++
      }
      // 'remove' lines don't advance newLine
    }
  }
  return map
}

// Line highlight decorations for git diff — gutter mode (subtle tints)
const gutterAddedDeco = Decoration.line({ class: 'cm-git-line-added' })
const gutterModifiedDeco = Decoration.line({ class: 'cm-git-line-modified' })
// Inline/PR mode (strong green/red like GitHub diffs)
const inlineAddedDeco = Decoration.line({ class: 'cm-diff-line-added' })
const inlineModifiedDeco = Decoration.line({ class: 'cm-diff-line-modified' })
const inlineDeletedDeco = Decoration.line({ class: 'cm-diff-line-deleted' })

function buildGitGutterExtension(
  lineChanges: Map<number, 'added' | 'modified' | 'deleted'>,
  showScrollAnnotations: boolean,
  diffStyle: 'gutter' | 'inline' = 'gutter',
): Extension {
  if (lineChanges.size === 0) return []

  const isInline = diffStyle === 'inline'

  // Line highlight plugin
  const lineHighlightPlugin = ViewPlugin.fromClass(
    class {
      decorations: DecorationSet
      constructor(view: EditorView) { this.decorations = this.buildDecos(view) }
      update(update: ViewUpdate) {
        if (update.docChanged) this.decorations = this.buildDecos(update.view)
      }
      buildDecos(view: EditorView): DecorationSet {
        const decos: { from: number; deco: Decoration }[] = []
        for (const [lineNum, kind] of lineChanges) {
          if (lineNum < 1 || lineNum > view.state.doc.lines) continue
          if (!isInline && kind === 'deleted') continue
          const from = view.state.doc.line(lineNum).from
          let deco: Decoration
          if (isInline) {
            deco = kind === 'added' ? inlineAddedDeco : kind === 'modified' ? inlineModifiedDeco : inlineDeletedDeco
          } else {
            deco = kind === 'added' ? gutterAddedDeco : gutterModifiedDeco
          }
          decos.push({ from, deco })
        }
        decos.sort((a, b) => a.from - b.from)
        return Decoration.set(decos.map(d => d.deco.range(d.from)))
      }
    },
    { decorations: (v) => v.decorations }
  )

  // Scrollbar annotation overlay — colored tick marks pinned over the scrollbar track.
  // Attached to view.dom (the editor wrapper) so it stays fixed while content scrolls.
  const scrollAnnotationPlugin = ViewPlugin.fromClass(
    class {
      container: HTMLElement
      constructor(view: EditorView) {
        this.container = document.createElement('div')
        this.container.className = 'cm-scroll-annotations'
        // Attach to the editor wrapper (view.dom), NOT the scrollDOM
        view.dom.style.position = 'relative'
        view.dom.appendChild(this.container)
        this.render(view)
      }
      update(update: ViewUpdate) {
        if (update.geometryChanged || update.docChanged || update.viewportChanged) {
          this.render(update.view)
        }
      }
      render(view: EditorView) {
        const totalLines = view.state.doc.lines
        if (totalLines === 0) { this.container.innerHTML = ''; return }

        // Group consecutive same-type changes into ranges for cleaner rendering
        const marks: { top: number; height: number; color: string }[] = []
        const entries = Array.from(lineChanges.entries()).sort((a, b) => a[0] - b[0])

        for (const [lineNum, kind] of entries) {
          if (lineNum < 1 || lineNum > totalLines) continue
          const pct = (lineNum - 1) / totalLines
          const color = kind === 'added' ? '#22c55e' : kind === 'modified' ? '#3b82f6' : '#ef4444'
          const last = marks.length > 0 ? marks[marks.length - 1] : null
          const topPx = pct * 100
          if (last && last.color === color && Math.abs(topPx - (last.top + last.height)) < 0.5) {
            last.height += Math.max(100 / totalLines, 0.3)
          } else {
            marks.push({ top: topPx, height: Math.max(100 / totalLines, 0.3), color })
          }
        }

        this.container.textContent = ''
        for (const m of marks) {
          const el = document.createElement('div')
          el.style.cssText = 'position:absolute;right:0;width:6px;min-height:2px;opacity:0.7;pointer-events:none;border-radius:1px;'
          el.style.top = `${m.top}%`
          el.style.height = `${m.height}%`
          el.style.background = m.color
          this.container.appendChild(el)
        }
      }
      destroy() {
        this.container.remove()
      }
    }
  )

  const exts: Extension[] = [lineHighlightPlugin]

  // Gutter mode: thin colored bars in the margin
  if (!isInline) {
    exts.push(gutter({
      class: 'cm-git-gutter',
      markers: (view) => {
        const markers: { from: number; marker: GutterMarker }[] = []
        for (const [lineNum, kind] of lineChanges) {
          if (lineNum < 1 || lineNum > view.state.doc.lines) continue
          const from = view.state.doc.line(lineNum).from
          const marker = kind === 'added' ? addMarker : kind === 'modified' ? modifyMarker : deleteMarker
          markers.push({ from, marker })
        }
        markers.sort((a, b) => a.from - b.from)
        return RangeSet.of(markers.map(m => m.marker.range(m.from)))
      },
      initialSpacer: () => addMarker,
    }))
  }

  if (showScrollAnnotations) {
    exts.push(scrollAnnotationPlugin)
  }

  return exts
}

// ── Setting helpers ──────────────────────────────────────────────────

function buildFontFamilyExtension(family: string, ligatures: boolean): Extension {
  const fontStack = `"${family}", "Menlo", "Monaco", "Courier New", monospace`
  const ligatureCSS = ligatures
    ? { fontVariantLigatures: 'normal', fontFeatureSettings: '"liga" 1, "calt" 1' }
    : { fontVariantLigatures: 'none', fontFeatureSettings: '"liga" 0, "calt" 0' }
  return EditorView.theme({
    '&': { fontFamily: fontStack },
    '.cm-scroller': { fontFamily: 'inherit' },
    '.cm-content': ligatureCSS,
    '.cm-line': ligatureCSS,
    '.cm-tooltip.cm-tooltip-autocomplete > ul': { fontFamily: fontStack },
  })
}

function buildCursorExtension(style: string, blink: boolean): Extension {
  const blinkRate = blink ? undefined : '0' // 0 = no animation
  if (style === 'block') {
    return EditorView.theme({
      '.cm-cursor': {
        borderLeft: 'none',
        backgroundColor: '#3b82f680',
        width: '0.6em',
        ...(blinkRate ? { animationDuration: blinkRate } : {}),
      },
      '&.cm-focused .cm-cursor': {
        ...(blinkRate ? { animationDuration: blinkRate } : {}),
      },
    })
  }
  if (style === 'underline') {
    return EditorView.theme({
      '.cm-cursor': {
        borderLeft: 'none',
        borderBottom: '2px solid #3b82f6',
        width: '0.6em',
        height: '0 !important',
        ...(blinkRate ? { animationDuration: blinkRate } : {}),
      },
    })
  }
  // Default: bar
  return EditorView.theme({
    '.cm-cursor, .cm-dropCursor': {
      borderLeftColor: '#3b82f6',
      borderLeftWidth: '2px',
      ...(blinkRate ? { animationDuration: blinkRate } : {}),
    },
  })
}

// ── Per-language tab size defaults ───────────────────────────────────

const LANGUAGE_TAB_DEFAULTS: Record<string, number> = {
  py: 4, pyw: 4, pyi: 4,
  rs: 4,
  go: 4,
  java: 4,
  kt: 4, kts: 4,
  cs: 4,
  swift: 4,
  c: 4, cpp: 4, h: 4, hpp: 4,
}

function getEffectiveTabSize(filePath: string, settingsTabSize: number): number {
  const ext = filePath.split('.').pop()?.toLowerCase() || ''
  return LANGUAGE_TAB_DEFAULTS[ext] ?? settingsTabSize
}

// ── Rainbow bracket colorization ──────────────────────────────────────

const BRACKET_PAIRS: Record<string, string> = { '(': ')', '[': ']', '{': '}' }
const CLOSE_BRACKETS = new Set([')', ']', '}'])
const RAINBOW_COLORS = ['#fbbf24', '#c084fc', '#22d3ee', '#f472b6', '#34d399', '#fb923c']

function buildRainbowDecorations(view: EditorView): DecorationSet {
  const decorations: { from: number; to: number; color: string }[] = []
  const stack: { char: string; pos: number }[] = []

  // Only scan visible viewport + 500 char buffer (not entire document)
  const { from: vpFrom, to: vpTo } = view.viewport
  const scanFrom = Math.max(0, vpFrom - 500)
  const scanTo = Math.min(view.state.doc.length, vpTo + 500)

  // Pre-scan from document start to scanFrom to establish bracket depth
  if (scanFrom > 0) {
    const prefix = view.state.sliceDoc(0, scanFrom)
    for (let i = 0; i < prefix.length; i++) {
      const ch = prefix[i]
      if (BRACKET_PAIRS[ch]) {
        stack.push({ char: ch, pos: i })
      } else if (CLOSE_BRACKETS.has(ch)) {
        const last = stack.length > 0 ? stack[stack.length - 1] : null
        if (last && BRACKET_PAIRS[last.char] === ch) stack.pop()
      }
    }
  }

  // Scan visible region and decorate
  const visible = view.state.sliceDoc(scanFrom, scanTo)
  for (let i = 0; i < visible.length; i++) {
    const ch = visible[i]
    const absPos = scanFrom + i
    if (BRACKET_PAIRS[ch]) {
      const depth = stack.length
      const color = RAINBOW_COLORS[depth % RAINBOW_COLORS.length]
      stack.push({ char: ch, pos: absPos })
      decorations.push({ from: absPos, to: absPos + 1, color })
    } else if (CLOSE_BRACKETS.has(ch)) {
      const last = stack.length > 0 ? stack[stack.length - 1] : null
      if (last && BRACKET_PAIRS[last.char] === ch) {
        stack.pop()
        const depth = stack.length
        const color = RAINBOW_COLORS[depth % RAINBOW_COLORS.length]
        decorations.push({ from: absPos, to: absPos + 1, color })
      }
    }
  }

  return Decoration.set(
    decorations.map(d =>
      Decoration.mark({ attributes: { style: `color: ${d.color}` } }).range(d.from, d.to)
    ),
    true
  )
}

const rainbowBrackets = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet
    constructor(view: EditorView) { this.decorations = buildRainbowDecorations(view) }
    update(update: ViewUpdate) {
      if (update.docChanged || update.viewportChanged) {
        this.decorations = buildRainbowDecorations(update.view)
      }
    }
  },
  { decorations: (v) => v.decorations }
)

// ── Sticky scroll ─────────────────────────────────────────────────────
// Shows the enclosing scope (function/class/method) header pinned at top.
// Uses the syntax tree node types to find scope boundaries.

const SCOPE_NODE_TYPES = new Set([
  'FunctionDeclaration', 'FunctionDefinition', 'FunctionExpression', 'ArrowFunction',
  'MethodDeclaration', 'MethodDefinition',
  'ClassDeclaration', 'ClassDefinition', 'ClassBody',
  'ObjectExpression', 'ObjectType',
  'IfStatement', 'ForStatement', 'WhileStatement', 'SwitchStatement',
  'Block', 'BlockStatement',
])

const stickyScrollPlugin = ViewPlugin.fromClass(
  class {
    dom: HTMLElement
    constructor(view: EditorView) {
      this.dom = document.createElement('div')
      this.dom.className = 'cm-sticky-scroll'
      this.dom.style.cssText = 'position:absolute;top:0;left:0;right:0;z-index:5;background:var(--color-bg-surface,#141414);border-bottom:1px solid var(--color-border,#2a2a2a);font-size:11px;line-height:1.6;padding:0 4px 0 16px;color:var(--color-text-muted,#71717a);overflow:hidden;white-space:nowrap;text-overflow:ellipsis;pointer-events:none;display:none;'
      view.dom.style.position = 'relative'
      view.dom.appendChild(this.dom)
      this.updateContent(view)
    }
    update(update: ViewUpdate) {
      if (update.viewportChanged || update.docChanged || update.selectionSet) {
        this.updateContent(update.view)
      }
    }
    updateContent(view: EditorView) {
      const tree = syntaxTree(view.state)
      const topLine = view.lineBlockAtHeight(view.scrollDOM.scrollTop)
      const pos = topLine.from

      // Walk up the tree from the top visible position to find enclosing scopes
      const headers: string[] = []
      let node = tree.resolveInner(pos, 1)
      while (node.parent) {
        node = node.parent
        if (SCOPE_NODE_TYPES.has(node.type.name)) {
          const firstLine = view.state.doc.lineAt(node.from)
          // Only show if the header line is scrolled off the top
          if (firstLine.from < topLine.from) {
            const text = firstLine.text.trim()
            if (text.length > 2) headers.unshift(text)
          }
        }
        if (headers.length >= 3) break // max 3 levels of nesting
      }

      if (headers.length > 0) {
        this.dom.style.display = 'block'
        this.dom.textContent = headers.join('  >  ')
      } else {
        this.dom.style.display = 'none'
      }
    }
    destroy() {
      this.dom.remove()
    }
  }
)

export function CodeEditor({ code, filePath, onSave, onChange, onCursorChange, readOnly = false, demoLineChanges, themeOverride, initialScrollTop, initialCursorPos, onPersistState }: CodeEditorProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const onSaveRef = useRef(onSave)
  const onChangeRef = useRef(onChange)
  const onCursorChangeRef = useRef(onCursorChange)
  const onPersistStateRef = useRef(onPersistState)
  const codeRef = useRef(code)
  const initialScrollTopRef = useRef(initialScrollTop)
  const initialCursorPosRef = useRef(initialCursorPos)

  const editorSettings = useSettingsStore((s) => s.editor)
  // Subscribe to custom themes so we re-render when they load/change
  const customThemes = useCustomThemesStore((s) => s.customThemes)
  // Retained-view: when this editor lives inside a hidden tab, CodeMirror
  // mounts against a 0x0 parent. On visibility flip we re-measure so the
  // viewport lays out correctly.
  const isVisible = useIsTabVisible()

  // Keep refs current
  onSaveRef.current = onSave
  onChangeRef.current = onChange
  onCursorChangeRef.current = onCursorChange
  onPersistStateRef.current = onPersistState
  codeRef.current = code

  // Create editor on mount
  useEffect(() => {
    if (!containerRef.current) return

    const es = useSettingsStore.getState().editor
    const langExt = getLanguageExtension(filePath)

    const saveKeymap = keymap.of([{
      key: 'Mod-s',
      run: (view) => {
        const content = view.state.doc.toString()
        onSaveRef.current(content)
        // Format on save: run formatter then reload file content
        if (useSettingsStore.getState().editor.formatOnSave) {
          invoke('format_file', { filePath }).then(() => {
            // Re-read the formatted file — the parent will poll and update
            // but we can also trigger an immediate onChange to refresh
          }).catch(() => {
            // Formatter not available or failed — silently ignore
          })
        }
        return true
      },
    }])

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        onChangeRef.current(update.state.doc.toString())
      }
      if (update.selectionSet || update.docChanged) {
        const sel = update.state.selection.main
        const line = update.state.doc.lineAt(sel.head)
        const col = sel.head - line.from
        const numSelections = update.state.selection.ranges.length
        onCursorChangeRef.current?.(line.number, col + 1, numSelections)
      }
    })

    const effectiveTabSize = getEffectiveTabSize(filePath, es.tabSize)

    const extensions: Extension[] = [
      // Core editing
      history(),
      drawSelection(),
      rectangularSelection(),
      indentOnInput(),
      closeBrackets(),
      highlightSelectionMatches(),
      // Settings-driven compartments
      // Markdown files always soft-wrap regardless of the global
      // editor.wordWrap setting — prose with horizontal scrolling
      // forces users to scroll mid-paragraph, which is awful UX.
      // Code files (TS, Rust, etc.) keep the user's chosen behavior
      // because indentation and column alignment matter there.
      wrapCompartment.of((es.wordWrap || isMarkdownPath(filePath)) ? EditorView.lineWrapping : []),
      tabSizeCompartment.of([EditorState.tabSize.of(effectiveTabSize), indentUnit.of(' '.repeat(effectiveTabSize))]),
      whitespaceCompartment.of(es.showWhitespace ? highlightWhitespace() : []),
      indentGuidesCompartment.of(es.indentGuides ? indentationMarkers({ highlightActiveBlock: true, hideFirstIndent: false }) : []),
      foldCompartment.of(es.foldGutter ? [codeFolding(), foldGutter({ openText: '▾', closedText: '▸' })] : []),
      autocompleteCompartment.of(es.autocomplete ? autocompletion({ activateOnTyping: true, maxRenderedOptions: 30 }) : []),
      lineNumbersCompartment.of(es.lineNumbers ? lineNumbers() : []),
      activeLineCompartment.of(es.highlightActiveLine ? [highlightActiveLine(), highlightActiveLineGutter()] : []),
      bracketCompartment.of(es.bracketMatching ? bracketMatching() : []),
      fontSizeCompartment.of(EditorView.theme({ '&': { fontSize: `${es.fontSize || 12}px` }, '.cm-gutters': { fontSize: `${Math.max((es.fontSize || 12) - 1, 10)}px` } })),
      fontFamilyCompartment.of(buildFontFamilyExtension(es.fontFamily || 'MesloLGM Nerd Font', es.fontLigatures ?? false)),
      cursorCompartment.of(buildCursorExtension(es.cursorStyle || 'bar', es.cursorBlink ?? true)),
      scrollPastEndCompartment.of(es.scrollPastEnd ? scrollPastEnd() : []),
      minimapCompartment.of(es.minimap ? showMinimap.compute(['doc'], () => ({
        enabled: true, displayText: 'blocks' as const, showOverlay: 'mouse-over' as const,
      })) : []),
      stickyScrollCompartment.of(es.stickyScroll ? stickyScrollPlugin : []),
      vimCompartment.of(es.vimMode ? vim() : []),
      // Rainbow bracket colorization
      rainbowBrackets,
      // Git gutter (populated async after mount)
      gitGutterCompartment.of([]),
      // Search with replace panel
      search({ top: true }),
      // Keymaps (Cmd+D = select next occurrence)
      saveKeymap,
      keymap.of([
        { key: 'Mod-d', run: selectNextOccurrence, preventDefault: true },
        ...closeBracketsKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        ...searchKeymap,
        ...foldKeymap,
        indentWithTab,
      ]),
      // Syntax highlighting (per-theme colors via compartment)
      highlightCompartment.of(
        themeOverride
          ? [syntaxHighlighting(themeOverride.highlight), syntaxHighlighting(defaultHighlightStyle, { fallback: true })]
          : getHighlightExtension(es.theme || 'k2so-dark')
      ),
      // Theme (via compartment for live switching)
      themeCompartment.of(
        themeOverride
          ? buildEditorTheme(themeOverride.colors, themeOverride.isLight)
          : getThemeExtension(es.theme || 'k2so-dark')
      ),
      // Change tracking
      updateListener,
    ]

    if (langExt) extensions.push(langExt)
    if (readOnly) extensions.push(EditorState.readOnly.of(true))

    const state = EditorState.create({
      doc: code,
      extensions,
    })

    const view = new EditorView({
      state,
      parent: containerRef.current,
    })

    viewRef.current = view

    // Restore saved scroll/cursor position if provided.
    // Defer to next frame so CodeMirror has laid out the content first.
    const initScroll = initialScrollTopRef.current
    const initCursor = initialCursorPosRef.current
    if (initScroll || initCursor) {
      requestAnimationFrame(() => {
        if (!viewRef.current) return
        if (typeof initCursor === 'number' && initCursor > 0) {
          const docLen = viewRef.current.state.doc.length
          const pos = Math.min(initCursor, docLen)
          viewRef.current.dispatch({ selection: { anchor: pos, head: pos } })
        }
        if (typeof initScroll === 'number' && initScroll > 0) {
          viewRef.current.scrollDOM.scrollTop = initScroll
        }
      })
    }

    // Capture scroll position in a local variable as the user scrolls —
    // no state update, no re-render. Flush to the store on unmount
    // (reading scrollDOM.scrollTop at cleanup time returns 0 because
    // React detaches the DOM before cleanups run).
    let latestScrollTop = view.scrollDOM.scrollTop
    const handleScroll = (): void => {
      if (viewRef.current) latestScrollTop = viewRef.current.scrollDOM.scrollTop
    }
    view.scrollDOM.addEventListener('scroll', handleScroll, { passive: true })

    return () => {
      view.scrollDOM.removeEventListener('scroll', handleScroll)
      // Flush final scroll/cursor state to the store. Read cursorPos
      // from the state here (still accessible before view.destroy()).
      const persist = onPersistStateRef.current
      if (persist && viewRef.current) {
        const cursorPos = viewRef.current.state.selection.main.head
        if (latestScrollTop > 0 || cursorPos > 0) {
          persist({ scrollTop: latestScrollTop, cursorPos })
        }
      }
      view.destroy()
      viewRef.current = null
    }
  }, [filePath, readOnly]) // Recreate when file or readOnly changes

  // Re-measure the editor when it transitions from hidden to visible.
  // CodeMirror caches viewport dimensions; a display:none parent means
  // those cached values are 0, so the editor renders blank after reveal
  // without this nudge.
  useEffect(() => {
    if (!isVisible) return
    const view = viewRef.current
    if (!view) return
    view.requestMeasure()
    // Also restore the scroll position if it was saved — CodeMirror's
    // scrollDOM state should survive display:none, but on first
    // reveal after mount the RAF-deferred restore may have landed
    // against a 0-height viewport. Re-apply if we have a target.
    const target = initialScrollTopRef.current
    if (typeof target === 'number' && target > 0 && view.scrollDOM.scrollTop === 0) {
      requestAnimationFrame(() => {
        if (viewRef.current) viewRef.current.scrollDOM.scrollTop = target
      })
    }
  }, [isVisible])

  // Live-reconfigure compartments when settings change
  useEffect(() => {
    const view = viewRef.current
    if (!view) return

    view.dispatch({
      effects: [
        wrapCompartment.reconfigure((editorSettings.wordWrap || isMarkdownPath(filePath)) ? EditorView.lineWrapping : []),
        tabSizeCompartment.reconfigure([EditorState.tabSize.of(editorSettings.tabSize), indentUnit.of(' '.repeat(editorSettings.tabSize))]),
        whitespaceCompartment.reconfigure(editorSettings.showWhitespace ? highlightWhitespace() : []),
        indentGuidesCompartment.reconfigure(editorSettings.indentGuides ? indentationMarkers({ highlightActiveBlock: true, hideFirstIndent: false }) : []),
        foldCompartment.reconfigure(editorSettings.foldGutter ? [codeFolding(), foldGutter({ openText: '▾', closedText: '▸' })] : []),
        autocompleteCompartment.reconfigure(editorSettings.autocomplete ? autocompletion({ activateOnTyping: true, maxRenderedOptions: 30 }) : []),
        lineNumbersCompartment.reconfigure(editorSettings.lineNumbers ? lineNumbers() : []),
        activeLineCompartment.reconfigure(editorSettings.highlightActiveLine ? [highlightActiveLine(), highlightActiveLineGutter()] : []),
        bracketCompartment.reconfigure(editorSettings.bracketMatching ? bracketMatching() : []),
        fontSizeCompartment.reconfigure(EditorView.theme({ '&': { fontSize: `${editorSettings.fontSize}px` }, '.cm-gutters': { fontSize: `${Math.max(editorSettings.fontSize - 1, 10)}px` } })),
        fontFamilyCompartment.reconfigure(buildFontFamilyExtension(editorSettings.fontFamily || 'MesloLGM Nerd Font', editorSettings.fontLigatures ?? false)),
        cursorCompartment.reconfigure(buildCursorExtension(editorSettings.cursorStyle || 'bar', editorSettings.cursorBlink ?? true)),
        themeCompartment.reconfigure(
          themeOverride
            ? buildEditorTheme(themeOverride.colors, themeOverride.isLight)
            : getThemeExtension(editorSettings.theme || 'k2so-dark')
        ),
        highlightCompartment.reconfigure(
          themeOverride
            ? [syntaxHighlighting(themeOverride.highlight), syntaxHighlighting(defaultHighlightStyle, { fallback: true })]
            : getHighlightExtension(editorSettings.theme || 'k2so-dark')
        ),
        scrollPastEndCompartment.reconfigure(editorSettings.scrollPastEnd ? scrollPastEnd() : []),
        minimapCompartment.reconfigure(editorSettings.minimap ? showMinimap.compute(['doc'], () => ({
          enabled: true, displayText: 'blocks' as const, showOverlay: 'mouse-over' as const,
        })) : []),
        stickyScrollCompartment.reconfigure(editorSettings.stickyScroll ? stickyScrollPlugin : []),
        vimCompartment.reconfigure(editorSettings.vimMode ? vim() : []),
      ],
    })
  }, [editorSettings, themeOverride, customThemes])

  // Update content when external changes arrive (file polling)
  useEffect(() => {
    const view = viewRef.current
    if (!view) return

    const currentContent = view.state.doc.toString()
    if (code !== currentContent) {
      view.dispatch({
        changes: {
          from: 0,
          to: view.state.doc.length,
          insert: code,
        },
      })
    }
  }, [code])

  // Git gutter: poll for diff data every 5 seconds (or use demo data)
  useEffect(() => {
    const view = viewRef.current
    if (!view) return

    // Demo mode: inject fake diff data immediately, no polling
    if (demoLineChanges) {
      const es = useSettingsStore.getState().editor
      const ext = buildGitGutterExtension(demoLineChanges, es.scrollbarAnnotations ?? true, es.diffStyle ?? 'gutter')
      view.dispatch({ effects: gitGutterCompartment.reconfigure(ext) })
      return
    }

    // Pass file's directory as repo hint (Repository::discover walks up to find .git)
    const dirPath = filePath.substring(0, filePath.lastIndexOf('/'))

    const fetchDiff = async () => {
      try {
        const hunks = await daemonCliGet<DiffHunk[]>('git/diff-file', { path: dirPath, file_path: filePath })
        const lineChanges = hunksToLineMap(hunks)
        const es = useSettingsStore.getState().editor
        const ext = buildGitGutterExtension(lineChanges, es.scrollbarAnnotations ?? true, es.diffStyle ?? 'gutter')
        if (viewRef.current) {
          viewRef.current.dispatch({ effects: gitGutterCompartment.reconfigure(ext) })
        }
      } catch {
        // Not in a git repo or file not tracked — clear gutter
        if (viewRef.current) {
          viewRef.current.dispatch({ effects: gitGutterCompartment.reconfigure([]) })
        }
      }
    }

    fetchDiff()
    const interval = setInterval(fetchDiff, 15000)
    return () => clearInterval(interval)
  }, [filePath, demoLineChanges, editorSettings.diffStyle, editorSettings.scrollbarAnnotations])

  return (
    <div
      ref={containerRef}
      className="h-full w-full overflow-hidden"
    />
  )
}
