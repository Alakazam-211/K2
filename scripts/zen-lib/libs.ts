// The Zen widget standard library: what scripts/vendor-zen-lib.sh writes
// (prd-zen-user-widgets-v2 §13.9, UWB12–UWB17; Rosson R2, R4, R8).
//
// One entry per library version. The vendor script turns this list into
// `src/renderer/zen-lib/<id>@<version>/` (bundled) and the rows of
// `src/shared/zen-lib.json`, in this order. Every npm package named here is
// pinned exactly in `vendor-package.json`, and its tarball integrity in
// `vendor-package-lock.json`.
//
// Rules (fail the vendor run when broken):
// - Classic globals only. A package that ships ESM only is pre-bundled to
//   an IIFE with esbuild (`bundle`); the command is recorded in the row's
//   `prebundled`.
// - CSS a library needs is shipped as a small script that adds one <style>
//   (`style`), with its url() assets inlined as data: URLs, because the
//   sealed frame inlines scripts only.
// - A licence compatible with shipping inside K2 (MIT, ISC, BSD, Apache-2.0,
//   OFL-1.1, MPL-2.0 as an option). p5.js is LGPL (R4: dropped). No GSAP.
// - `copyright` must appear word for word in the licence text the vendor
//   script writes (the NOTICE line is checked against it).
// - 0.46.5 (prd-widget-size-and-shared-libs-v1 WL6, vs-live L3): one entry
//   per LINE (the major; 0.minor below 1.0). A patch replaces its line's
//   entry in place (K2 updates patches inside a line); a new major is a new
//   entry beside the old one. A line is never removed: to retire it, make
//   its entry `source: 'download'` (pinned URLs, fetched on first use) and
//   add `{id, line, release, use}` to zen-lib.json's `retired` (the
//   Retirement ledger). The stdlib tests enforce both.

export type LibFile =
  /** Copy one file from an installed package, as is. */
  | { name: string; copy: string }
  /** esbuild an entry (source text below) into an IIFE that sets `global`. */
  | { name: string; bundle: string }
  /** A script that injects CSS (from installed files), url() assets inlined. */
  | { name: string; style: StyleSource }
  /** Hand-written glue shipped next to the library (kept tiny). A function
   *  gets `dataUrl(packagePath)` to inline an installed file. */
  | { name: string; glue: string | ((ctx: GlueContext) => string) }
  /** K2's own file (a library with `own`): the checked-in file in
   *  `src/renderer/zen-lib/<id>@<version>/` IS its source, kept as is. */
  | { name: string; own: true }

export interface GlueContext {
  dataUrl(packagePath: string): string
}

export interface StyleSource {
  /** Package paths of the CSS, in order. */
  css: { path: string; wrap?: (css: string) => string }[]
  /** Drop `url(…) format("woff")` / `format("truetype")` fallbacks so only
   *  woff2 is inlined. */
  woff2Only?: boolean
  /** Keep only @font-face blocks whose fontsource comment matches. */
  fontFaceFilter?: RegExp
  /** Attribute value on the injected <style data-k2-lib="…">. */
  tag: string
}

export interface LibSpec {
  id: string
  version: string
  title: string
  global: string
  license: string
  copyright: string
  source: 'bundled' | 'download'
  /** Licence texts joined into the folder's LICENSE.txt: package paths,
   *  `url:<url>` (download libraries), or `header:<path>` (the leading
   *  `/*!` licence comment of an installed file). */
  licenses: string[]
  files: LibFile[]
  /** Download libraries: pinned URL per file name. */
  urls?: Record<string, string>
  notes?: string
  /** K2's own widget kit (0.46.3: k2-thread, k2-help): no npm package; its
   *  files are `{own: true}`, its licence is K2's (FSL-1.1-Apache-2.0, only
   *  allowed here) and `licenses` names `k2:LICENSE.md`. A change ships as
   *  a new version folder; a shipped one is never edited. */
  own?: boolean
  /** 0.46.5 (WL3): other libraries it needs, as line requests
   *  (`marked@18`); K2 loads them first, once. */
  needs?: string[]
  /** 0.46.5 (WL17): text a copy of it carries in its first 1 KB (its own
   *  upstream banner, never a generic licence header). */
  fingerprints?: string[]
}

