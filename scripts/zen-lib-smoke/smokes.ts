// One smoke per Zen widget library (prd-zen-user-widgets-v2 UWB17): the
// smallest real use of the library inside a sealed widget frame, under the
// widget CSP (no network, no eval, nonced scripts only).
//
// Each value is the BODY of an async function run in the frame after the
// library's script, with:
//   root            a 320×200 <div> in the frame
//   done(ok, why)   report the result once
//   dataPng         a 4×4 PNG as a data: URL (a stand-in for k2.asset)
// A smoke that throws or never calls done fails. Results feed the
// library's `notes` in scripts/zen-lib/libs.ts (what works, what's limited).

export const SMOKES: Record<string, string> = {
  three: `
    const c = document.createElement('canvas'); c.width = 64; c.height = 64; root.appendChild(c);
    const r = new THREE.WebGLRenderer({ canvas: c, preserveDrawingBuffer: true });
    const scene = new THREE.Scene(); scene.background = new THREE.Color(0x3366ff);
    const cam = new THREE.PerspectiveCamera(50, 1, 0.1, 10); cam.position.z = 3;
    scene.add(new THREE.Mesh(new THREE.BoxGeometry(), new THREE.MeshBasicMaterial({ color: 0xff0000 })));
    new THREE.OrbitControls(cam, c);
    r.render(scene, cam);
    const gl = r.getContext(); const px = new Uint8Array(4); gl.readPixels(1, 1, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, px);
    // A glTF from a data: URL goes through fetch('data:') (connect-src data:).
    const gltf = { asset: { version: '2.0' }, scenes: [{ nodes: [] }], scene: 0 };
    const url = 'data:model/gltf+json;base64,' + btoa(JSON.stringify(gltf));
    const loaded = await new Promise((res, rej) => new THREE.GLTFLoader().load(url, res, undefined, rej));
    const tex = await new THREE.TextureLoader().loadAsync(dataPng);
    done(px[2] > 200 && !!loaded.scene && !!tex.image, 'r' + THREE.REVISION + ' px ' + Array.from(px).join(',') + ', GLTFLoader(data:) ok, texture(data:) ok');
  `,
  'globe.gl': `
    const el = document.createElement('div'); el.style.width = '200px'; el.style.height = '200px'; root.appendChild(el);
    const g = Globe()(el).width(200).height(200).globeImageUrl(dataPng).pointsData([{ lat: 10, lng: 20 }]);
    await new Promise((r) => setTimeout(r, 400));
    const canvas = el.querySelector('canvas');
    done(!!canvas && typeof g.pointOfView === 'function', canvas ? 'canvas ' + canvas.width + 'x' + canvas.height : 'no canvas');
  `,
  'cannon-es': `
    const w = new CANNON.World({ gravity: new CANNON.Vec3(0, -9.82, 0) });
    const b = new CANNON.Body({ mass: 1, shape: new CANNON.Sphere(1) }); b.position.set(0, 10, 0); w.addBody(b);
    for (let i = 0; i < 60; i++) w.step(1 / 60);
    done(b.position.y < 10, 'y after 1 s: ' + b.position.y.toFixed(2));
  `,
  'pixi.js': `
    const app = new PIXI.Application();
    await app.init({ width: 64, height: 64, background: '#3366ff', preference: 'webgl' });
    root.appendChild(app.canvas);
    const g = new PIXI.Graphics().rect(0, 0, 32, 32).fill(0xff0000);
    const t = new PIXI.Text({ text: 'hi', style: { fill: 0xffffff, fontSize: 12 } });
    app.stage.addChild(g, t);
    app.render();
    done(app.renderer.type !== undefined, 'renderer ' + (app.renderer.name || app.renderer.type) + ', ' + PIXI.VERSION);
  `,
  phaser: `
    await new Promise((resolve, reject) => {
      const t = setTimeout(() => reject(new Error('scene never created')), 8000);
      new Phaser.Game({
        type: Phaser.AUTO, width: 64, height: 64, parent: root, banner: false,
        scene: {
          preload() { this.load.image('px', dataPng); },
          create() { this.add.image(8, 8, 'px'); clearTimeout(t); resolve(this.game.renderer.type); },
        },
      });
    }).then((type) => done(true, 'renderer type ' + type + ', Phaser ' + Phaser.VERSION));
  `,
  kaplay: `
    const c = document.createElement('canvas'); c.width = 64; c.height = 64; root.appendChild(c);
    const k = kaplay({ canvas: c, width: 64, height: 64, global: false, background: [51, 102, 255] });
    k.add([k.rect(10, 10), k.pos(4, 4), k.color(255, 0, 0)]);
    await new Promise((r) => k.onUpdate(() => r()));
    done(true, 'onUpdate ran');
  `,
  'matter-js': `
    const e = Matter.Engine.create();
    const box = Matter.Bodies.rectangle(50, 0, 10, 10);
    Matter.Composite.add(e.world, [box, Matter.Bodies.rectangle(50, 100, 100, 10, { isStatic: true })]);
    for (let i = 0; i < 60; i++) Matter.Engine.update(e, 1000 / 60);
    done(box.position.y > 0, 'y ' + box.position.y.toFixed(1));
  `,
  d3: `
    const svg = d3.select(root).append('svg').attr('width', 100).attr('height', 50);
    const x = d3.scaleLinear().domain([0, 10]).range([0, 100]);
    const rows = d3.csvParse('v,w\\n1,a\\n5,\\n9,c', (d, i) => (i === 1 ? { ...d, w: '-' } : d));
    svg.selectAll('rect').data(rows).join('rect').attr('x', (d) => x(+d.v)).attr('width', 2).attr('height', 10);
    const tsv = d3.tsvParse('a\\tb\\n1\\t2');
    const ok = root.querySelectorAll('rect').length === 3 && rows.columns.join() === 'v,w' && rows[1].w === '-' && rows[2].w === 'c' && tsv[0].b === '2';
    done(ok, 'csvParse/tsvParse (K2 glue) + 3 rects: ' + JSON.stringify(rows));
  `,
  'chart.js': `
    const c = document.createElement('canvas'); root.appendChild(c);
    const ch = new Chart(c, { type: 'bar', data: { labels: ['a', 'b'], datasets: [{ data: [1, 2] }] }, options: { animation: false } });
    // Responsive sizing settles after a frame in some engines.
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    const responsive = c.width + 'x' + c.height;
    const c2 = document.createElement('canvas'); c2.width = 200; c2.height = 100; root.appendChild(c2);
    new Chart(c2, { type: 'line', data: { labels: ['a', 'b'], datasets: [{ data: [1, 2] }] }, options: { animation: false, responsive: false } });
    done(ch.data.datasets[0].data.length === 2 && c2.width > 0, 'responsive bar ' + responsive + ', fixed line ' + c2.width + 'x' + c2.height);
  `,
  echarts: `
    const el = document.createElement('div'); el.style.width = '200px'; el.style.height = '120px'; root.appendChild(el);
    const ch = echarts.init(el);
    ch.setOption({ animation: false, xAxis: { data: ['a', 'b'] }, yAxis: {}, series: [{ type: 'bar', data: [1, 2] }] });
    done(!!el.querySelector('canvas'), 'canvas renderer, echarts ' + echarts.version);
  `,
  animejs: `
    const el = document.createElement('div'); el.style.width = '10px'; el.style.height = '10px'; root.appendChild(el);
    await new Promise((r) => anime.animate(el, { translateX: 50, duration: 100, onComplete: r }));
    done(/translateX\\(50/.test(el.style.transform), 'transform ' + el.style.transform);
  `,
  'lottie-web': `
    const data = { v: '5.7.4', fr: 30, ip: 0, op: 30, w: 50, h: 50, nm: 'k2', ddd: 0, assets: [],
      layers: [{ ddd: 0, ind: 1, ty: 4, nm: 's', sr: 1, ks: { o: { a: 0, k: 100 }, r: { a: 0, k: 0 }, p: { a: 0, k: [25, 25, 0] }, a: { a: 0, k: [0, 0, 0] }, s: { a: 0, k: [100, 100, 100] } },
        ao: 0, shapes: [{ ty: 'rc', d: 1, s: { a: 0, k: [20, 20] }, p: { a: 0, k: [0, 0] }, r: { a: 0, k: 0 }, nm: 'r' }, { ty: 'fl', c: { a: 0, k: [1, 0, 0, 1] }, o: { a: 0, k: 100 }, r: 1, nm: 'f' }],
        ip: 0, op: 30, st: 0, bm: 0 }] };
    const a = lottie.loadAnimation({ container: root, renderer: 'svg', loop: false, autoplay: true, animationData: data });
    await new Promise((r) => a.addEventListener('DOMLoaded', r));
    done(!!root.querySelector('svg'), 'svg renderer, ' + a.totalFrames + ' frames');
  `,
  'perfect-freehand': `
    const outline = PerfectFreehand.getStroke([[0, 0], [10, 5], [20, 20], [40, 25]], { size: 8 });
    done(Array.isArray(outline) && outline.length > 4, outline.length + ' outline points');
  `,
  roughjs: `
    const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg'); root.appendChild(svg);
    svg.appendChild(rough.svg(svg).rectangle(5, 5, 50, 30, { fill: 'red' }));
    done(svg.querySelectorAll('path').length > 0, svg.querySelectorAll('path').length + ' paths');
  `,
  howler: `
    // 0.05 s of silence, 8 kHz 8-bit mono WAV.
    const n = 400, b = new Uint8Array(44 + n), v = new DataView(b.buffer);
    const s = (o, t) => { for (let i = 0; i < t.length; i++) b[o + i] = t.charCodeAt(i); };
    s(0, 'RIFF'); v.setUint32(4, 36 + n, true); s(8, 'WAVEfmt '); v.setUint32(16, 16, true); v.setUint16(20, 1, true); v.setUint16(22, 1, true);
    v.setUint32(24, 8000, true); v.setUint32(28, 8000, true); v.setUint16(32, 1, true); v.setUint16(34, 8, true); s(36, 'data'); v.setUint32(40, n, true); b.fill(128, 44);
    let bin = ''; for (const x of b) bin += String.fromCharCode(x);
    const h = new Howl({ src: ['data:audio/wav;base64,' + btoa(bin)], format: ['wav'] });
    const state = await new Promise((r) => { h.once('load', () => r('loaded')); h.once('loaderror', (_, e) => r('loaderror ' + e)); setTimeout(() => r('state ' + h.state()), 3000); });
    done(state === 'loaded', state + ', ctx ' + (Howler.ctx ? Howler.ctx.state : 'none'));
  `,
  tone: `
    const synth = new Tone.Synth().toDestination();
    const ctx = Tone.getContext();
    let worklet = 'n/a';
    try {
      const raw = ctx.rawContext;
      if (raw.audioWorklet) {
        const url = URL.createObjectURL(new Blob(['registerProcessor("k2-noop", class extends AudioWorkletProcessor { process() { return true } })'], { type: 'text/javascript' }));
        await raw.audioWorklet.addModule(url).then(() => { worklet = 'loads'; }, (e) => { worklet = 'blocked (' + (e && e.name) + ')'; });
      }
    } catch (e) { worklet = 'blocked (' + (e && e.name) + ')'; }
    done(!!synth && !!ctx, 'Synth ok, AudioWorklet addModule(blob:) ' + worklet + ', Tone ' + Tone.version);
  `,
  leaflet: `
    const el = document.createElement('div'); el.style.width = '200px'; el.style.height = '150px'; root.appendChild(el);
    const map = L.map(el, { crs: L.CRS.Simple, minZoom: -2 });
    const bounds = [[0, 0], [100, 100]];
    L.imageOverlay(dataPng, bounds).addTo(map); map.fitBounds(bounds);
    const m = L.marker([50, 50]).addTo(map);
    const icon = m.getElement();
    const css = getComputedStyle(el.querySelector('.leaflet-container') || el).position;
    done(!!icon && icon.src.startsWith('data:image/png') && css === 'relative', 'marker icon ' + (icon ? icon.src.slice(0, 22) : 'none') + ', container ' + css);
  `,
  marked: `
    const html = marked.parse('# hi\\n\\n*there*');
    done(/<h1[^>]*>hi<\\/h1>/.test(html) && html.includes('<em>there</em>'), html.replace(/\\n/g, ' '));
  `,
  dompurify: `
    const out = DOMPurify.sanitize('<img src=x onerror=alert(1)><b>ok</b>');
    done(!out.includes('onerror') && out.includes('<b>ok</b>'), out);
  `,
  'highlight.js': `
    const r = hljs.highlight('const x = 1', { language: 'javascript' });
    const styled = !!document.querySelector('style[data-k2-lib="highlight.js"]');
    done(r.value.includes('hljs-keyword') && styled, hljs.listLanguages().length + ' languages, theme style ' + styled);
  `,
  katex: `
    katex.render('x^2 + \\\\frac{1}{2}', root);
    await document.fonts.load('16px KaTeX_Main');
    const fontOk = document.fonts.check('16px KaTeX_Main');
    const auto = typeof renderMathInElement === 'function';
    done(!!root.querySelector('.katex') && fontOk && auto, 'rendered, KaTeX_Main font ' + fontOk + ', auto-render ' + auto);
  `,
  preact: `
    const { html, render, useState } = preact;
    function App() { const [n] = useState(3); return html\`<b id="p">n=\${n}</b>\`; }
    render(html\`<\${App} />\`, root);
    done(root.querySelector('#p')?.textContent === 'n=3', root.innerHTML);
  `,
  lit: `
    class K2Smoke extends Lit.LitElement {
      static properties = { n: { type: Number } };
      constructor() { super(); this.n = 4; }
      render() { return Lit.html\`<i>n=\${this.n}</i>\`; }
    }
    customElements.define('k2-smoke', K2Smoke);
    const el = document.createElement('k2-smoke'); root.appendChild(el);
    await el.updateComplete;
    done(el.shadowRoot?.textContent === 'n=4', 'shadow ' + el.shadowRoot?.innerHTML.replace(/<!--[^>]*-->/g, ''));
  `,
  'font-caveat': `
    await document.fonts.load('20px Caveat');
    const ok = document.fonts.check('20px Caveat');
    done(ok && K2Fonts.caveat === 'Caveat', 'Caveat loaded ' + ok);
  `,
  babylonjs: `
    const c = document.createElement('canvas'); c.width = 64; c.height = 64; root.appendChild(c);
    const engine = new BABYLON.Engine(c, true, { preserveDrawingBuffer: true });
    const scene = new BABYLON.Scene(engine);
    new BABYLON.FreeCamera('cam', new BABYLON.Vector3(0, 0, -5), scene);
    new BABYLON.HemisphericLight('l', new BABYLON.Vector3(0, 1, 0), scene);
    BABYLON.MeshBuilder.CreateBox('b', {}, scene);
    await scene.whenReadyAsync();
    scene.render();
    done(engine.webGLVersion >= 1, 'WebGL ' + engine.webGLVersion + ', Babylon ' + BABYLON.Engine.Version);
  `,
  'plotly.js-dist-min': `
    const el = document.createElement('div'); el.style.width = '300px'; el.style.height = '200px'; root.appendChild(el);
    await Plotly.newPlot(el, [{ x: [1, 2, 3], y: [2, 1, 3], type: 'scatter' }], { margin: { t: 0 } }, { staticPlot: true });
    let gl = 'n/a';
    try {
      const el2 = document.createElement('div'); el2.style.width = '200px'; el2.style.height = '120px'; root.appendChild(el2);
      await Plotly.newPlot(el2, [{ x: [1, 2], y: [1, 2], type: 'scattergl' }]);
      gl = 'ok';
    } catch (e) { gl = 'fails (' + ((e && e.message) || e) + ')'; }
    done(!!el.querySelector('svg.main-svg'), 'scatter (svg) ok, scattergl ' + gl + ', Plotly ' + Plotly.version);
  `,
}
