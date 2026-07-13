// Narrow WebGL2 backend for the terminal painter.
//
// The painter orchestration (row cache, frame packing, scroll
// windowing, context-loss policy) is written against THIS interface,
// not against WebGL2RenderingContext — so unit tests stub a ~30-line
// fake backend instead of a GL context, and every GL call site lives
// in this one file. Rendering discipline follows xterm.js's WebGL
// addon: context `{alpha:false, antialias:false, depth:false}`, one
// STREAM_DRAW upload + one instanced draw per pass, standard
// SRC_ALPHA/ONE_MINUS_SRC_ALPHA blending enabled once.

export interface GlyphDrawUniforms {
  cols: number
  cellW: number
  cellH: number
  /** Sub-cell scroll offset, device px (content shifts up). */
  scrollY: number
  texW: number
  texH: number
  /** Coverage-gamma exponent for tinted glyphs (text-weight tuning,
   *  the "chonky text" fix): AA edge coverage is raised to this
   *  power, thinning (>1) or fattening (<1) glyph edges without
   *  touching fully-covered pixels. 1 = raw atlas coverage. */
  textGamma: number
}

export interface PainterBackend {
  /** Set the drawing-buffer size (device px) + per-program
   *  resolution uniforms. */
  resize(deviceW: number, deviceH: number): void
  /** Viewport + opaque clear to the theme background (0xRRGGBB) —
   *  the full-viewport bg "rect" of the brief's pass order. */
  beginFrame(bgColor: number): void
  /** Instanced rect pass: 8 floats per rect (x y w h, rgba 0–1),
   *  device px. Used for backgrounds, selection and decorations. */
  drawRects(data: Float32Array, count: number): void
  /** (Re-)upload the glyph atlas page. Whole-page texImage2D keyed
   *  off the atlas version by the caller — xterm's dead-simple
   *  upload, no partial updates. */
  uploadAtlas(source: TexImageSource): void
  /** Instanced glyph pass: fixed cell slots, GLYPH_FLOATS floats per
   *  instance (see packFrame.ts for the layout); one draw. */
  drawGlyphs(
    data: Float32Array,
    instanceCount: number,
    u: GlyphDrawUniforms,
  ): void
  /** Read one pixel (device coords, y-down like the rect space) from
   *  the drawing buffer — sanity probe. Null when the read fails. */
  readPixel(x: number, y: number): [number, number, number, number] | null
  dispose(): void
}

const RECT_VS = `#version 300 es
precision highp float;
layout(location=0) in vec2 a_unit;
layout(location=1) in vec4 a_rect;
layout(location=2) in vec4 a_color;
uniform vec2 u_resolution;
out vec4 v_color;
void main() {
  vec2 px = a_rect.xy + a_unit * a_rect.zw;
  vec2 clip = px / u_resolution * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
  v_color = a_color;
}`

const RECT_FS = `#version 300 es
precision highp float;
in vec4 v_color;
out vec4 outColor;
void main() { outColor = v_color; }`

// Cell position derives from gl_InstanceID (fixed slot per cell) —
// no per-instance cellpos attribute (brief §2.3). Texture coords are
// atlas PIXELS normalized here by u_texSize, so atlas growth leaves
// packed instance data valid.
const GLYPH_VS = `#version 300 es
precision highp float;
layout(location=0) in vec2 a_unit;
layout(location=1) in vec4 a_geom;
layout(location=2) in vec2 a_tex;
layout(location=3) in vec4 a_color;
layout(location=4) in float a_flags;
uniform vec2 u_resolution;
uniform int u_cols;
uniform vec2 u_cell;
uniform float u_scrollY;
uniform vec2 u_texSize;
out vec2 v_uv;
out vec4 v_color;
flat out float v_flags;
void main() {
  int col = gl_InstanceID % u_cols;
  int row = gl_InstanceID / u_cols;
  vec2 origin = vec2(float(col) * u_cell.x, float(row) * u_cell.y - u_scrollY);
  vec2 px = origin + a_geom.xy + a_unit * a_geom.zw;
  vec2 clip = px / u_resolution * 2.0 - 1.0;
  gl_Position = vec4(clip.x, -clip.y, 0.0, 1.0);
  v_uv = (a_tex + a_unit * a_geom.zw) / u_texSize;
  v_color = a_color;
  v_flags = a_flags;
}`

// Monochrome glyphs: the white atlas sample is a coverage mask —
// tint with the per-instance fg (AA fringes tint with the glyph,
// never halo). Coverage is raised to u_gamma before tinting: macOS
// rasterizes Canvas2D text with dilated ("smoothed") stems while the
// DOM strip renders -webkit-font-smoothing:antialiased, so raw atlas
// coverage reads visibly bolder than the DOM at the same font —
// u_gamma > 1 thins the AA edges back (pow leaves solid pixels and
// blank pixels alone; shades ░▒▓ shift slightly, acceptably). Color
// glyphs (emoji): sample directly, fg alpha still applies (dim
// emoji), no gamma (their alpha is content, not edge coverage).
const GLYPH_FS = `#version 300 es
precision highp float;
uniform sampler2D u_atlas;
uniform float u_gamma;
in vec2 v_uv;
in vec4 v_color;
flat in float v_flags;
out vec4 outColor;
void main() {
  vec4 s = texture(u_atlas, v_uv);
  if (v_flags > 0.5) {
    outColor = vec4(s.rgb, s.a * v_color.a);
  } else {
    outColor = vec4(v_color.rgb, v_color.a * pow(s.a, u_gamma));
  }
}`

