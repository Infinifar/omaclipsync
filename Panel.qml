pragma ComponentBehavior: Bound
import QtQuick
import QtQuick.Controls as Controls
import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import qs.Commons
import qs.Commons as Commons
import qs.Ui as Ui

Ui.Panel {
  id: root
  moduleName: "omaclipsync"
  ipcTarget: "omaclipsync"
  manageIpc: false
  property var anchorItem: null
  property var hostWidget: null
  property var service: null
  property bool editing: false
  property bool historyMode: false
  property int copyRequestId: 0
  property int favoriteRequestId: 0
  property string favoriteError: ""
  property bool pasteAfterCopy: false
  property var pasteTarget: null
  property var pendingConfig: null
  property int settingsRequestId: 0
  property int passwordRequestId: 0
  property int testRequestId: 0
  property string pendingPassword: ""
  property string pendingAccount: ""
  property string formError: ""
  property string testResult: ""
  property int actionIndex: 0
  readonly property var identity: hostWidget || root
  readonly property var barApi: bar
  readonly property var fontTokens: Style.font
  readonly property var popupColors: Commons.Color.popups
  readonly property color foreground: barApi ? barApi.foreground : Commons.Color.foreground
  readonly property string fontFamily: barApi ? barApi.fontFamily : fontTokens.family
  readonly property string syncState: service && service.status ? String(service.status.state || "starting") : "starting"
  readonly property bool syncEnabled: service && service.status ? service.status.enabled === true : false
  Commons.Translations { id: strings; directory: Qt.resolvedUrl("i18n") }
  function open() {
    if (service) { service.request("get_config"); service.reloadHistory() }
    controller.show()
  }
  function close() { editing = false; historyMode = false; actionIndex = 0; pendingPassword = ""; formError = ""; favoriteError = ""; controller.hide() }
  function toggle() { if (opened) close(); else open() }
  function showHistory() { pasteTarget = ToplevelManager.activeToplevel; historyMode = true; editing = false; actionIndex = 0; if (service) service.reloadHistory(0) }
  function pasteUnavailable() { Quickshell.execDetached(["notify-send", strings.t("title"), strings.t("paste_unavailable")]) }
  function selectHistory(hash, paste) {
    if (!service || !service.ready || copyRequestId) return
    pasteAfterCopy = paste
    copyRequestId = service.request("copy_history", {hash: String(hash)}) || 0
    if (!copyRequestId) formError = "history_failed"
  }
  function toggleFavorite(entry) {
    if (!service || !service.ready || favoriteRequestId) return
    favoriteError = ""
    favoriteRequestId = service.request("set_favorite", {hash: String(entry.hash), favorite: entry.favorite !== true}) || 0
    if (!favoriteRequestId) favoriteError = "history_failed"
  }
  function switchPanel(direction) { return barApi && typeof barApi.switchPanelFrom === "function" ? barApi.switchPanelFrom(identity, direction) : false }
  function stateText() {
    if (service && service.busy) return strings.t("syncing")
    if (service && service.status && service.status.clipboardError === true) return strings.t("clipboard_error")
    var valid = ["connected", "connecting", "offline", "credentials", "needs_config", "config_error", "paused", "clipboard_error", "daemon_stopped", "starting"]
    return strings.t(valid.indexOf(syncState) >= 0 ? syncState : "starting")
  }
  function errorText(code) {
    if (code === "invalid_config" || code === "config_write_failed" || code === "invalid_filter") return strings.t("error_config")
    if (code === "keyring_failed") return strings.t("error_keyring")
    if (code === "clipboard_unavailable") return strings.t("clipboard_error")
    if (code === "sync_failed" || code === "not_configured") return strings.t("error_sync")
    if (code === "history_failed" || code === "history_not_found" || code === "history_disabled") return strings.t("error_history")
    if (code === "favorite_limit") return strings.t("favorite_limit")
    return strings.t("error_generic")
  }
  function startEditing() {
    var content = popup.ensureContent()
    if (!content || !service || !service.ready || !service.configReady) return
    formError = ""; testResult = ""; pendingPassword = ""; content.loadSettings()
    editing = true
    Qt.callLater(function() { content.focusUrl() })
  }
  function cancelEditing() { editing = false; pendingPassword = ""; formError = ""; testResult = ""; if (popup.loadedContent) popup.loadedContent.forceActiveFocus() }
  function activateAction() {
    if (!service || !service.ready) return
    if (historyMode) {
      if (actionIndex === 0) { historyMode = false; actionIndex = 0 }
      else if (actionIndex === 1 && service.historyOffset > 0) { service.reloadHistory(Math.max(0, service.historyOffset - 20)); actionIndex = 0 }
      else if (actionIndex === 2 && service.historyOffset + 20 < service.historyTotal) { service.reloadHistory(service.historyOffset + 20); actionIndex = 0 }
      else if (actionIndex >= 3) {
        let index = Math.floor((actionIndex - 3) / 3)
        let action = (actionIndex - 3) % 3
        if (index < service.history.length) {
          if (action === 2) toggleFavorite(service.history[index])
          else selectHistory(service.history[index].hash, action === 1)
        }
      }
      return
    }
    if (actionIndex === 0) service.push()
    else if (actionIndex === 1) service.pull(false)
    else if (actionIndex === 2) service.toggleSync()
    else if (actionIndex === 3) startEditing()
    else if (actionIndex === 4) showHistory()
    else if (service) service.request("clear_history")
  }
  Connections {
    target: root.service
    ignoreUnknownSignals: true
    function onReplied(id, method, ok, result, error) {
      if (method === "copy_history" && id === root.copyRequestId) {
        root.copyRequestId = 0
        if (!ok) { root.formError = error || "history_failed"; root.pasteAfterCopy = false; return }
        let paste = root.pasteAfterCopy; root.pasteAfterCopy = false; root.close()
        if (paste) pasteTimer.restart()
      } else if (method === "set_favorite" && id === root.favoriteRequestId) {
        root.favoriteRequestId = 0
        if (!ok) root.favoriteError = error || "history_failed"
        else { root.favoriteError = ""; root.service.reloadHistory(root.service.historyOffset) }
      } else if (method === "set_config" && id === root.settingsRequestId) {
        if (!ok) { root.pendingPassword = ""; root.pendingConfig = null; root.formError = error || "unknown"; return }
        if (root.pendingConfig && root.hostWidget && typeof root.hostWidget.reflectSettings === "function") root.hostWidget.reflectSettings(root.pendingConfig)
        root.pendingConfig = null
        if (root.pendingPassword !== "") {
          let password = root.pendingPassword; root.pendingPassword = ""
          root.passwordRequestId = root.service.request("set_password", {account: root.pendingAccount, password: password}) || 0
          if (!root.passwordRequestId) root.formError = "keyring_failed"
          password = ""
        } else root.cancelEditing()
      } else if (method === "set_password" && id === root.passwordRequestId) {
        if (ok) root.cancelEditing(); else root.formError = error || "unknown"
      } else if (method === "test_filter" && id === root.testRequestId) {
        root.testResult = ok ? (result.matched ? "test_match" : "test_no_match") : (error || "unknown")
      }
    }
  }
  Timer {
    id: pasteTimer; interval: 200
    onTriggered: {
      if (root.pasteTarget && ToplevelManager.activeToplevel === root.pasteTarget) pasteProcess.running = true
      else root.pasteUnavailable()
      root.pasteTarget = null
    }
  }
  Process {
    id: pasteProcess
    command: ["wtype", "-M", "shift", "-k", "Insert", "-m", "shift"]
    onExited: function(exitCode, exitStatus) { if (exitCode !== 0) root.pasteUnavailable() }
  }
  Ui.KeyboardPanel {
    id: popup
    anchorItem: root.anchorItem
    owner: root.identity
    bar: root.bar
    open: root.opened
    centerOnBar: false
    focusTarget: popup.loadedContent
    contentWidth: popup.fittedContentWidth(Style.space(430))
    contentHeight: popup.fittedContentHeight(popup.loadedContentHeight, Style.space(680))
    content: Ui.PanelKeyCatcher {
      id: keyCatcher
      implicitHeight: body.implicitHeight
      anchors.fill: parent
      blocked: root.editing
      LayoutMirroring.enabled: Commons.I18n.rightToLeft
      LayoutMirroring.childrenInherit: true
      property string filterValue: "off"
      property bool blockDownloads: false
      property bool historyEnabled: false
      property bool insecure: false
      property int intervalValue: 3
      property int historyLimit: 50
      property int dedupeValue: 300
      property string userValue: userField.text
      function focusUrl() { urlField.forceActiveFocus() }
      function loadSettings() {
        var c = root.service.config || ({})
        var server = c.server || ({}), sync = c.sync || ({}), filter = c.filter || ({}), history = c.history || ({}), dedupe = c.dedupe || ({})
        urlField.text = String(server.url || ""); userField.text = String(server.user || ""); passwordField.text = ""
        intervalValue = Number(sync.intervalSec || 3); filterValue = String(filter.mode || "off")
        patternsField.text = (filter.patterns || []).join("\n")
        blockDownloads = filter.alsoBlockDownloads === true; historyEnabled = history.enabled === true
        historyLimit = Number(history.limit || 50); dedupeValue = dedupe.windowSec === undefined ? 300 : Number(dedupe.windowSec)
        insecure = server.insecure === true; testField.text = ""
      }
      function filterConfig() { return {mode: filterValue, patterns: patternsField.text.split("\n").map(function(line) { return line.replace(/\r$/, "") }).filter(function(line) { return line.length > 0 }), alsoBlockDownloads: blockDownloads} }
      function save() {
        if (!root.service) return
        var old = root.service.config || ({})
        var config = {
          server: {url: urlField.text.trim(), user: userField.text.trim(), insecure: insecure},
          sync: {intervalSec: intervalValue, enabled: old.sync ? old.sync.enabled === true : true, maxInlineBytes: old.sync && old.sync.maxInlineBytes !== undefined ? old.sync.maxInlineBytes : 32768},
          filter: filterConfig(), history: {enabled: historyEnabled, limit: historyLimit},
          dedupe: {windowSec: dedupeValue}, logging: old.logging || {level: "info"}
        }
        root.pendingConfig = config; root.pendingPassword = passwordField.text; root.pendingAccount = userField.text.trim(); passwordField.text = ""; root.formError = ""
        root.settingsRequestId = root.service.request("set_config", {config: config}) || 0
        if (!root.settingsRequestId) { root.pendingPassword = ""; root.formError = "unknown" }
      }
      onCloseRequested: root.close()
      onTabRequested: function(direction) { root.switchPanel(direction) }
      onMoveRequested: function(dx, dy) {
        root.actionIndex = Math.max(0, Math.min(root.historyMode ? 2 + 3 * (root.service ? root.service.history.length : 0) : (root.service && root.service.config.history && root.service.config.history.enabled ? 5 : 4), root.actionIndex + (dx || dy)))
        if (root.historyMode && root.actionIndex >= 3) {
          let row = historyRepeater.itemAt(Math.floor((root.actionIndex - 3) / 3))
          if (row) {
            let top = row.mapToItem(body, 0, 0).y
            if (top < scroll.contentY) scroll.contentY = Math.max(0, top)
            else if (top + row.height > scroll.contentY + scroll.height) scroll.contentY = Math.min(scroll.contentHeight - scroll.height, top + row.height - scroll.height)
          }
        } else if (root.historyMode) scroll.contentY = 0
      }
      onActivateRequested: root.activateAction()
      Flickable {
        id: scroll
        anchors.fill: parent
        contentWidth: width
        contentHeight: body.implicitHeight
        clip: true
        boundsBehavior: Flickable.StopAtBounds
        interactive: contentHeight > height
        Keys.onEscapePressed: function(event) { if (root.editing) { root.cancelEditing(); event.accepted = true } }
        Column {
          id: body
          width: scroll.width
          spacing: Style.space(10)
          Text {
            textFormat: Text.PlainText
            width: parent.width
            text: strings.t("title")
            color: root.foreground
            font.family: root.fontFamily; font.pixelSize: root.fontTokens.title; font.bold: true
          }
          Text {
            textFormat: Text.PlainText
            width: parent.width
            visible: !root.historyMode
            text: root.stateText()
            color: root.syncState === "connected" ? Commons.Color.accent : root.foreground
            font.family: root.fontFamily; font.pixelSize: root.fontTokens.body
            wrapMode: Text.Wrap
          }
          Text {
            textFormat: Text.PlainText
            width: parent.width
            visible: !root.historyMode
            text: strings.t("server_value").arg(root.service && root.service.status.url ? String(root.service.status.url) : "—")
            color: Qt.darker(root.foreground, 1.4); font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall
            elide: Text.ElideRight
          }
          Text {
            textFormat: Text.PlainText
            width: parent.width
            visible: !root.historyMode
            text: strings.t("last_sync_value").arg(root.service && root.service.status.lastSync ? new Date(Number(root.service.status.lastSync) * 1000).toLocaleString(Qt.locale(Commons.I18n.formatLocale), Locale.ShortFormat) : strings.t("never"))
            color: Qt.darker(root.foreground, 1.4); font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall
          }
          Text {
            textFormat: Text.PlainText
            visible: !!root.service && (root.favoriteError !== "" || root.service.status.historyError === true || root.service.status.logError === true || root.service.status.dedupeFull === true || root.service.errorCode !== "")
            width: parent.width
            text: !root.service ? "" : root.favoriteError !== "" ? root.errorText(root.favoriteError) : root.service.status.historyError ? strings.t("history_error") : root.service.status.logError ? strings.t("log_error") : root.service.status.dedupeFull ? strings.t("dedupe_full") : root.errorText(root.service.errorCode)
            color: Commons.Color.urgent; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; wrapMode: Text.Wrap
          }
          Flow {
            visible: !root.editing && !root.historyMode
            width: parent.width
            spacing: Style.space(4)
            Ui.Button { text: strings.t("upload"); focusable: true; hasCursor: root.actionIndex === 0 && keyCatcher.activeFocus; enabled: !!root.service && root.service.ready; onClicked: root.service.push() }
            Ui.Button { text: strings.t("download"); focusable: true; hasCursor: root.actionIndex === 1 && keyCatcher.activeFocus; enabled: !!root.service && root.service.ready; onClicked: root.service.pull(false) }
            Ui.Button { text: strings.t(root.syncEnabled ? "pause" : "resume"); focusable: true; hasCursor: root.actionIndex === 2 && keyCatcher.activeFocus; enabled: !!root.service && root.service.ready; onClicked: root.service.toggleSync() }
            Ui.Button { text: strings.t("settings"); focusable: true; hasCursor: root.actionIndex === 3 && keyCatcher.activeFocus; enabled: !!root.service && root.service.ready && root.service.configReady; onClicked: root.startEditing() }
          }
          Ui.PanelSectionHeader { visible: !root.editing; width: parent.width; text: strings.t("history"); foreground: root.foreground }
          Text {
            textFormat: Text.PlainText
            visible: root.historyMode && !root.editing && root.service && root.service.config.history && root.service.config.history.enabled
            width: parent.width
            text: strings.t("favorite_note")
            color: Qt.darker(root.foreground, 1.4); font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; wrapMode: Text.Wrap
          }
          Text {
            textFormat: Text.PlainText
            visible: !root.editing && (!root.service || !root.service.config.history || !root.service.config.history.enabled || root.service.history.length === 0)
            width: parent.width
            text: root.service && root.service.config.history && root.service.config.history.enabled ? strings.t("no_history") : strings.t("history_disabled")
            color: Qt.darker(root.foreground, 1.4); font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall
          }
          Repeater {
            model: !root.editing && !root.historyMode && root.service && root.service.config.history && root.service.config.history.enabled ? root.service.history.slice(0, 5) : []
            delegate: Text {
              required property var modelData
              textFormat: Text.PlainText
              width: body.width
              text: String(modelData.text || "").replace(/[\r\n]+/g, " ")
              color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; elide: Text.ElideRight
            }
          }
          Ui.Button {
            visible: !root.editing && !root.historyMode
            text: strings.t("open_history"); focusable: true; hasCursor: root.actionIndex === 4 && keyCatcher.activeFocus
            onClicked: root.showHistory()
          }
          Flow {
            visible: root.historyMode && !root.editing
            width: parent.width; spacing: Style.space(4)
            Ui.Button { text: strings.t("back"); focusable: true; hasCursor: root.actionIndex === 0 && keyCatcher.activeFocus; onClicked: { root.historyMode = false; root.actionIndex = 0 } }
            Ui.Button { text: strings.t("previous"); enabled: !!root.service && root.service.historyOffset > 0; focusable: true; hasCursor: root.actionIndex === 1 && keyCatcher.activeFocus; onClicked: { root.service.reloadHistory(Math.max(0, root.service.historyOffset - 20)); root.actionIndex = 0 } }
            Ui.Button { text: strings.t("next"); enabled: !!root.service && root.service.historyOffset + 20 < root.service.historyTotal; focusable: true; hasCursor: root.actionIndex === 2 && keyCatcher.activeFocus; onClicked: { root.service.reloadHistory(root.service.historyOffset + 20); root.actionIndex = 0 } }
          }
          Repeater {
            id: historyRepeater
            model: root.historyMode && !root.editing && root.service && root.service.config.history && root.service.config.history.enabled ? root.service.history : []
            delegate: Column {
              id: historyRow
              required property var modelData
              required property int index
              width: body.width; spacing: Style.space(3)
              Text { textFormat: Text.PlainText; width: parent.width; text: String(historyRow.modelData.text || "").replace(/[\r\n]+/g, " "); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; elide: Text.ElideRight }
              Flow {
                width: parent.width; spacing: Style.space(4)
                Ui.Button { text: strings.t("copy_entry"); focusable: true; hasCursor: root.actionIndex === 3 + historyRow.index * 3 && keyCatcher.activeFocus; onClicked: root.selectHistory(historyRow.modelData.hash, false) }
                Ui.Button { text: strings.t("paste_entry"); focusable: true; hasCursor: root.actionIndex === 4 + historyRow.index * 3 && keyCatcher.activeFocus; onClicked: root.selectHistory(historyRow.modelData.hash, true) }
                Ui.Button { text: strings.t(historyRow.modelData.favorite === true ? "unfavorite_entry" : "favorite_entry"); focusable: true; enabled: root.favoriteRequestId === 0; hasCursor: root.actionIndex === 5 + historyRow.index * 3 && keyCatcher.activeFocus; onClicked: root.toggleFavorite(historyRow.modelData) }
              }
            }
          }
          Ui.Button {
            visible: !root.editing && !root.historyMode && root.service && root.service.config.history && root.service.config.history.enabled
            text: strings.t("clear_history"); focusable: true; hasCursor: root.actionIndex === 5 && keyCatcher.activeFocus
            onClicked: root.service.request("clear_history")
          }
          Column {
            id: settingsColumn
            visible: root.editing
            width: parent.width
            spacing: Style.space(7)
            Ui.PanelSectionHeader { width: parent.width; text: strings.t("settings"); foreground: root.foreground }
            Text { textFormat: Text.PlainText; width: parent.width; text: strings.t("setup_hint"); color: Qt.darker(root.foreground, 1.4); font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; wrapMode: Text.Wrap }
            Text { textFormat: Text.PlainText; text: strings.t("url"); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall }
            Ui.TextField { id: urlField; width: parent.width; LayoutMirroring.enabled: false; placeholderText: "https://…" }
            Text { textFormat: Text.PlainText; text: strings.t("username"); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall }
            Ui.TextField { id: userField; width: parent.width }
            Text { textFormat: Text.PlainText; text: strings.t("password"); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall }
            Ui.TextField { id: passwordField; width: parent.width; password: true; placeholderText: strings.t("password_hint") }
            Ui.NumberField { field.locale: Qt.locale(Commons.I18n.formatLocale); label: strings.t("interval"); from: 1; to: 30; value: keyCatcher.intervalValue; onModified: function(v) { keyCatcher.intervalValue = v } }
            Ui.Dropdown {
              width: parent.width; label: strings.t("filter"); value: keyCatcher.filterValue
              options: [{value:"off",label:strings.t("filter_off")},{value:"blacklist",label:strings.t("filter_blacklist")},{value:"whitelist",label:strings.t("filter_whitelist")}]
              onChanged: function(v) { keyCatcher.filterValue = v }
            }
            Text { textFormat: Text.PlainText; width: parent.width; text: strings.t("patterns"); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; wrapMode: Text.Wrap }
            Controls.TextArea {
              id: patternsField; width: parent.width; height: Style.space(80); textFormat: TextEdit.PlainText; wrapMode: TextEdit.NoWrap
              font.family: root.fontFamily; font.pixelSize: root.fontTokens.body; color: root.foreground; selectByMouse: true
              background: Rectangle { color: Style.controlFill(patternsField.activeFocus, patternsField.hovered, root.foreground, Commons.Color.accent); radius: Style.cornerRadius; border.color: root.popupColors.border }
            }
            Ui.TextField { id: testField; width: parent.width; placeholderText: strings.t("test_text") }
            Row {
              spacing: Style.space(8)
              Ui.Button { text: strings.t("test_regex"); focusable: true; onClicked: root.testRequestId = root.service.request("test_filter", {filter: keyCatcher.filterConfig(), text: testField.text}) || 0 }
              Text { textFormat: Text.PlainText; anchors.verticalCenter: parent.verticalCenter; text: root.testResult === "" ? "" : (root.testResult === "test_match" || root.testResult === "test_no_match" ? strings.t(root.testResult) : root.errorText(root.testResult)); color: root.foreground; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall }
            }
            Ui.Toggle { width: parent.width; label: strings.t("block_downloads"); checked: keyCatcher.blockDownloads; onClicked: keyCatcher.blockDownloads = !keyCatcher.blockDownloads }
            Ui.Toggle { width: parent.width; label: strings.t("history_enabled"); description: strings.t("history_note"); checked: keyCatcher.historyEnabled; onClicked: keyCatcher.historyEnabled = !keyCatcher.historyEnabled }
            Ui.NumberField { field.locale: Qt.locale(Commons.I18n.formatLocale); label: strings.t("history_limit"); from: 1; to: 1000; value: keyCatcher.historyLimit; onModified: function(v) { keyCatcher.historyLimit = v } }
            Ui.NumberField { field.locale: Qt.locale(Commons.I18n.formatLocale); label: strings.t("dedupe"); from: 0; to: 86400; value: keyCatcher.dedupeValue; onModified: function(v) { keyCatcher.dedupeValue = v } }
            Ui.Toggle { width: parent.width; label: strings.t("insecure"); description: strings.t("insecure_note"); checked: keyCatcher.insecure; onClicked: keyCatcher.insecure = !keyCatcher.insecure }
            Text { textFormat: Text.PlainText; visible: root.formError !== ""; width: parent.width; text: root.errorText(root.formError); color: Commons.Color.urgent; font.family: root.fontFamily; font.pixelSize: root.fontTokens.bodySmall; wrapMode: Text.Wrap }
            Row {
              spacing: Style.space(8)
              Ui.Button { text: strings.t("save"); focusable: true; onClicked: keyCatcher.save() }
              Ui.Button { text: strings.t("cancel"); focusable: true; onClicked: root.cancelEditing() }
            }
          }
        }
      }
    }
  }
}