const THREE_ADDONS: [string, string][] = [
  ['OrbitControls', 'controls/OrbitControls.js'],
  ['MapControls', 'controls/MapControls.js'],
  ['TrackballControls', 'controls/TrackballControls.js'],
  ['PointerLockControls', 'controls/PointerLockControls.js'],
  ['DragControls', 'controls/DragControls.js'],
  ['TransformControls', 'controls/TransformControls.js'],
  ['GLTFLoader', 'loaders/GLTFLoader.js'],
  ['OBJLoader', 'loaders/OBJLoader.js'],
  ['MTLLoader', 'loaders/MTLLoader.js'],
  ['STLLoader', 'loaders/STLLoader.js'],
  ['SVGLoader', 'loaders/SVGLoader.js'],
  ['FontLoader, Font', 'loaders/FontLoader.js'],
  ['TextGeometry', 'geometries/TextGeometry.js'],
  ['RoundedBoxGeometry', 'geometries/RoundedBoxGeometry.js'],
  ['ConvexGeometry', 'geometries/ConvexGeometry.js'],
  ['EffectComposer', 'postprocessing/EffectComposer.js'],
  ['RenderPass', 'postprocessing/RenderPass.js'],
  ['ShaderPass', 'postprocessing/ShaderPass.js'],
  ['UnrealBloomPass', 'postprocessing/UnrealBloomPass.js'],
  ['OutlinePass', 'postprocessing/OutlinePass.js'],
  ['FXAAPass', 'postprocessing/FXAAPass.js'],
  ['OutputPass', 'postprocessing/OutputPass.js'],
  ['CSS2DRenderer, CSS2DObject', 'renderers/CSS2DRenderer.js'],
  ['CSS3DRenderer, CSS3DObject', 'renderers/CSS3DRenderer.js'],
  ['Sky', 'objects/Sky.js'],
  ['SimplexNoise', 'math/SimplexNoise.js'],
  ['ImprovedNoise', 'math/ImprovedNoise.js'],
]

const THREE_ENTRY = [
  "export * from 'three';",
  ...THREE_ADDONS.map(([names, path]) => `export { ${names} } from 'three/addons/${path}';`),
  "export * as BufferGeometryUtils from 'three/addons/utils/BufferGeometryUtils.js';",
  "export * as SkeletonUtils from 'three/addons/utils/SkeletonUtils.js';",
].join('\n')

// Leaflet finds its default marker images by parsing a CSS url() that ends
// in `marker-icon.png`; inlined data: URLs don't, so point the default icon
// at the inlined images directly.
const LEAFLET_GLUE = (ctx: GlueContext): string =>
  `/* K2 glue: Leaflet's default marker icons from inlined images (data: URLs). */
(function(){var L=window.L;if(!L||!L.Icon||!L.Icon.Default)return;L.Icon.Default.imagePath='';L.Icon.Default.mergeOptions({iconUrl:${JSON.stringify(
    ctx.dataUrl('leaflet/dist/images/marker-icon.png'),
  )},iconRetinaUrl:${JSON.stringify(ctx.dataUrl('leaflet/dist/images/marker-icon-2x.png'))},shadowUrl:${JSON.stringify(
    ctx.dataUrl('leaflet/dist/images/marker-shadow.png'),
  )}})})();
`

// d3-dsv's csvParse/tsvParse turn the header row into code with
// `new Function`, which the widget CSP refuses (no 'unsafe-eval'). Same
// results without eval: rows from parseRows, objects built in a loop, the
// row function called as (d, i, columns), null/undefined rows dropped, and
// `columns` on the array.
const D3_GLUE = `/* K2 glue: d3.csvParse / d3.tsvParse without new Function (sealed widget CSP). */
(function(){var d3=window.d3;if(!d3||!d3.csvParseRows)return;function mk(parseRows){return function(text,f){var rows=parseRows(text),columns=rows.length?rows.shift():[],out=[],n=0;for(var i=0;i<rows.length;i++){var r=rows[i],o={};for(var j=0;j<columns.length;j++)o[columns[j]]=r[j]||"";if(f){o=f(o,n++,columns);if(o==null)continue}else{n++}out.push(o)}out.columns=columns;return out}}d3.csvParse=mk(d3.csvParseRows);d3.tsvParse=mk(d3.tsvParseRows)})();
`