function compile(
  gl: WebGL2RenderingContext,
  type: number,
  src: string,
): WebGLShader | null {
  const sh = gl.createShader(type)
  if (!sh) return null
  gl.shaderSource(sh, src)
  gl.compileShader(sh)
  if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
    // eslint-disable-next-line no-console
    console.warn('[kessel-term/webgl] shader compile failed:', gl.getShaderInfoLog(sh))
    gl.deleteShader(sh)
    return null
  }
  return sh
}

function link(
  gl: WebGL2RenderingContext,
  vsSrc: string,
  fsSrc: string,
): WebGLProgram | null {
  const vs = compile(gl, gl.VERTEX_SHADER, vsSrc)
  const fs = compile(gl, gl.FRAGMENT_SHADER, fsSrc)
  if (!vs || !fs) return null
  const prog = gl.createProgram()
  if (!prog) return null
  gl.attachShader(prog, vs)
  gl.attachShader(prog, fs)
  gl.linkProgram(prog)
  // Shaders are owned by the program from here on.
  gl.deleteShader(vs)
  gl.deleteShader(fs)
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    // eslint-disable-next-line no-console
    console.warn('[kessel-term/webgl] program link failed:', gl.getProgramInfoLog(prog))
    gl.deleteProgram(prog)
    return null
  }
  return prog
}

/** Create the real WebGL2 backend, or null when WebGL2 / shader
 *  compilation is unavailable (caller falls back to the DOM strip). */
