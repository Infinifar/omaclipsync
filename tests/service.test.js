// Exercise the actual QML reply handler under Node or the existing GJS runner.
let source
if (typeof require === "function") {
  source = require("node:fs").readFileSync(process.argv[2] || require("node:path").join(__dirname, "../Service.qml"), "utf8")
} else {
  source = imports.byteArray.toString(imports.gi.Gio.File.new_for_path(ARGV[0]).load_contents(null)[1])
}
const receiveSource = source.slice(source.indexOf("  function receive("), source.indexOf("  function failed("))
function check(condition, message) { if (!condition) throw new Error(message) }
for (const method of ["set_config", "toggle", "set_direction", "cycle_direction", "set_password", "get_config"]) {
  let pending = {7: method}, errorCode = "", reply
  const requests = [], osds = []
  function request(name) { requests.push(name) }
  function showDirectionOsd(...args) { osds.push(args) }
  function replied(...args) { reply = args }
  const receive = eval("(" + receiveSource.trim().replace(/^function receive/, "function") + ")")
  receive(JSON.stringify({id: 7, ok: false, error: "config_apply_pending"}))
  const expected = method === "get_config" ? [] : ["status", "get_config"]
  check(JSON.stringify(requests) === JSON.stringify(expected), method + ": saved config/status must be refreshed on an apply error")
  check(errorCode === "config_apply_pending", method + ": failure must remain explicit")
  check(reply[2] === false && reply[4] === "config_apply_pending", method + ": must not emit a success reply")
  check(Object.keys(pending).length === 0, method + ": request must complete")
  check(osds.length === (method === "cycle_direction" ? 1 : 0), method + ": OSD must not claim a successful switch")
}
