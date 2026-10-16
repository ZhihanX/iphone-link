const assert = require('node:assert/strict')
const fs = require('node:fs')
const path = require('node:path')
const { test } = require('node:test')

const I18n = require('../src/shared/i18n')

const read = file => fs.readFileSync(path.join(__dirname, '..', file), 'utf8')

test('both languages have the same words', () => {
  const zh = Object.keys(I18n.STRINGS.zh).sort()
  const en = Object.keys(I18n.STRINGS.en).sort()
  assert.deepEqual(zh, en)
  for (const key of zh) assert.ok(key.startsWith('phone.'), `${key}: every key under phone., to merge into Wakuwaku`)
})

test('every error the Rust side sends has its words', () => {
  const rust = ['src-tauri/src/phone.rs', 'src-tauri/src/phone/goios_setup.rs'].map(read).join('\n')
  const codes = new Set()
  for (const m of rust.matchAll(/\berr\("(\w+)"/g)) codes.add(m[1])
  for (const m of rust.matchAll(/\bfail\("(\w+)"/g)) codes.add(m[1])
  for (const m of rust.matchAll(/(?:ok_or|Err)\("(no\w+)"\)/g)) codes.add(m[1])
  assert.ok(codes.size > 15, `found ${codes.size} codes`)
  for (const code of codes) assert.ok(I18n.STRINGS.zh[`phone.err.${code}`], `phone.err.${code}`)
})

test('every step the Rust side says has its words', () => {
  const steps = [...read('src-tauri/src/phone.rs').matchAll(/\bstep\(sh, "(\w+)"\)/g)].map(m => m[1])
  assert.ok(steps.length >= 6)
  for (const s of steps) assert.ok(I18n.STRINGS.zh[`phone.step.${s}`], `phone.step.${s}`)
})

test('every word the page asks for is there', () => {
  const html = read('src/phone/index.html')
  const js = read('src/phone/phone.js')
  const keys = [...html.matchAll(/data-i18n(?:-placeholder)?="([\w.]+)"/g), ...js.matchAll(/t\('(phone\.[\w.]+)'/g)].map(m => m[1])
  assert.ok(keys.length > 30)
  for (const key of keys) assert.ok(I18n.STRINGS.zh[key], key)
})

test('words are filled in and fall back to English', () => {
  assert.equal(I18n.t('zh', 'phone.setup.progress', { pct: 42 }), '正在下载… 42%')
  assert.equal(I18n.t('xx', 'phone.connect'), 'Connect')
  assert.equal(I18n.detectLang('zh-CN'), 'zh')
  assert.equal(I18n.detectLang('en-US'), 'en')
})
