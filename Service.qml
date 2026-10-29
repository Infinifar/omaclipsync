import QtQuick
import Quickshell.Io
import qs.Commons

Item {
  id: root
  property var shell: null
  property var status: ({ state: "starting", enabled: false })
  property var config: ({})
  property bool configReady: false
  property var history: []
  property int historyOffset: 0
  property int historyTotal: 0
  property string errorCode: ""
  property bool ready: false
  property bool automaticBusy: false
  property bool stopped: false
  property bool shuttingDown: false
  property bool failureScheduled: false
  property int failures: 0
  property int nextId: 0
  property var pending: ({})
  readonly property bool busy: {
    if (automaticBusy) return true
    for (let key in pending) if (pending[key] === "push" || pending[key] === "pull") return true
    return false
  }
  signal replied(int id, string method, bool ok, var result, string error)
  signal historyRequested()
  signal iconVisibilityRequested(bool visible)

  function daemonPath() {
    return decodeURIComponent(String(Qt.resolvedUrl("bin/syncclipd")).replace(/^file:\/\//, ""))
  }
  function request(method, params) {
    if (!process.running || (method !== "hello" && !ready) || Object.keys(pending).length >= 64) return false
    var id = ++nextId
    var map = Object.assign({}, pending); map[id] = method; pending = map
    process.write(JSON.stringify({id: id, method: method, params: params || {}}) + "\n")
    return id
  }
  function receive(line) {
    var message
    try { message = JSON.parse(line) } catch (e) { return }
    if (message.event === "activity" && message.data) { automaticBusy = message.data.active === true; return }
    if (message.event === "status" && message.data && typeof message.data === "object") {
      let oldRevision = status.configRevision
      status = message.data
      if (ready && message.data.configRevision !== oldRevision) { request("get_config"); request("history") }
      return
    }
    if (typeof message.id !== "number") return
    var method = pending[message.id]
    if (!method) return
    var map = Object.assign({}, pending); delete map[message.id]; pending = map
    var result = message.result || ({})
    if (method === "hello") {
      if (message.ok !== true || result.protocol !== 1) { process.running = false; failed(); return }
      ready = true; handshake.stop(); stable.restart()
      request("get_config"); request("status")
    } else if (message.ok === true) {
      errorCode = ""
      if (method === "get_config") { config = result; configReady = true }
      if (method === "status" || method === "set_config" || method === "toggle" || method === "set_password") status = result
      if (method === "history") { history = result.entries || []; historyOffset = Number(result.offset || 0); historyTotal = Number(result.total || 0) }
      if (method === "clear_history") { history = []; historyOffset = 0; historyTotal = 0; request("history") }
      if (method === "set_config") { request("get_config"); request("history") }
    } else {
      errorCode = String(message.error || "unknown")
    }
    replied(message.id, method, message.ok === true, result, String(message.error || ""))
  }
  function failed() {
    if (shuttingDown || failureScheduled) return
    ready = false; automaticBusy = false; configReady = false; handshake.stop(); stable.stop(); pending = ({})
    status = ({state: "daemon_stopped", enabled: false})
    if (failures >= 3) { stopped = true; return }
    failures += 1; failureScheduled = true
    restart.interval = Math.pow(2, failures - 1) * 1000
    restart.restart()
  }
  function launch() {
    if (shuttingDown || stopped) return
    failureScheduled = false
    process.running = true
    handshake.restart()
  }
  function reloadHistory(offset) { request("history", {offset: Math.max(0, Number(offset || 0))}) }
  function push() { return request("push") }
  function pull(force) { return request("pull", {force: force === true}) }
  function toggleSync() { return request("toggle") }
  Component.onCompleted: launch()
  Component.onDestruction: { shuttingDown = true; restart.stop(); handshake.stop(); if (process.running) process.write('{"id":0,"method":"shutdown"}\n') }

  ShellIpc {
    target: "io.github.infinifar.omaclipsync"
    function open(): string { return root.shell && root.shell.summon("io.github.infinifar.omaclipsync", "{}") ? "opened" : "unavailable" }
    function showIcon(): string { root.iconVisibilityRequested(true); return "queued" }
    function hideIcon(): string { root.iconVisibilityRequested(false); return "queued" }
    function pull(): string { return root.pull(false) ? "queued" : "unavailable" }
    function pullForce(): string { return root.pull(true) ? "queued" : "unavailable" }
    function push(): string { return root.push() ? "queued" : "unavailable" }
    function toggle(): string { return root.toggleSync() ? "queued" : "unavailable" }
    function status(): string { return JSON.stringify(root.status) }
    function openHistory(): string {
      root.historyRequested()
      return root.shell && root.shell.summon("io.github.infinifar.omaclipsync", "{}") ? "opened" : "unavailable"
    }
  }
  Process {
    id: process
    command: [root.daemonPath()]
    stdinEnabled: true
    onStarted: { root.request("hello") }
    onExited: { root.failed() }
    stdout: SplitParser { onRead: function(data) { root.receive(data) } }
    stderr: SplitParser { onRead: function(data) {} }
  }
  Timer { id: handshake; interval: 15000; onTriggered: { if (!root.ready) { process.running = false; root.failed() } } }
  Timer { id: restart; onTriggered: root.launch() }
  Timer { id: stable; interval: 60000; onTriggered: root.failures = 0 }
}
