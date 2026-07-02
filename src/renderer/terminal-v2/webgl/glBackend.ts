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
    console.warn('[terminal-v2/webgl] shader compile failed:', gl.getShaderInfoLog(sh))
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
    console.warn('[terminal-v2/webgl] program link failed:', gl.getProgramInfoLog(prog))
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
  }) as WebGL2RenderingContext | null
  if (!gl) return null

  const rectProgram = link(gl, RECT_VS, RECT_FS)
  if (!rectProgram) return null
  const uRectResolution = gl.getUniformLocation(rectProgram, 'u_resolution')

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
      gl.deleteVertexArray(rectVao)
      gl.deleteProgram(rectProgram)
    },
  }
}
