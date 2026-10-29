// Conversion between optional bar settings and the daemon's canonical config.
function snapshot(settings) {
  var result = {}
  var keys = ["refreshIntervalSec", "filterEnabled", "historyEnabled"]
  for (var i = 0; i < keys.length; ++i) {
    var key = keys[i]
    if (settings && Object.prototype.hasOwnProperty.call(settings, key)) result[key] = settings[key]
  }
  return result
}
function changed(before, after) {
  var result = {}
  var keys = ["refreshIntervalSec", "filterEnabled", "historyEnabled"]
  for (var i = 0; i < keys.length; ++i) {
    var key = keys[i]
    if (Object.prototype.hasOwnProperty.call(after, key) && before[key] !== after[key]) result[key] = after[key]
  }
  return result
}
function merge(config, changes) {
  if (!config || !config.sync || !config.filter || !config.history) return null
  var next = JSON.parse(JSON.stringify(config))
  if (Object.prototype.hasOwnProperty.call(changes, "refreshIntervalSec")) next.sync.intervalSec = Math.max(30, Math.min(300, Number(changes.refreshIntervalSec) || 60))
  if (Object.prototype.hasOwnProperty.call(changes, "historyEnabled")) next.history.enabled = changes.historyEnabled
  if (Object.prototype.hasOwnProperty.call(changes, "filterEnabled")) {
    if (changes.filterEnabled === false) next.filter.mode = "off"
    else if (changes.filterEnabled === true && next.filter.mode === "off") next.filter.mode = "blacklist"
  }
  return next
}
function values(config) {
  return {refreshIntervalSec: Math.max(30, Math.min(300, Number(config.sync.intervalSec) || 60)), filterEnabled: config.filter.mode !== "off", historyEnabled: config.history.enabled}
}
// Preserve all existing inline fields when changing one UI-only setting.
// showIcon belongs to the shell layout and is never sent to the daemon.
function inlineEntry(id, settings, config, showIcon) {
  var entry = {id: id}
  for (var key in settings) if (key !== "id" && Object.prototype.hasOwnProperty.call(settings, key)) entry[key] = settings[key]
  if (config) {
    var currentValues = values(config)
    for (var field in currentValues) entry[field] = currentValues[field]
  }
  if (showIcon !== undefined) entry.showIcon = showIcon === true
  return entry
}
function inlineSame(settings, entry) {
  var current = inlineEntry(entry.id, settings, null, undefined)
  var keys = Object.keys(current).sort(), nextKeys = Object.keys(entry).sort()
  if (JSON.stringify(keys) !== JSON.stringify(nextKeys)) return false
  for (var i = 0; i < keys.length; ++i)
    if (JSON.stringify(current[keys[i]]) !== JSON.stringify(entry[keys[i]])) return false
  return true
}
if (typeof module !== "undefined") module.exports = {snapshot: snapshot, changed: changed, merge: merge, values: values, inlineEntry: inlineEntry, inlineSame: inlineSame}
