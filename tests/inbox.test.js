// Runs under Node for local development and GJS on the existing CI runner.
let model, byteLength
const assert = {equal(actual, expected) { if (actual !== expected) throw new Error("Inbox assertion failed") }}
if (typeof require === "function") {
  const fs = require('node:fs'), vm = require('node:vm')
  model = vm.createContext({})
  vm.runInContext(fs.readFileSync(require('node:path').join(__dirname, '../Inbox.js'), 'utf8'), model)
  byteLength = text => Buffer.byteLength(text)
} else {
  const Gio = imports.gi.Gio, ByteArray = imports.byteArray
  const source = ByteArray.toString(Gio.File.new_for_path(ARGV[0]).load_contents(null)[1])
  model = eval(source + "\n;({empty, begin, append})")
  byteLength = text => ByteArray.fromString(text).length
}
const chunk = (id, text, offset = 0, total = byteLength(text)) =>
  ({id, text, offset, nextOffset: offset + byteLength(text), total})
const secret = '<img src="https://invalid.test">😀中文'
let state = model.begin('current')
assert.equal(state.text, '')
assert.equal(state.revealed, false)
state = model.append(state, chunk('current', secret))
assert.equal(state.text, secret) // QML must render this unchanged as PlainText.
assert.equal(state.revealed, true)
assert.equal(model.append(model.empty(), chunk('current', secret)), null)
assert.equal(model.append(model.begin('new'), chunk('current', secret)), null)
assert.equal(model.append(model.begin('current'), chunk('current', secret, 3)), null)
assert.equal(model.append(model.begin('current'), {id:'current',text:'\ud800',offset:0,nextOffset:3,total:3}), null)
state = model.begin('long')
const text = '😀'.repeat(4096)
for (let i = 0; i < 4; i++) state = model.append(state, chunk('long', text, i * 16384, 100000))
assert.equal(byteLength(state.text), 65536)
assert.equal(state.loading, false)
assert.equal(state.truncated, true)
assert.equal(model.append(state, chunk('long', 'more', 65536, 100000)), null)
assert.equal(model.empty().text, '')
const report = 'Inbox previews: delayed/stale replies, explicit reveal, UTF-8 and size limits passed.'
if (typeof print === 'function') print(report); else console.log(report)
