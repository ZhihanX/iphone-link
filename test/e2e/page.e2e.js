// The phone page in Chromium (WebView2's engine), with a Tauri faked in the
// page: the mouse, the keyboard, an IME's word, the buttons, the settings,
// an error and the go-ios download, checked by what the page invokes.
// Optional (it needs playwright-core and a Chromium; test/*.test.js need
// nothing): docs/DEVELOPMENT.md, "测试".
//
//   npm i --no-save playwright-core && node test/e2e/page.e2e.js
//   (CHROMIUM=<path to chrome> if Playwright's own is not installed)
const path = require('node:path')
const { pathToFileURL } = require('node:url')
const { chromium } = require('playwright-core')
const assert = require('node:assert/strict')
const PAGE = pathToFileURL(path.join(__dirname, '..', '..', 'src', 'phone', 'index.html')).href
const SHOTS = process.env.SHOTS || ''

// A Tauri in the page: invoke answers from window.__answers, every call kept.
const MOCK = ({ goios }) => {
  window.__calls = []
  window.__listeners = {}
  window.__channels = []
  window.__answers = {
    phone_state: { goios: goios ? '/x/ios.exe' : null, goiosVersion: 'v1.3.2', connected: false },
    phone_devices: { devices: [{ udid: 'U1', name: 'Zhihan的iPhone', ios: '17.4', model: 'iPhone15,2', connection: 'USB' }] },
    phone_connect: { ok: true, device: { udid: 'U1', name: 'Zhihan的iPhone', ios: '17.4' }, size: { width: 390, height: 844 } },
    phone_setup: { ok: true, path: '/x/ios.exe' },
    phone_screenshot: { path: 'C:\\Users\\z\\Pictures\\iPhone Link\\iPhone-1.png' },
  }
  class Channel { constructor() { this.onmessage = () => {}; window.__channels.push(this) } }
  window.__TAURI__ = {
    core: {
      Channel,
      invoke: async (cmd, args) => {
        window.__calls.push([cmd, args || {}])
        if (cmd === 'phone_setup') {
          for (const pct of [10, 60, 100]) (window.__listeners['phone:setup'] || []).forEach(f => f({ payload: pct }))
        }
        const a = window.__answers[cmd]
        return a === undefined ? { ok: true } : JSON.parse(JSON.stringify(a))
      },
    },
    event: { listen: async (name, fn) => { (window.__listeners[name] ||= []).push(fn) } },
  }
  window.__emit = (name, payload) => (window.__listeners[name] || []).forEach(f => f({ payload }))
}

const calls = page => page.evaluate(() => window.__calls)
const named = async (page, cmd) => (await calls(page)).filter(c => c[0] === cmd)
const until = async (fn, ms = 3000) => { const end = Date.now() + ms; while (Date.now() < end) { if (await fn()) return; await new Promise(r => setTimeout(r, 30)) } throw new Error('timed out') }