/** The libraries, in manifest order. */
/** The notes `k2-markdown`'s row carries (the k2-zen skill shows them). */
export const K2_MARKDOWN_NOTES =
  "K2's own: markdown, safe by default. K2Markdown.into(el, text, opts) fills el; K2Markdown.render(text, opts) returns a DocumentFragment (never a string, so no innerHTML). Raw HTML is escaped; links show as text with the URL visible, or with opts.links = 'open' and the links:open cap as buttons that call k2.open; images show as text, or with opts.images = 'data' as data: PNG, JPEG, GIF and WebP only. It loads marked and DOMPurify for you (its own DOMPurify instance: your settings never change its rules). Colours follow the Garden (--zen-*)."

/** The notes `k2-activity`'s row carries (the k2-zen skill shows them). */
export const K2_ACTIVITY_NOTES =
  "K2's own: the activity bar K2's Conversation shows, for one agent. K2Activity.mount(el, {address: 'handle::host', look: 'chamfer', 'glass' or 'plain'}) then bar.setAddress(address), bar.setLook(look), bar.destroy(). It listens with k2.thread.onActivity (caps thread:read and activity:read): the turn an App guest gets, counts and step kinds, never text. Working pulses; monitoring, needs-you and a quiet turn never do (no typing dots); an end shows Done, Stopped or Failed briefly. K2Activity.describe(K2Activity.coerce(state.turn, Date.now()), Date.now()) gives {tone, headline, animate, counters, clock, aria, detail} for your own drawing. No turn means no bar: never guess from messages. Colours follow the Garden (--zen-*, or --k2a-*)."

