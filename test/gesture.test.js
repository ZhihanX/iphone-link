const assert = require('node:assert/strict')
const { test } = require('node:test')

const G = require('../src/phone/gesture')

// A press at fractions (x, y), pixels (px, py), at ms t.
const at = (x, y, px, py, t) => ({ x, y, px, py, t })

test('a still click is a tap, held long it is a long press', () => {
  assert.deepEqual(G.gestureOf([at(0.5, 0.5, 100, 200, 0), at(0.5, 0.5, 101, 201, 120)]), { kind: 'tap', x: 0.5, y: 0.5 })
  assert.deepEqual(G.gestureOf([at(0.2, 0.3, 10, 10, 0), at(0.2, 0.3, 12, 10, 600)]), { kind: 'press', x: 0.2, y: 0.3, ms: 600 })
  assert.equal(G.gestureOf([]), null)
})

test('a drag keeps its path and its speed, a step at most every 25 ms', () => {
  const pts = [at(0.5, 0.8, 100, 400, 0)]
  for (let i = 1; i <= 10; i++) pts.push(at(0.5, 0.8 - i * 0.05, 100, 400 - i * 20, i * 10))
  const g = G.gestureOf(pts)
  assert.equal(g.kind, 'drag')
  assert.equal(g.hold, 0)
  assert.deepEqual(g.steps[0], [0.5, 0.8, 0])
  const last = g.steps[g.steps.length - 1]
  assert.deepEqual(last.slice(0, 2), [0.5, 0.8 - 10 * 0.05])
  for (const s of g.steps.slice(1, -1)) assert.ok(s[2] >= G.MIN_STEP_MS, `step of ${s[2]} ms`)
  const total = g.steps.reduce((sum, s) => sum + s[2], 0)
  assert.ok(total <= 100 && total >= 90, `took ${total} ms, as the mouse did`)
})

test('held before moving, a drag picks up what is under it', () => {
  const g = G.gestureOf([at(0.1, 0.1, 10, 10, 0), at(0.1, 0.1, 11, 10, 500), at(0.3, 0.1, 60, 10, 700)])
  assert.equal(g.kind, 'drag')
  assert.equal(g.hold, 500)
  assert.deepEqual(g.steps, [[0.1, 0.1, 0], [0.3, 0.1, 200]])
})

test('the frame fits the box in its middle, and points become its fractions', () => {
  // A tall frame in a wide box: bars left and right.
  const r = G.fit(390, 844, 800, 844)
  assert.deepEqual(r, { x: 205, y: 0, w: 390, h: 844 })
  assert.deepEqual(G.toFraction(r, 205 + 195, 422), [0.5, 0.5])
  assert.equal(G.toFraction(r, 100, 422), null, 'on the bar')
  assert.deepEqual(G.toFraction(r, 100, 422, true), [0, 0.5], 'a drag goes on along the edge')
  assert.equal(G.fit(0, 0, 100, 100), null)
})

test('only Backspace, Enter and Tab are keys; letters come as text', () => {
  assert.equal(G.keyOf({ key: 'Backspace' }), '\b')
  assert.equal(G.keyOf({ key: 'Enter' }), '\n')
  assert.equal(G.keyOf({ key: 'Tab' }), '\t')
  assert.equal(G.keyOf({ key: 'a' }), null)
  assert.equal(G.keyOf({ key: 'Enter', isComposing: true }), null, 'the IME takes it')
  assert.equal(G.keyOf({ key: 'Backspace', ctrlKey: true }), null)
})

test('a screenshot fallback is a PNG, the stream JPEGs', () => {
  assert.equal(G.imageType(new Uint8Array([0x89, 0x50, 0x4e, 0x47])), 'image/png')
  assert.equal(G.imageType(new Uint8Array([0xff, 0xd8, 0xff])), 'image/jpeg')
})
