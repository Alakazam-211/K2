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
export const LIBS: LibSpec[] = [
  // ── 3D and physics ──────────────────────────────────────────────────
  {
    id: 'three',
    version: '0.186.0',
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
  // ── Animation ───────────────────────────────────────────────────────
  {
    id: 'animejs',
    version: '4.5.0',
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
    version: '3.4.16',
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
]
