// What the mouse and the keyboard did on the phone's screen, as the phone
// should feel it. Loaded by the page as a script (window.Gesture) and
// required by the tests; nothing here touches the page.
//
//   a click                         a tap
//   held still ≥ 0.45 s             a long press, as long as it was held
//   dragged                         a drag along the path, at its own speed
//                                   (the speed at the end flings a list)
//   held ≥ 0.35 s, then dragged     a press and a drag: an icon picked up
//
// Places are fractions of the frame (0..1 across, 0..1 down), so the Rust
// side turns them into the phone's points whatever size the window is.
;(function (root) {
  const TAP_SLOP_PX = 6
  const LONG_PRESS_MS = 450
  const HOLD_BEFORE_DRAG_MS = 350
  // A drag's steps no closer in time than this (WDA takes each as a move).
  const MIN_STEP_MS = 25

  // The frame drawn as big as the box lets it, in its middle.
  function fit(frameW, frameH, boxW, boxH) {
    if (!frameW || !frameH || !boxW || !boxH) return null
    const scale = Math.min(boxW / frameW, boxH / frameH)
    const w = frameW * scale
    const h = frameH * scale
    return { x: (boxW - w) / 2, y: (boxH - h) / 2, w, h }
  }

  // A point in the box as fractions of the frame drawn there; null outside it
  // (unless `clamp`: a drag that leaves the frame goes on along its edge).
  function toFraction(rect, px, py, clamp = false) {
    if (!rect) return null
    const fx = (px - rect.x) / rect.w
    const fy = (py - rect.y) / rect.h
    const inside = fx >= 0 && fx <= 1 && fy >= 0 && fy <= 1
    if (!inside && !clamp) return null
    return [Math.min(Math.max(fx, 0), 1), Math.min(Math.max(fy, 0), 1)]
  }

  // The gesture of one press: points are { x, y, px, py, t }: fractions of
  // the frame, pixels in the box (for how far it moved) and ms.
  function gestureOf(points) {
    if (!points || !points.length) return null
    const first = points[0]
    const last = points[points.length - 1]
    const far = points.findIndex(p => Math.abs(p.px - first.px) + Math.abs(p.py - first.py) > TAP_SLOP_PX)
    if (far === -1) {
      const ms = Math.round(last.t - first.t)
      return ms >= LONG_PRESS_MS ? { kind: 'press', x: first.x, y: first.y, ms } : { kind: 'tap', x: first.x, y: first.y }
    }
    const still = points[far - 1].t - first.t
    const hold = still >= HOLD_BEFORE_DRAG_MS ? Math.round(still) : 0
    const steps = [[first.x, first.y, 0]]
    let at = hold ? points[far - 1].t : first.t
    for (const p of points.slice(far, -1)) {
      if (p.t - at >= MIN_STEP_MS) {
        steps.push([p.x, p.y, Math.round(p.t - at)])
        at = p.t
      }
    }
    steps.push([last.x, last.y, Math.max(Math.round(last.t - at), 10)])
    return { kind: 'drag', steps, hold }
  }

  // What a key typed on the phone: a letter as it is, Backspace, Enter and
  // Tab as WDA's characters; null for the rest (shortcuts, F keys, arrows).
  function keyOf(e) {
    if (e.isComposing || e.ctrlKey || e.altKey || e.metaKey) return null
    if (e.key === 'Backspace') return '\b'
    if (e.key === 'Enter') return '\n'
    if (e.key === 'Tab') return '\t'
    return null
  }

  // A frame's type from its first bytes: the stream's JPEGs, or a
  // screenshot's PNG when the stream fell back to screenshots.
  function imageType(bytes) {
    if (bytes && bytes[0] === 0x89 && bytes[1] === 0x50) return 'image/png'
    return 'image/jpeg'
  }

  // Wide or tall: the screen turned when this changes.
  const landscape = (w, h) => w > h

  const api = { TAP_SLOP_PX, LONG_PRESS_MS, HOLD_BEFORE_DRAG_MS, MIN_STEP_MS, fit, toFraction, gestureOf, keyOf, imageType, landscape }

  if (typeof module !== 'undefined' && module.exports) {
    module.exports = api
  } else {
    root.Gesture = api
  }
})(this)
