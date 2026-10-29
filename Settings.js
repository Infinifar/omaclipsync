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
  if (Object.prototype.hasOwnProperty.call(changes, "refreshIntervalSec")) next.sync.intervalSec = changes.refreshIntervalSec
  if (Object.prototype.hasOwnProperty.call(changes, "historyEnabled")) next.history.enabled = changes.historyEnabled
  if (Object.prototype.hasOwnProperty.call(changes, "filterEnabled")) {
    if (changes.filterEnabled === false) next.filter.mode = "off"
    else if (changes.filterEnabled === true && next.filter.mode === "off") next.filter.mode = "blacklist"
  }
  return next
}
function values(config) {
  return {refreshIntervalSec: config.sync.intervalSec, filterEnabled: config.filter.mode !== "off", historyEnabled: config.history.enabled}
}
if (typeof module !== "undefined") module.exports = {snapshot: snapshot, changed: changed, merge: merge, values: values}
