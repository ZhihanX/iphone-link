// The phone's window: its screen drawn from the frames the Rust side sends
// (phone_stream, raw JPEGs over a channel), the mouse on it turned into
// touches (gesture.js), the keyboard and the IME into typing, and the side
// panel's buttons. Everything done to the phone goes one at a time, in order
// (act): a drag must not overtake the tap before it.
;(function () {
  const { invoke, Channel } = window.__TAURI__.core
  const { listen } = window.__TAURI__.event
  const G = window.Gesture
  const lang = I18n.detectLang()
  const t = (key, vars) => I18n.t(lang, key, vars)
  const $ = id => document.getElementById(id)
  // Events for this window alone: listen() by itself hears every window's.
  const target = { kind: 'WebviewWindow', label: 'phone' }

  const stage = $('stage')
  const canvas = $('screen')
  const ctx = canvas.getContext('2d')
  const ime = $('ime')

  // What the person set: kept in this window's storage (the page's alone).
  const PREFS = 'phone.prefs'
  const prefs = Object.assign({ bundle: '', direct: '', quality: 'standard' }, read())
  function read() {
    try {
      return JSON.parse(localStorage.getItem(PREFS)) || {}
    } catch {
      return {}
    }
  }
  function save() {
    try {
      localStorage.setItem(PREFS, JSON.stringify(prefs))
    } catch {}
  }

  const state = {
    devices: [],
    connected: false,
    connecting: false,
    frame: null,
    frameSeq: 0,
    drawnSeq: 0,
    landscape: null,
    rect: null,
    pressing: null,
    wheel: { delta: 0, x: 0, y: 0, timer: 0 },
    typed: '',
    typeTimer: 0,
  }

  // --- Words ----------------------------------------------------------------------

  document.documentElement.lang = lang
  document.title = t('phone.title')
  for (const el of document.querySelectorAll('[data-i18n]')) {
    el.textContent = t(el.dataset.i18n)
    if (el.dataset.key) el.title = `${el.textContent} (${el.dataset.key})`
  }
  for (const el of document.querySelectorAll('[data-i18n-placeholder]')) el.placeholder = t(el.dataset.i18nPlaceholder)

  function say(text, error = false) {
    const s = $('status')
    s.textContent = text
    s.title = text
    s.classList.toggle('error', error)
  }

  function sayError(res) {
    const text = t(`phone.err.${res.error}`) + (res.detail ? ` (${res.detail})` : '')
    say(text, true)
    logLine(`[error] ${res.error}: ${res.detail || ''}`)
    return text
  }

  function placeholder(text) {
    const p = $('placeholder')
    p.textContent = text || ''
    p.hidden = !text
  }

  const logLines = []
  function logLine(line) {
    const time = new Date().toLocaleTimeString()
    logLines.push(`${time} ${line}`)
    if (logLines.length > 500) logLines.splice(0, logLines.length - 500)
    const el = $('log')
    el.textContent = logLines.join('\n')
    el.scrollTop = el.scrollHeight
  }

  // --- To the phone, in order ------------------------------------------------------

  let queue = Promise.resolve()
  function act(cmd, args, ok) {
    const run = queue.then(() => invoke(cmd, args || {}))
    queue = run
      .then(res => {
        if (res && res.error) sayError(res)
        else if (ok) say(typeof ok === 'function' ? ok(res) : ok)
      })
      .catch(err => say(String(err), true))
    return run
  }

  function setConnected(on) {
    state.connected = on
    for (const el of document.querySelectorAll('.ctl')) el.disabled = !on
    const btn = $('connect')
    btn.textContent = t(on ? 'phone.disconnect' : state.connecting ? 'phone.connecting' : 'phone.connect')
    btn.disabled = state.connecting
    $('device').disabled = on || state.connecting
    $('refresh').disabled = on || state.connecting
  }

  // --- go-ios and the devices ---------------------------------------------------------

  function showSetup(show) {
    $('setup').hidden = !show
    if (show) placeholder('')
  }

  async function runSetup() {
    $('setupButton').disabled = true
    $('setupProgress').hidden = false
    const res = await invoke('phone_setup')
    $('setupButton').disabled = false
    if (res.error) {
      sayError(res)
      return
    }
    say(t('phone.setup.done'))
    showSetup(false)
    refresh()
  }

  async function refresh() {
    const select = $('device')
    select.innerHTML = ''
    select.add(new Option(t('phone.searching'), ''))
    const res = await invoke('phone_devices')
    select.innerHTML = ''
    if (res.error) {
      if (res.error === 'noGoIos') showSetup(true)
      select.add(new Option(t('phone.noDevices'), ''))
      sayError(res)
      return
    }
    state.devices = res.devices
    for (const d of res.devices) select.add(new Option(t('phone.deviceLabel', { name: d.name, ios: d.ios }), d.udid))
    if (!res.devices.length) select.add(new Option(t('phone.noDevices'), ''))
    placeholder(t('phone.placeholder'))
    say(res.devices.length ? '' : t('phone.noDevices'))
  }

  // --- Connecting -------------------------------------------------------------------------

  async function toggleConnection() {
    if (state.connected) {
      await invoke('phone_disconnect')
      dropFrame()
      setConnected(false)
      $('info').textContent = ''
      placeholder(t('phone.disconnected'))
      say(t('phone.disconnected'))
      return
    }
    const udid = $('device').value
    if (!udid && !prefs.direct) {
      say(t('phone.noDevices'), true)
      return
    }
    state.connecting = true
    setConnected(false)
    const res = await invoke('phone_connect', {
      udid: udid || null,
      direct: prefs.direct || null,
      bundle: prefs.bundle || null,
      quality: prefs.quality,
    })
    state.connecting = false
    if (res.error) {
      setConnected(false)
      placeholder(sayError(res))
      $('log').hidden = false
      return
    }
    const name = res.device ? res.device.name : prefs.direct
    const info = t('phone.connected', { name, w: Math.round(res.size.width), h: Math.round(res.size.height) })
    $('info').textContent = info
    say(info)
    setConnected(true)
    placeholder(t('phone.waitingFrame'))
    startStream()
    focusIme()
  }

  // --- The screen ---------------------------------------------------------------------------

  function startStream() {
    const frames = new Channel()
    frames.onmessage = onFrame
    invoke('phone_stream', { frames }).then(res => res && res.error && sayError(res))
  }

  function dropFrame() {
    if (state.frame) state.frame.close()
    state.frame = null
    state.rect = null
    state.landscape = null
    draw()
  }

  async function onFrame(data) {
    const seq = ++state.frameSeq
    const bytes = data instanceof Uint8Array ? data : new Uint8Array(data instanceof ArrayBuffer ? data : data.buffer || data)
    try {
      const bmp = await createImageBitmap(new Blob([bytes], { type: G.imageType(bytes) }))
      // Decoded out of order: an older one is not drawn over a newer.
      if (seq < state.drawnSeq || !state.connected) {
        bmp.close()
        return
      }
      state.drawnSeq = seq
      if (state.frame) state.frame.close()
      state.frame = bmp
      const land = G.landscape(bmp.width, bmp.height)
      if (state.landscape !== null && land !== state.landscape) invoke('phone_size')
      state.landscape = land
      placeholder('')
      draw()
    } catch (err) {
      logLine(`frame: ${err}`)
    } finally {
      invoke('phone_seen').catch(() => {})
    }
  }

  function draw() {
    const dpr = window.devicePixelRatio || 1
    const w = stage.clientWidth
    const h = stage.clientHeight
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(h * dpr)) {
      canvas.width = Math.round(w * dpr)
      canvas.height = Math.round(h * dpr)
    }
    ctx.setTransform(1, 0, 0, 1, 0, 0)
    ctx.clearRect(0, 0, canvas.width, canvas.height)
    const f = state.frame
    if (!f) return
    state.rect = G.fit(f.width, f.height, w, h)
    const r = state.rect
    ctx.imageSmoothingQuality = 'high'
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0)
    ctx.drawImage(f, r.x, r.y, r.w, r.h)
  }

  new ResizeObserver(draw).observe(stage)

  // --- The mouse ------------------------------------------------------------------------------

  const point = (e, clamp) => {
    const f = G.toFraction(state.rect, e.offsetX, e.offsetY, clamp)
    return f && { x: f[0], y: f[1], px: e.offsetX, py: e.offsetY, t: e.timeStamp }
  }

  canvas.addEventListener('pointerdown', e => {
    if (!state.connected || e.button !== 0) return
    e.preventDefault()
    focusIme()
    const p = point(e, false)
    if (!p) return
    canvas.setPointerCapture(e.pointerId)
    state.pressing = [p]
  })

  canvas.addEventListener('pointermove', e => {
    if (!state.pressing) return
    const p = point(e, true)
    if (p) state.pressing.push(p)
  })

  canvas.addEventListener('pointerup', e => {
    if (!state.pressing) return
    const p = point(e, true)
    if (p) state.pressing.push(p)
    const g = G.gestureOf(state.pressing)
    state.pressing = null
    if (!g) return
    if (g.kind === 'tap') act('phone_tap', { x: g.x, y: g.y })
    else if (g.kind === 'press') act('phone_press', { x: g.x, y: g.y, ms: g.ms })
    else act('phone_drag', { steps: g.steps, hold: g.hold })
  })

  canvas.addEventListener('pointercancel', () => {
    state.pressing = null
  })

  // The wheel, a notch a step: added up for a moment, then one swipe. Up
  // (deltaY < 0) moves the finger down, as the list scrolls up.
  canvas.addEventListener(
    'wheel',
    e => {
      if (!state.connected) return
      const p = point(e, false)
      if (!p) return
      e.preventDefault()
      const w = state.wheel
      w.delta += -e.deltaY * (e.deltaMode === 1 ? 40 : 1.2)
      w.x = p.x
      w.y = p.y
      if (!w.timer) {
        w.timer = setTimeout(() => {
          const delta = w.delta
          w.delta = 0
          w.timer = 0
          if (Math.abs(delta) >= 1) act('phone_scroll', { x: w.x, y: w.y, delta })
        }, 120)
      }
    },
    { passive: false },
  )

  // --- Typing -----------------------------------------------------------------------------------
  // The hidden textarea has the keyboard while the screen was the last thing
  // clicked. Letters come as its input (and an IME's words as its
  // compositionend), Backspace, Enter and Tab as keys, a paste as a paste.

  function focusIme() {
    ime.focus({ preventScroll: true })
  }
  ime.addEventListener('focus', () => stage.classList.add('focused'))
  ime.addEventListener('blur', () => stage.classList.remove('focused'))

  function type(text) {
    if (!text || !state.connected) return
    state.typed += text
    clearTimeout(state.typeTimer)
    state.typeTimer = setTimeout(() => {
      const text = state.typed
      state.typed = ''
      if (text) act('phone_type', { text })
    }, 80)
  }

  function takeTyped() {
    const text = ime.value
    ime.value = ''
    type(text)
  }

  ime.addEventListener('keydown', e => {
    const key = G.keyOf(e)
    if (key) {
      e.preventDefault()
      type(key)
    }
  })
  ime.addEventListener('input', e => {
    if (!e.isComposing) takeTyped()
  })
  ime.addEventListener('compositionend', takeTyped)
  ime.addEventListener('paste', e => {
    e.preventDefault()
    type(e.clipboardData.getData('text/plain'))
  })

  // --- Buttons ------------------------------------------------------------------------------------

  for (const btn of document.querySelectorAll('[data-button]')) {
    btn.addEventListener('click', () => act('phone_button', { name: btn.dataset.button }))
  }

  // F1-F6 for the buttons that say so; F5 and Ctrl+R never reload the page.
  document.addEventListener('keydown', e => {
    if (e.key === 'F5' || (e.ctrlKey && e.key.toLowerCase() === 'r')) e.preventDefault()
    const btn = document.querySelector(`[data-key="${e.key}"]`)
    if (btn) {
      e.preventDefault()
      if (!btn.disabled) btn.click()
    }
  })

  $('screenshot').addEventListener('click', () => act('phone_screenshot', {}, res => t('phone.saved', { path: res.path })))

  $('textInput').addEventListener('keydown', e => {
    if (e.key !== 'Enter' || e.isComposing) return
    const text = e.target.value
    if (!text) return
    e.target.value = ''
    act('phone_type', { text }, t('phone.sent'))
  })

  async function pcClipboard() {
    try {
      const text = await navigator.clipboard.readText()
      if (!text) say(t('phone.clipEmpty'))
      return text
    } catch {
      say(t('phone.clipDenied'), true)
      return ''
    }
  }

  $('clipType').addEventListener('click', async () => {
    const text = await pcClipboard()
    if (text) act('phone_type', { text }, t('phone.sent'))
  })

  $('clipPush').addEventListener('click', async () => {
    const text = await pcClipboard()
    if (text) act('phone_clip_push', { text }, t('phone.clipPushed'))
  })

  $('clipPull').addEventListener('click', () => {
    act('phone_clip_pull').then(async res => {
      if (!res || res.error) return
      if (!res.text) return say(t('phone.clipPhoneEmpty'))
      try {
        await navigator.clipboard.writeText(res.text)
        say(t('phone.clipPulled', { n: res.text.length }))
      } catch (err) {
        say(String(err), true)
      }
    })
  })

  // Not in the queue: it takes minutes, and the screen goes on meanwhile.
  $('photos').addEventListener('click', async () => {
    const btn = $('photos')
    btn.disabled = true
    say(t('phone.photosRunning'))
    $('log').hidden = false
    const res = await invoke('phone_photos')
    btn.disabled = !state.connected
    if (res.error) sayError(res)
    else say(t('phone.photosDone', { path: res.path }))
  })

  const quality = $('quality')
  quality.value = prefs.quality
  quality.addEventListener('change', () => {
    prefs.quality = quality.value
    save()
    if (state.connected) act('phone_quality', { name: quality.value })
  })

  $('toggleLog').addEventListener('click', () => {
    $('log').hidden = !$('log').hidden
  })

  // --- Settings ---------------------------------------------------------------------------------

  const dialog = $('settings')
  $('openSettings').addEventListener('click', () => {
    $('bundle').value = prefs.bundle
    $('direct').value = prefs.direct
    dialog.showModal()
  })
  dialog.addEventListener('close', () => {
    if (dialog.returnValue !== 'save') return
    prefs.bundle = $('bundle').value.trim()
    prefs.direct = $('direct').value.trim()
    save()
  })

  $('connect').addEventListener('click', toggleConnection)
  $('refresh').addEventListener('click', refresh)
  $('setupButton').addEventListener('click', runSetup)

  // --- From the Rust side -----------------------------------------------------------------------

  listen(
    'phone:step',
    e => {
      const text = t(`phone.step.${e.payload}`)
      placeholder(text)
      say(text)
    },
    { target },
  )
  listen('phone:log', e => logLine(e.payload), { target })
  listen(
    'phone:setup',
    e => {
      $('setupProgress').value = e.payload
      say(t('phone.setup.progress', { pct: e.payload }))
    },
    { target },
  )
  listen('phone:stream', e => say(t(e.payload === 'screenshots' ? 'phone.streamShots' : 'phone.streamBack')), { target })

  // --- Start ------------------------------------------------------------------------------------

  $('setupBody').textContent = t('phone.setup.body', { version: 'go-ios' })
  setConnected(false)
  placeholder(t('phone.placeholder'))
  invoke('phone_state').then(s => {
    $('setupBody').textContent = t('phone.setup.body', { version: `go-ios ${s.goiosVersion}` })
    if (!s.goios) showSetup(true)
    else refresh()
  })
})()