export function createWebgl2Backend(
  canvas: HTMLCanvasElement,
): PainterBackend | null {
  const gl = canvas.getContext('webgl2', {
    alpha: false,
    antialias: false,
    depth: false,
    stencil: false,
    preserveDrawingBuffer: false,
    // Shorter input-to-photon present path where honored (Chromium/
    // WebView2 desync the canvas from the compositor; WebKit ignores
    // it). Every visible cell redraws each frame, so neither hint
    // risks stale content.
    desynchronized: true,
    powerPreference: 'high-performance',
  }) as WebGL2RenderingContext | null
  if (!gl) return null

  const rectProgram = link(gl, RECT_VS, RECT_FS)
  if (!rectProgram) return null
  const uRectResolution = gl.getUniformLocation(rectProgram, 'u_resolution')

  const glyphProgram = link(gl, GLYPH_VS, GLYPH_FS)
  if (!glyphProgram) return null
  const uGlyphResolution = gl.getUniformLocation(glyphProgram, 'u_resolution')
  const uGlyphCols = gl.getUniformLocation(glyphProgram, 'u_cols')
  const uGlyphCell = gl.getUniformLocation(glyphProgram, 'u_cell')
  const uGlyphScrollY = gl.getUniformLocation(glyphProgram, 'u_scrollY')
  const uGlyphTexSize = gl.getUniformLocation(glyphProgram, 'u_texSize')
  const uGlyphAtlas = gl.getUniformLocation(glyphProgram, 'u_atlas')
  const uGlyphGamma = gl.getUniformLocation(glyphProgram, 'u_gamma')

  // Shared unit quad (TRIANGLE_STRIP): (0,0)(1,0)(0,1)(1,1).
  const unitQuad = gl.createBuffer()
  gl.bindBuffer(gl.ARRAY_BUFFER, unitQuad)
  gl.bufferData(
    gl.ARRAY_BUFFER,
    new Float32Array([0, 0, 1, 0, 0, 1, 1, 1]),
    gl.STATIC_DRAW,
  )

  const rectVao = gl.createVertexArray()
  const rectInstances = gl.createBuffer()
  gl.bindVertexArray(rectVao)
  gl.bindBuffer(gl.ARRAY_BUFFER, unitQuad)
  gl.enableVertexAttribArray(0)
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0)
  gl.bindBuffer(gl.ARRAY_BUFFER, rectInstances)
  gl.enableVertexAttribArray(1)
  gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 32, 0)
  gl.vertexAttribDivisor(1, 1)
  gl.enableVertexAttribArray(2)
  gl.vertexAttribPointer(2, 4, gl.FLOAT, false, 32, 16)
  gl.vertexAttribDivisor(2, 1)
  gl.bindVertexArray(null)

  // Glyph VAO: 12 floats / 48 B stride per instance (packFrame.ts
  // documents the slot layout).
  const glyphVao = gl.createVertexArray()
  const glyphInstances = gl.createBuffer()
  gl.bindVertexArray(glyphVao)
  gl.bindBuffer(gl.ARRAY_BUFFER, unitQuad)
  gl.enableVertexAttribArray(0)
  gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0)
  gl.bindBuffer(gl.ARRAY_BUFFER, glyphInstances)
  gl.enableVertexAttribArray(1)
  gl.vertexAttribPointer(1, 4, gl.FLOAT, false, 48, 0)
  gl.vertexAttribDivisor(1, 1)
  gl.enableVertexAttribArray(2)
  gl.vertexAttribPointer(2, 2, gl.FLOAT, false, 48, 16)
  gl.vertexAttribDivisor(2, 1)
  gl.enableVertexAttribArray(3)
  gl.vertexAttribPointer(3, 4, gl.FLOAT, false, 48, 24)
  gl.vertexAttribDivisor(3, 1)
  gl.enableVertexAttribArray(4)
  gl.vertexAttribPointer(4, 1, gl.FLOAT, false, 48, 40)
  gl.vertexAttribDivisor(4, 1)
  gl.bindVertexArray(null)

  // Atlas texture: 1:1 sampling on an integer device grid → NEAREST
  // (no mipmaps — brief §7.3: xterm's per-upload generateMipmap is
  // wasted work).
  const atlasTexture = gl.createTexture()
  gl.bindTexture(gl.TEXTURE_2D, atlasTexture)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MIN_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_MAG_FILTER, gl.NEAREST)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_S, gl.CLAMP_TO_EDGE)
  gl.texParameteri(gl.TEXTURE_2D, gl.TEXTURE_WRAP_T, gl.CLAMP_TO_EDGE)

  gl.enable(gl.BLEND)
  gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA)

  let width = 0
  let height = 0

  return {
    resize(deviceW: number, deviceH: number): void {
      width = deviceW
      height = deviceH
      gl.useProgram(rectProgram)
      gl.uniform2f(uRectResolution, deviceW, deviceH)
      gl.useProgram(glyphProgram)
      gl.uniform2f(uGlyphResolution, deviceW, deviceH)
    },

    beginFrame(bgColor: number): void {
      gl.viewport(0, 0, width, height)
      gl.clearColor(
        ((bgColor >> 16) & 0xff) / 255,
        ((bgColor >> 8) & 0xff) / 255,
        (bgColor & 0xff) / 255,
        1,
      )
      gl.clear(gl.COLOR_BUFFER_BIT)
    },

    drawRects(data: Float32Array, count: number): void {
      if (count <= 0) return
      gl.useProgram(rectProgram)
      gl.bindVertexArray(rectVao)
      gl.bindBuffer(gl.ARRAY_BUFFER, rectInstances)
      gl.bufferData(gl.ARRAY_BUFFER, data.subarray(0, count * 8), gl.STREAM_DRAW)
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, count)
      gl.bindVertexArray(null)
    },

    uploadAtlas(source: TexImageSource): void {
      gl.bindTexture(gl.TEXTURE_2D, atlasTexture)
      gl.texImage2D(gl.TEXTURE_2D, 0, gl.RGBA, gl.RGBA, gl.UNSIGNED_BYTE, source)
    },

    drawGlyphs(
      data: Float32Array,
      instanceCount: number,
      u: GlyphDrawUniforms,
    ): void {
      if (instanceCount <= 0) return
      gl.useProgram(glyphProgram)
      gl.uniform1i(uGlyphCols, u.cols)
      gl.uniform2f(uGlyphCell, u.cellW, u.cellH)
      gl.uniform1f(uGlyphScrollY, u.scrollY)
      gl.uniform2f(uGlyphTexSize, u.texW, u.texH)
      gl.uniform1i(uGlyphAtlas, 0)
      gl.uniform1f(uGlyphGamma, u.textGamma)
      gl.activeTexture(gl.TEXTURE0)
      gl.bindTexture(gl.TEXTURE_2D, atlasTexture)
      gl.bindVertexArray(glyphVao)
      gl.bindBuffer(gl.ARRAY_BUFFER, glyphInstances)
      gl.bufferData(
        gl.ARRAY_BUFFER,
        data.subarray(0, instanceCount * 12),
        gl.STREAM_DRAW,
      )
      gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, instanceCount)
      gl.bindVertexArray(null)
    },

    readPixel(x: number, y: number): [number, number, number, number] | null {
      const out = new Uint8Array(4)
      try {
        // GL reads y-up; the painter addresses y-down like its rects.
        gl.readPixels(x, height - 1 - y, 1, 1, gl.RGBA, gl.UNSIGNED_BYTE, out)
      } catch {
        return null
      }
      if (gl.getError() !== gl.NO_ERROR) return null
      return [out[0], out[1], out[2], out[3]]
    },

    dispose(): void {
      gl.deleteBuffer(unitQuad)
      gl.deleteBuffer(rectInstances)
      gl.deleteBuffer(glyphInstances)
      gl.deleteVertexArray(rectVao)
      gl.deleteVertexArray(glyphVao)
      gl.deleteTexture(atlasTexture)
      gl.deleteProgram(rectProgram)
      gl.deleteProgram(glyphProgram)
    },
  }
}