;(async () => {
  const browser = await chromium.launch(process.env.CHROMIUM ? { executablePath: process.env.CHROMIUM } : {})
  const shot = (page, name) => SHOTS && page.screenshot({ path: path.join(SHOTS, name) })
  const errors = []

  // --- Connected, the screen, the mouse, the keyboard ---
  const ctx = await browser.newContext({ viewport: { width: 920, height: 900 }, locale: 'zh-CN' })
  const page = await ctx.newPage()
  page.on('pageerror', e => errors.push(String(e)))
  page.on('console', m => m.type() === 'error' && errors.push(m.text()))
  await page.addInitScript(MOCK, { goios: true })
  await page.goto(PAGE)
  await until(async () => (await page.locator('#device option').first().textContent()).includes('Zhihan'))
  assert.equal(await page.locator('#device option').first().textContent(), 'Zhihan的iPhone（iOS 17.4）')
  assert.ok(await page.locator('[data-button=home]').isDisabled(), 'controls off before connecting')
  await shot(page, 'shot-idle.png')

  await page.click('#connect')
  await until(async () => (await named(page, 'phone_stream')).length === 1)
  const [, connectArgs] = (await named(page, 'phone_connect'))[0]
  assert.deepEqual(connectArgs, { udid: 'U1', direct: null, bundle: null, quality: 'standard' })
  assert.equal(await page.locator('#connect').textContent(), '断开')
  assert.ok(!(await page.locator('[data-button=home]').isDisabled()))

  // A frame: a 390x844 JPEG made in the page, sent as the channel would.
  await page.evaluate(async () => {
    const c = new OffscreenCanvas(390, 844)
    const g = c.getContext('2d')
    g.fillStyle = '#3366cc'; g.fillRect(0, 0, 390, 844)
    g.fillStyle = '#fff'; g.font = '40px sans-serif'; g.fillText('12:34', 140, 200)
    g.fillStyle = '#ffcc00'; for (let i = 0; i < 4; i++) for (let j = 0; j < 6; j++) g.fillRect(30 + i * 90, 300 + j * 90, 60, 60)
    const buf = await (await c.convertToBlob({ type: 'image/jpeg' })).arrayBuffer()
    window.__channels[0].onmessage(buf)
  })
  await until(async () => (await named(page, 'phone_seen')).length === 1)
  assert.equal(await page.locator('#placeholder').isHidden(), true)

  // The frame's rect in the stage, from the page itself.
  const box = await page.locator('#screen').boundingBox()
  const rect = await page.evaluate(() => { const s = document.getElementById('stage'); const fw = 390, fh = 844; const sc = Math.min(s.clientWidth / fw, s.clientHeight / fh); return { x: (s.clientWidth - fw * sc) / 2, y: (s.clientHeight - fh * sc) / 2, w: fw * sc, h: fh * sc } })
  const at = (fx, fy) => [box.x + rect.x + fx * rect.w, box.y + rect.y + fy * rect.h]

  // A tap in the middle.
  await page.mouse.click(...at(0.5, 0.5))
  await until(async () => (await named(page, 'phone_tap')).length === 1)
  const [, tap] = (await named(page, 'phone_tap'))[0]
  assert.ok(Math.abs(tap.x - 0.5) < 0.01 && Math.abs(tap.y - 0.5) < 0.01, JSON.stringify(tap))

  // A long press.
  await page.mouse.move(...at(0.25, 0.25)); await page.mouse.down(); await page.waitForTimeout(600); await page.mouse.up()
  await until(async () => (await named(page, 'phone_press')).length === 1)
  assert.ok((await named(page, 'phone_press'))[0][1].ms >= 450)

  // A drag up.
  await page.mouse.move(...at(0.5, 0.8)); await page.mouse.down()
  for (let i = 1; i <= 10; i++) { await page.mouse.move(...at(0.5, 0.8 - i * 0.05)); await page.waitForTimeout(15) }
  await page.mouse.up()
  await until(async () => (await named(page, 'phone_drag')).length === 1)
  const [, drag] = (await named(page, 'phone_drag'))[0]
  assert.ok(drag.steps.length >= 3, JSON.stringify(drag))
  assert.ok(Math.abs(drag.steps[0][1] - 0.8) < 0.01 && Math.abs(drag.steps.at(-1)[1] - 0.3) < 0.01, JSON.stringify(drag))

  // The wheel.
  await page.mouse.move(...at(0.5, 0.5)); await page.mouse.wheel(0, -100)
  await until(async () => (await named(page, 'phone_scroll')).length === 1)
  assert.ok((await named(page, 'phone_scroll'))[0][1].delta > 0, 'wheel up moves the finger down')

  // Typing: letters, Backspace, Enter, an IME's word, a paste.
  assert.equal(await page.evaluate(() => document.activeElement.id), 'ime')
  await page.keyboard.type('hi')
  await page.keyboard.press('Backspace')
  await page.keyboard.press('Enter')
  await page.evaluate(() => {
    const ime = document.getElementById('ime')
    ime.dispatchEvent(new CompositionEvent('compositionstart'))
    ime.value = '你好'
    ime.dispatchEvent(new InputEvent('input', { isComposing: true, data: '你好' }))
    ime.dispatchEvent(new CompositionEvent('compositionend', { data: '你好' }))
  })
  await page.waitForTimeout(300)
  const typed = (await named(page, 'phone_type')).map(c => c[1].text).join('')
  assert.equal(typed, 'hi\b\n你好')

  // F1 is Home, F5 never reloads.
  await page.keyboard.press('F1')
  await page.keyboard.press('F5')
  await page.waitForTimeout(200)
  const buttons = (await named(page, 'phone_button')).map(c => c[1].name)
  assert.deepEqual(buttons, ['home', 'lock'])
  assert.equal((await named(page, 'phone_state')).length, 1, 'not reloaded')

  // A side button, the screenshot, the quality.
  await page.click('[data-button=switcher]')
  await page.click('#screenshot')
  await page.selectOption('#quality', 'hd')
  await page.waitForTimeout(200)
  assert.ok((await page.locator('#status').textContent()).includes('截图已保存'))
  assert.deepEqual((await named(page, 'phone_quality'))[0][1], { name: 'hd' })
  await shot(page, 'shot-connected.png')

  // Settings kept.
  await page.click('#openSettings')
  await page.fill('#bundle', 'com.zh.WebDriverAgentRunner.xctrunner')
  await page.click('dialog button[value=save]')
  await page.waitForTimeout(100)
  assert.equal(await page.evaluate(() => JSON.parse(localStorage.getItem('phone.prefs')).bundle), 'com.zh.WebDriverAgentRunner.xctrunner')

  // Disconnect.
  await page.click('#connect')
  await until(async () => (await named(page, 'phone_disconnect')).length === 1)
  assert.equal(await page.locator('#connect').textContent(), '连接')

  // --- An error, worded ---
  await page.evaluate(() => { window.__answers.phone_connect = { error: 'wdaExited', detail: 'testmanagerd: denied' } })
  await page.click('#connect')
  await until(async () => (await page.locator('#status').textContent()).includes('WebDriverAgent'))
  assert.ok((await page.locator('#status').textContent()).includes('testmanagerd: denied'))
  assert.ok(await page.locator('#log').isVisible())
  await shot(page, 'shot-error.png')

  // --- No go-ios yet: the setup card, then the download ---
  const p3ctx = await browser.newContext({ viewport: { width: 920, height: 900 }, locale: 'en-US' })
  const p3 = await p3ctx.newPage()
  p3.on('pageerror', e => errors.push(String(e)))
  await p3.addInitScript(MOCK, { goios: false })
  await p3.goto(PAGE)
  await until(async () => p3.locator('#setup').isVisible())
  assert.ok((await p3.locator('#setupBody').textContent()).includes('go-ios v1.3.2'))
  await shot(p3, 'shot-setup.png')
  await p3.click('#setupButton')
  await until(async () => (await named(p3, 'phone_devices')).length === 1)
  assert.ok(await p3.locator('#setup').isHidden())
  assert.equal(await p3.locator('#setupProgress').evaluate(e => e.value), 100)

  assert.deepEqual(errors, [])
  console.log('page test: all good')
  await browser.close()
})().catch(e => { console.error(e); process.exit(1) })
