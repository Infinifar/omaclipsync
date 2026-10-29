const Gio = imports.gi.Gio
const ByteArray = imports.byteArray
const source = ByteArray.toString(Gio.File.new_for_path(ARGV[0]).load_contents(null)[1])
const api = eval(source + "\n;({snapshot, changed, merge, values})")
function assert(condition, message) { if (!condition) throw new Error(message) }
const original = {sync: {intervalSec: 3}, filter: {mode: "whitelist", patterns: ["a"]}, history: {enabled: false}}
const before = api.snapshot({refreshIntervalSec: 3, filterEnabled: true, historyEnabled: false})
const after = api.snapshot({refreshIntervalSec: 10, filterEnabled: true, historyEnabled: false})
const delta = api.changed(before, after)
assert(JSON.stringify(delta) === '{"refreshIntervalSec":10}', "unexpected change set")
const merged = api.merge(original, delta)
assert(merged.sync.intervalSec === 10 && original.sync.intervalSec === 3, "merge must not modify original")
assert(merged.filter.mode === "whitelist", "unmodified filter must be retained")
assert(api.merge(original, {filterEnabled:false}).filter.mode === "off", "filter must turn off")
assert(api.merge({sync:{},filter:{mode:"off"},history:{}},{filterEnabled:true}).filter.mode === "blacklist", "filter must turn on")
