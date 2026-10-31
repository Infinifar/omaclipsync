const Gio = imports.gi.Gio
const ByteArray = imports.byteArray
const source = ByteArray.toString(Gio.File.new_for_path(ARGV[0]).load_contents(null)[1])
const api = eval(source + "\n;({snapshot, changed, merge, values, inlineEntry, inlineSame})")
function assert(condition, message) { if (!condition) throw new Error(message) }
const original = {sync: {intervalSec: 3}, filter: {mode: "whitelist", patterns: ["a"]}, history: {enabled: false}}
const before = api.snapshot({refreshIntervalSec: 3, filterEnabled: true, historyEnabled: false})
const after = api.snapshot({refreshIntervalSec: 60, filterEnabled: true, historyEnabled: false})
const delta = api.changed(before, after)
assert(JSON.stringify(delta) === '{"refreshIntervalSec":60}', "unexpected change set")
const merged = api.merge(original, delta)
assert(merged.sync.intervalSec === 60 && original.sync.intervalSec === 3, "merge must not modify original")
assert(api.merge({sync:{intervalSec:60,allowSensitive:true},filter:{mode:"off"},history:{}},{refreshIntervalSec:90}).sync.allowSensitive === true, "unrelated widget settings must preserve sensitive policy")
assert(api.values(original).refreshIntervalSec === 30, "legacy interval must show the effective minimum")
assert(merged.filter.mode === "whitelist", "unmodified filter must be retained")
assert(api.merge(original, {filterEnabled:false}).filter.mode === "off", "filter must turn off")
assert(api.merge({sync:{},filter:{mode:"off"},history:{}},{filterEnabled:true}).filter.mode === "blacklist", "filter must turn on")
const entry = {id:"old", showIcon:true, refreshIntervalSec:60, futureOption:"keep"}
const hidden = api.inlineEntry("plugin", entry, null, false)
assert(hidden.id === "plugin" && hidden.showIcon === false && hidden.refreshIntervalSec === 60 && hidden.futureOption === "keep", "hide must preserve inline fields")
assert(entry.showIcon === true, "hide must not mutate the source entry")
assert(api.inlineSame(entry, api.inlineEntry("plugin", entry, null, true)), "unchanged entry must compare equal")
assert(!api.inlineSame(entry, hidden), "changed entry must compare different")
assert(!api.inlineSame({}, {id:"plugin", showIcon:true}), "missing setting must not compare equal")
const reflected = api.inlineEntry("plugin", {showIcon:false, futureOption:"keep"}, original)
assert(reflected.showIcon === false && reflected.futureOption === "keep" && reflected.refreshIntervalSec === 30, "config reflection must retain hidden state")
assert(!Object.prototype.hasOwnProperty.call(api.merge(original, {}), "showIcon"), "icon visibility must not enter daemon config")