export const LIBS: LibSpec[] = [
  // ── 3D and physics ──────────────────────────────────────────────────
  {
    id: 'three',
    version: '0.186.0',
    fingerprints: ['Three.js Authors', '/*! three.js '],
    title: 'three.js',
    global: 'THREE',
    license: 'MIT',
    copyright: 'Copyright © 2010-2026 three.js authors',
    source: 'bundled',
    licenses: ['three/LICENSE'],
    files: [{ name: 'three.js', bundle: THREE_ENTRY }],
    notes:
      'THREE also carries common add-ons: OrbitControls, MapControls, TrackballControls, PointerLockControls, DragControls, TransformControls, GLTFLoader, OBJLoader, MTLLoader, STLLoader, SVGLoader, FontLoader, TextGeometry, RoundedBoxGeometry, ConvexGeometry, EffectComposer, RenderPass, ShaderPass, UnrealBloomPass, OutlinePass, FXAAPass, OutputPass, CSS2DRenderer, CSS3DRenderer, Sky, SimplexNoise, ImprovedNoise, BufferGeometryUtils, SkeletonUtils. Load models and textures from data: URLs (k2.asset), never http.',
  },
  {
    id: 'globe.gl',
    version: '2.46.2',
    fingerprints: ['globe.gl - https://github.com/vasturiano/globe.gl'],
    title: 'globe.gl',
    global: 'Globe',
    license: 'MIT',
    copyright: 'Copyright (c) 2019 Vasco Asturiano',
    source: 'bundled',
    licenses: ['globe.gl/LICENSE'],
    files: [{ name: 'globe.gl.min.js', copy: 'globe.gl/dist/globe.gl.min.js' }],
    notes:
      'Carries its own copy of three.js inside. Its example earth textures are web URLs, which a sealed widget cannot load: pass your own image as a data: URL (globeImageUrl(k2.asset("earth.jpg"))).',
  },
  {
    id: 'cannon-es',
    version: '0.20.0',
    title: 'cannon-es',
    global: 'CANNON',
    license: 'MIT',
    copyright: 'Copyright (c) 2015 cannon.js Authors',
    source: 'bundled',
    licenses: ['cannon-es/LICENSE'],
    files: [{ name: 'cannon-es.js', bundle: "export * from 'cannon-es';" }],
  },
  // ── 2D, games ───────────────────────────────────────────────────────
  {
    id: 'pixi.js',
    version: '8.21.0',
    fingerprints: ['PixiJS - v'],
    title: 'PixiJS',
    global: 'PIXI',
    license: 'MIT',
    copyright: 'Copyright (c) 2013-2023 Mathew Groves, Chad Engler',
    source: 'bundled',
    licenses: ['pixi.js/LICENSE'],
    files: [
      { name: 'pixi.min.js', copy: 'pixi.js/dist/pixi.min.js' },
      // The sealed frame forbids eval/new Function; PixiJS's own CSP build
      // swaps its generated shader-sync code for plain functions.
      { name: 'unsafe-eval.min.js', copy: 'pixi.js/dist/packages/unsafe-eval.min.js' },
    ],
    notes: 'Ships with PixiJS\'s no-eval add-on already loaded, so it runs under the widget security policy.',
  },
  {
    id: 'phaser',
    version: '4.2.1',
    title: 'Phaser',
    global: 'Phaser',
    license: 'MIT',
    copyright: 'Copyright (c) 2026 Richard Davey, Phaser Studio Inc.',
    source: 'bundled',
    licenses: ['phaser/LICENSE.md'],
    files: [{ name: 'phaser.min.js', copy: 'phaser/dist/phaser.min.js' }],
    notes: 'Load images and audio from data: URLs (this.load.image("cat", k2.asset("cat.png"))).',
  },
  {
    id: 'kaplay',
    version: '3001.0.19',
    title: 'KAPLAY',
    global: 'kaplay',
    license: 'MIT',
    copyright: 'Copyright (c) 2025, KAPLAY Team and contributers',
    source: 'bundled',
    licenses: ['kaplay/LICENSE.md'],
    files: [{ name: 'kaplay.js', copy: 'kaplay/dist/kaplay.js' }],
  },
  {
    id: 'matter-js',
    version: '0.20.0',
    fingerprints: ['http://brm.io/matter-js/'],
    title: 'Matter.js',
    global: 'Matter',
    license: 'MIT',
    copyright: 'Copyright (c) Liam Brummitt and contributors.',
    source: 'bundled',
    licenses: ['matter-js/LICENSE'],
    files: [{ name: 'matter.min.js', copy: 'matter-js/build/matter.min.js' }],
  },
  // ── Charts and data ─────────────────────────────────────────────────
  {
    id: 'd3',
    version: '7.9.0',
    fingerprints: ['// https://d3js.org v'],
    title: 'D3',
    global: 'd3',
    license: 'ISC',
    copyright: 'Copyright 2010-2023 Mike Bostock',
    source: 'bundled',
    licenses: ['d3/LICENSE'],
    files: [
      { name: 'd3.min.js', copy: 'd3/dist/d3.min.js' },
      { name: 'd3-k2.js', glue: D3_GLUE },
    ],
    notes:
      'd3.json/d3.csv fetch over the network and are blocked; parse inline text instead. d3.csvParse and d3.tsvParse are K2 versions with the same results (upstream builds a function from text, which the widget security policy blocks); d3.dsvFormat(…).parse still needs eval, so use its parseRows.',
  },
  {
    id: 'chart.js',
    version: '4.5.1',
    fingerprints: ['https://www.chartjs.org'],
    title: 'Chart.js',
    global: 'Chart',
    license: 'MIT',
    copyright: 'Copyright (c) 2014-2024 Chart.js Contributors',
    source: 'bundled',
    licenses: ['chart.js/LICENSE.md'],
    files: [{ name: 'chart.umd.min.js', copy: 'chart.js/dist/chart.umd.min.js' }],
    notes: 'If a responsive chart draws at 0×0 (seen in Chrome, not WebKit), set responsive: false and give the canvas a width and height.',
  },
  {
    id: 'echarts',
    version: '6.1.0',
    title: 'Apache ECharts',
    global: 'echarts',
    license: 'Apache-2.0',
    copyright:
      'Copyright 2017-2026 The Apache Software Foundation; This product includes software developed at The Apache Software Foundation (https://www.apache.org/).',
    source: 'bundled',
    licenses: ['echarts/LICENSE', 'echarts/NOTICE', 'echarts/licenses/LICENSE-d3'],
    files: [{ name: 'echarts.min.js', copy: 'echarts/dist/echarts.min.js' }],
  },
  {
    id: 'uplot',
    version: '1.6.32',
    fingerprints: ['/*! https://github.com/leeoniya/uPlot (v'],
    title: 'uPlot',
    global: 'uPlot',
    license: 'MIT',
    copyright: 'Copyright (c) 2022 Leon Sorokin',
    source: 'bundled',
    licenses: ['uplot/LICENSE'],
    files: [
      { name: 'uPlot.iife.min.js', copy: 'uplot/dist/uPlot.iife.min.js' },
      { name: 'uplot-style.js', style: { tag: 'uplot', css: [{ path: 'uplot/dist/uPlot.min.css' }] } },
    ],
    notes: 'Small and fast time series (about 50 KB): sparklines and history lines for source trends. Give it a width and height; data is [xs, ...ys] with xs in seconds.',
  },
  // ── Animation ───────────────────────────────────────────────────────
  {
    id: 'animejs',
    version: '4.5.0',
    fingerprints: ['Anime.js - '],
    title: 'Anime.js',
    global: 'anime',
    license: 'MIT',
    copyright: 'Copyright (c) 2025 Julian Garnier',
    source: 'bundled',
    licenses: ['animejs/LICENSE.md'],
    files: [{ name: 'anime.umd.min.js', copy: 'animejs/dist/bundles/anime.umd.min.js' }],
  },
  {
    id: 'lottie-web',
    version: '5.13.0',
    title: 'Lottie',
    global: 'lottie',
    license: 'MIT',
    copyright: 'Copyright (c) 2015 Bodymovin',
    source: 'bundled',
    licenses: ['lottie-web/LICENSE.md'],
    files: [{ name: 'lottie.min.js', copy: 'lottie-web/build/player/lottie.min.js' }],
    notes: 'Pass the animation as animationData (an object), not a path. Animations that use expressions need eval and do not run.',
  },
  // ── Drawing ─────────────────────────────────────────────────────────
  {
    id: 'perfect-freehand',
    version: '1.2.3',
    title: 'perfect-freehand',
    global: 'PerfectFreehand',
    license: 'MIT',
    copyright: 'Copyright (c) 2021 Stephen Ruiz Ltd',
    source: 'bundled',
    licenses: ['perfect-freehand/LICENSE'],
    files: [{ name: 'perfect-freehand.js', bundle: "export * from 'perfect-freehand';" }],
    notes: 'PerfectFreehand.getStroke(points, options) returns the outline polygon; draw it as an SVG path or on a canvas.',
  },
  {
    id: 'roughjs',
    version: '4.6.6',
    title: 'Rough.js',
    global: 'rough',
    license: 'MIT',
    copyright: 'Copyright (c) 2019 Preet Shihn',
    source: 'bundled',
    licenses: ['roughjs/LICENSE'],
    files: [{ name: 'rough.js', copy: 'roughjs/bundled/rough.js' }],
  },
  // ── Sound ───────────────────────────────────────────────────────────
  {
    id: 'howler',
    version: '2.2.4',
    fingerprints: ['howler.js v'],
    title: 'howler.js',
    global: 'Howler',
    license: 'MIT',
    copyright: 'Copyright (c) 2013-2020 James Simpson and GoldFire Studios, Inc.',
    source: 'bundled',
    licenses: ['howler/LICENSE.md'],
    files: [{ name: 'howler.min.js', copy: 'howler/dist/howler.min.js' }],
    notes: 'Also defines Howl. Sounds play from data: URLs. Audio files are not widget assets in this version, so keep short sounds as base64 in a script, or synthesize them with Tone.js.',
  },
  {
    id: 'tone',
    version: '15.1.22',
    fingerprints: ['Tone.js.LICENSE.txt'],
    title: 'Tone.js',
    global: 'Tone',
    license: 'MIT',
    copyright: 'Copyright (c) 2014-2020 Yotam Mann',
    source: 'bundled',
    licenses: ['tone/LICENSE.md', 'tone/build/Tone.js.LICENSE.txt'],
    files: [{ name: 'Tone.js', copy: 'tone/build/Tone.js' }],
    notes:
      'Limited: AudioWorklet modules can\'t load in a sealed widget (audioWorklet.addModule(blob:) is refused: worklets need blob: scripts, which the widget policy does not allow), so worklet-based nodes fail. Oscillators, synths, effects and the Transport work. Sound starts only after a click (browser autoplay rule).',
  },
  // ── Maps ────────────────────────────────────────────────────────────
  {
    id: 'leaflet',
    version: '1.9.4',
    fingerprints: ['a JS library for interactive maps'],
    title: 'Leaflet',
    global: 'L',
    license: 'BSD-2-Clause',
    copyright: 'Copyright (c) 2010-2023, Volodymyr Agafonkin',
    source: 'bundled',
    licenses: ['leaflet/LICENSE'],
    files: [
      { name: 'leaflet.js', copy: 'leaflet/dist/leaflet.js' },
      { name: 'leaflet-style.js', style: { tag: 'leaflet', css: [{ path: 'leaflet/dist/leaflet.css' }] } },
      { name: 'leaflet-k2.js', glue: LEAFLET_GLUE },
    ],
    notes:
      'Limited: no map tiles (they come from the network). Use local image overlays (L.imageOverlay with a data: URL), GeoJSON and vector layers.',
  },
  // ── Text and code ───────────────────────────────────────────────────
  {
    id: 'marked',
    version: '18.0.14',
    fingerprints: [' - a markdown parser'],
    title: 'marked',
    global: 'marked',
    license: 'MIT',
    copyright: 'Copyright (c) 2018+, MarkedJS (https://github.com/markedjs/)',
    source: 'bundled',
    licenses: ['marked/LICENSE'],
    files: [{ name: 'marked.umd.js', copy: 'marked/lib/marked.umd.js' }],
    notes: 'marked does not sanitize: pass its HTML through DOMPurify.sanitize before showing agent text.',
  },
  {
    id: 'dompurify',
    version: '3.4.17',
    fingerprints: ['@license DOMPurify '],
    title: 'DOMPurify',
    global: 'DOMPurify',
    license: 'Apache-2.0',
    copyright: '(c) Cure53 and other contributors',
    source: 'bundled',
    licenses: ['header:dompurify/dist/purify.min.js', 'dompurify/LICENSE'],
    files: [{ name: 'purify.min.js', copy: 'dompurify/dist/purify.min.js' }],
    notes: 'Dual-licensed MPL-2.0 OR Apache-2.0 upstream; K2 ships it under Apache-2.0.',
  },
  {
    id: 'highlight.js',
    version: '11.12.0',
    fingerprints: ['Highlight.js v'],
    title: 'highlight.js',
    global: 'hljs',
    license: 'BSD-3-Clause',
    copyright: 'Copyright (c) 2006, Ivan Sagalaev.',
    source: 'bundled',
    licenses: ['@highlightjs/cdn-assets/LICENSE'],
    files: [
      { name: 'highlight.min.js', copy: '@highlightjs/cdn-assets/highlight.min.js' },
      {
        name: 'highlight-style.js',
        style: {
          tag: 'highlight.js',
          css: [
            { path: '@highlightjs/cdn-assets/styles/github.min.css' },
            // The frame's :root carries data-zen-scheme (UW39); the dark
            // theme applies under it (CSS nesting).
            { path: '@highlightjs/cdn-assets/styles/github-dark.min.css', wrap: (css) => `:root[data-zen-scheme="dark"]{${css}}` },
          ],
        },
      },
    ],
    notes: 'The common languages are built in. GitHub light and dark themes follow the Garden scheme.',
  },
  {
    id: 'katex',
    version: '0.18.9',
    title: 'KaTeX',
    global: 'katex',
    license: 'MIT',
    copyright: 'Copyright (c) 2013-2020 Khan Academy and other contributors',
    source: 'bundled',
    licenses: ['katex/LICENSE'],
    files: [
      { name: 'katex.min.js', copy: 'katex/dist/katex.min.js' },
      { name: 'auto-render.min.js', copy: 'katex/dist/contrib/auto-render.min.js' },
      { name: 'katex-style.js', style: { tag: 'katex', woff2Only: true, css: [{ path: 'katex/dist/katex.min.css' }] } },
    ],
    notes: 'Fonts are inlined (woff2). Also defines renderMathInElement (auto-render).',
  },
  // ── UI ──────────────────────────────────────────────────────────────
  {
    id: 'preact',
    version: '10.29.8',
    title: 'Preact + htm',
    global: 'preact',
    license: 'MIT AND Apache-2.0',
    copyright: 'Copyright (c) 2015-present Jason Miller; htm: Copyright 2018 Google Inc.',
    source: 'bundled',
    licenses: ['preact/LICENSE', 'htm/LICENSE'],
    files: [
      {
        name: 'preact-htm.js',
        bundle: [
          "export * from 'preact';",
          "export * from 'preact/hooks';",
          "import { h } from 'preact';",
          "import htm from 'htm';",
          'export const html = htm.bind(h);',
        ].join('\n'),
      },
    ],
    notes: 'preact.html is htm bound to preact.h: preact.render(preact.html`<b>hi</b>`, root). Hooks are on preact too (useState, useEffect, …).',
  },
  {
    id: 'lit',
    version: '3.3.3',
    title: 'Lit',
    global: 'Lit',
    license: 'BSD-3-Clause',
    copyright: 'Copyright (c) 2017 Google LLC. All rights reserved.',
    source: 'bundled',
    licenses: ['lit/LICENSE'],
    files: [
      {
        name: 'lit.js',
        bundle: [
          "export * from 'lit';",
          "export { classMap } from 'lit/directives/class-map.js';",
          "export { styleMap } from 'lit/directives/style-map.js';",
          "export { repeat } from 'lit/directives/repeat.js';",
          "export { when } from 'lit/directives/when.js';",
          "export { map } from 'lit/directives/map.js';",
          "export { ifDefined } from 'lit/directives/if-defined.js';",
          "export { live } from 'lit/directives/live.js';",
          "export { ref, createRef } from 'lit/directives/ref.js';",
        ].join('\n'),
      },
    ],
    notes: 'Lit.html, Lit.css, Lit.LitElement, Lit.render, and the classMap, styleMap, repeat, when, map, ifDefined, live and ref directives.',
  },
  // ── Fonts ───────────────────────────────────────────────────────────
  {
    id: 'font-caveat',
    version: '5.3.0',
    title: 'Caveat (handwriting font)',
    global: 'K2Fonts',
    license: 'OFL-1.1',
    copyright: 'Copyright 2014 The Caveat Project Authors (https://github.com/googlefonts/caveat)',
    source: 'bundled',
    licenses: ['@fontsource/caveat/LICENSE'],
    files: [
      {
        name: 'caveat-style.js',
        style: {
          tag: 'font-caveat',
          woff2Only: true,
          fontFaceFilter: /caveat-latin(-ext)?-(400|700)-normal/,
          css: [{ path: '@fontsource/caveat/400.css' }, { path: '@fontsource/caveat/700.css' }],
        },
      },
      { name: 'caveat-k2.js', glue: "/* K2 glue */\n(window.K2Fonts=window.K2Fonts||{}).caveat='Caveat';\n" },
    ],
    notes: "Adds the font family 'Caveat' (weights 400 and 700, Latin). Use font-family: Caveat; K2Fonts.caveat holds the name.",
  },
  // ── Downloaded on first use (R8) ────────────────────────────────────
  {
    id: 'babylonjs',
    version: '8.52.1',
    title: 'Babylon.js',
    global: 'BABYLON',
    license: 'Apache-2.0',
    copyright: 'Copyright 2023 The Babylon.js team',
    source: 'download',
    licenses: ['url:https://cdn.jsdelivr.net/npm/babylonjs@8.52.1/license.md', 'url:https://cdn.jsdelivr.net/npm/babylonjs@8.52.1/NOTICE.md'],
    files: [],
    urls: { 'babylon.js': 'https://cdn.jsdelivr.net/npm/babylonjs@8.52.1/babylon.js' },
    notes:
      'Downloaded once by K2 on first use (about 7.9 MB), then cached; refused on air-gapped computers. Babylon 9 (8.6 MB) is over the 8 MB per-widget library limit, so 8.52.1 is pinned. Leaves about 0.5 MB for other libraries in the same widget.',
  },
  {
    id: 'plotly.js-dist-min',
    version: '4.1.1',
    title: 'Plotly',
    global: 'Plotly',
    license: 'MIT',
    copyright: 'Copyright (c) 2016-2024 Plotly Technologies Inc.',
    source: 'download',
    licenses: ['url:https://cdn.jsdelivr.net/npm/plotly.js-dist-min@4.1.1/LICENSE'],
    files: [],
    urls: { 'plotly.min.js': 'https://cdn.jsdelivr.net/npm/plotly.js-dist-min@4.1.1/plotly.min.js' },
    notes: 'Downloaded once by K2 on first use (about 4.8 MB), then cached; refused on air-gapped computers.',
  },
  // ── K2's own widget kit (0.46.3) ────────────────────────────────────
  {
    id: 'k2-help',
    version: '1.0.0',
    fingerprints: ['/*! k2-help '],
    title: 'K2 help button (widget kit)',
    global: 'K2Help',
    license: 'FSL-1.1-Apache-2.0',
    copyright: 'Copyright 2025-2026 Alakazam Labs',
    source: 'bundled',
    own: true,
    licenses: ['k2:LICENSE.md'],
    files: [{ name: 'k2-help.js', own: true }],
    notes:
      "K2's own: the (i) help button. K2Help.tip(text, {label, key, title}) returns an (i) that shows a tooltip on hover or focus and pins it on click, tap, Enter or Space; text over 160 characters opens a popover with Close instead. Esc, a tap outside or the same (i) closes it; it stays inside the frame and follows the Garden's light/dark. Markup works too: class=\"k2h-btn\" with data-tip, or data-help=\"<id>\" for a popover. Put explanations behind an (i), not on the canvas.",
  },
  {
    // 0.46.5 activity SDK (review-thread-activity-surface-v1 §2.1): K2's
    // activity bar, built from sdk/activity/src (core + bar + widget) by
    // scripts/zen-lib/build-k2-activity.mjs; the built file is checked in.
    // Before k2-thread, which needs it.
    id: 'k2-activity',
    version: '1.0.0',
    fingerprints: ['/*! k2-activity '],
    title: 'K2 activity bar (widget kit)',
    global: 'K2Activity',
    license: 'FSL-1.1-Apache-2.0',
    copyright: 'Copyright 2025-2026 Alakazam Labs',
    source: 'bundled',
    own: true,
    licenses: ['k2:LICENSE.md'],
    files: [{ name: 'k2-activity.js', own: true }],
    notes: K2_ACTIVITY_NOTES,
  },
  {
    // 1.1.0 (0.46.5 activity SDK): the working tab is k2-activity@1's turn
    // from k2.thread.onActivity, never row.working or send timing.
    id: 'k2-thread',
    version: '1.1.0',
    needs: ['k2-activity@1'],
    fingerprints: ['/*! k2-thread '],
    title: 'K2 Thread panel (widget kit)',
    global: 'K2Thread',
    license: 'FSL-1.1-Apache-2.0',
    copyright: 'Copyright 2025-2026 Alakazam Labs',
    source: 'bundled',
    own: true,
    licenses: ['k2:LICENSE.md'],
    files: [{ name: 'k2-thread.js', own: true }],
    notes:
      "K2's own: a Thread panel to talk to one agent inside a widget. K2Thread.mount(el, {look: 'chamfer' or 'glass', address: 'handle::host'}) then panel.open(address, label), panel.post(text), panel.setLook(look), panel.destroy(). chamfer is K2's Thread with chamfered corners in the Garden theme's colours; glass is K2 Companion's round look and palette. Both: bubbles with author and time, choice cards, Enter sends (Shift+Enter a new line), the working tab (the agent's Thread turn in k2-activity@1's words, never a guess), stick-to-bottom scrolling with a new-messages pill, a narrow layout, file chips and data: images for attachments. Needs thread:read and thread:post (agents:read for names, activity:read for the working tab; K2 loads k2-activity@1 for you). Add marked and dompurify for markdown bodies.",
  },
  {
    // 0.46.5 (prd-widget-size-and-shared-libs-v1 WL13, vs-live L12): from
    // Kanban's md.js, safe by default, with a private DOMPurify instance.
    id: 'k2-markdown',
    version: '1.0.0',
    needs: ['marked@18', 'dompurify@3'],
    fingerprints: ['/*! k2-markdown '],
    title: 'K2 markdown (widget kit)',
    global: 'K2Markdown',
    license: 'FSL-1.1-Apache-2.0',
    copyright: 'Copyright 2025-2026 Alakazam Labs',
    source: 'bundled',
    own: true,
    licenses: ['k2:LICENSE.md'],
    files: [{ name: 'k2-markdown.js', own: true }],
    notes: K2_MARKDOWN_NOTES,
  },
  {
    // 0.46.6 (prd-gardens-on-phone-v1 Q11): canvas pixels to PNG without
    // toDataURL / toBlob, empty in a sealed frame on WebKit (Board U0).
    id: 'k2-png',
    version: '1.0.0',
    fingerprints: ['/*! k2-png '],
    title: 'K2 PNG encoder (widget kit)',
    global: 'K2Png',
    license: 'FSL-1.1-Apache-2.0',
    copyright: 'Copyright 2025-2026 Alakazam Labs',
    source: 'bundled',
    own: true,
    licenses: ['k2:LICENSE.md'],
    files: [{ name: 'k2-png.js', own: true }],
    notes:
      "K2's own: canvas pixels to a PNG without toDataURL or toBlob, which come back empty in a sealed frame on WebKit (the Mac app, iPhone, iPad). K2Png.toBlob(canvas), K2Png.toDataURL(canvas) and K2Png.encode(canvas or ImageData) read the pixels with getImageData and write an 8-bit RGBA PNG with the browser's CompressionStream; each answers a Promise. Hand the Blob to k2.blobs.put or k2.download.",
  },
]
