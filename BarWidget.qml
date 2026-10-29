import QtQuick
import QtQuick.Effects
import qs.Commons as Commons
import qs.Ui
import "Settings.js" as Settings

BarWidget {
  id: root
  moduleName: "io.github.infinifar.omaclipsync"
  readonly property var barApi: bar
  // Style.bar is declared as QtObject upstream; its tokens are resolved dynamically.
  readonly property var barStyle: Commons.Style.bar
  readonly property var panelItem: panelLoader.item
  readonly property var service: barApi && barApi.shell && typeof barApi.shell.serviceFor === "function" ? barApi.shell.serviceFor(moduleName) : null
  readonly property bool iconShown: setting("showIcon", true) !== false
  visible: iconShown
  property bool settingsObserved: false
  property var previousWidgetSettings: ({})
  property var pendingWidgetSettings: ({})
  property var inFlightWidgetSettings: ({})
  property int widgetRequestId: 0
  property bool awaitingWidgetConfig: false
  readonly property bool opened: panelItem ? panelItem.opened === true : false
  readonly property bool popoutSwitchClosing: panelItem ? panelItem.popoutSwitchClosing === true : false
  function open() { if (panelItem) panelItem.open() }
  function close() { if (panelItem) panelItem.close() }
  function toggle() { if (panelItem) panelItem.toggle() }
  function closeForPopoutSwitch() { if (panelItem) panelItem.closeForPopoutSwitch() }
  function injectPanel() {
    var panel = panelItem
    if (!panel) return
    panel.bar = root.bar; panel.anchorItem = button; panel.hostWidget = root; panel.service = root.service
  }
  function observeSettings() {
    var current = Settings.snapshot(settings)
    if (!settingsObserved) return
    let delta = Settings.changed(previousWidgetSettings, current)
    let pending = Object.assign({}, pendingWidgetSettings, delta)
    pendingWidgetSettings = pending
    previousWidgetSettings = current
    flushWidgetSettings()
  }
  function flushWidgetSettings() {
    if (!service || !service.ready || !service.configReady || widgetRequestId || awaitingWidgetConfig || Object.keys(pendingWidgetSettings).length === 0) return
    var next = Settings.merge(service.config, pendingWidgetSettings)
    if (!next) return
    var id = service.request("set_config", {config: next})
    if (id) { widgetRequestId = id; inFlightWidgetSettings = pendingWidgetSettings; pendingWidgetSettings = ({}) }
  }
  function reflectSettings(config, showIcon) {
    if (!barApi || !barApi.shell || typeof barApi.shell.updateEntryInline !== "function") return false
    var entry = Settings.inlineEntry(moduleName, settings, config, showIcon)
    var unchanged = Settings.inlineSame(settings, entry)
    var previous = previousWidgetSettings
    var wasObserved = settingsObserved
    previousWidgetSettings = Settings.snapshot(entry)
    settingsObserved = true
    if (barApi.shell.updateEntryInline(moduleName, entry) || unchanged) return true
    previousWidgetSettings = previous
    settingsObserved = wasObserved
    return false
  }
  Connections {
    target: root.service
    ignoreUnknownSignals: true
    function onHistoryRequested() { if (root.panelItem) root.panelItem.showHistory() }
    function onIconVisibilityRequested(visible) { root.reflectSettings(null, visible) }
    function onReadyChanged() {
      if (!root.service.ready && root.widgetRequestId) {
        root.pendingWidgetSettings = Object.assign({}, root.inFlightWidgetSettings, root.pendingWidgetSettings)
        root.widgetRequestId = 0; root.inFlightWidgetSettings = ({}); root.awaitingWidgetConfig = false
      }
      root.flushWidgetSettings()
    }
    function onConfigReadyChanged() { root.flushWidgetSettings() }
    function onConfigChanged() { if (root.awaitingWidgetConfig) { root.awaitingWidgetConfig = false; root.flushWidgetSettings() } }
    function onReplied(id, method, ok, result, error) {
      if (method !== "set_config" || id !== root.widgetRequestId) return
      root.widgetRequestId = 0; root.inFlightWidgetSettings = ({})
      if (ok) root.awaitingWidgetConfig = true
      else root.flushWidgetSettings()
    }
  }
  Component.onCompleted: Qt.callLater(function() { previousWidgetSettings = Settings.snapshot(settings); settingsObserved = true })
  onBarChanged: injectPanel()
  onSettingsChanged: { injectPanel(); observeSettings() }
  onServiceChanged: { injectPanel(); flushWidgetSettings() }
  implicitWidth: button.implicitWidth
  implicitHeight: button.implicitHeight
  Commons.Translations { id: strings; directory: Qt.resolvedUrl("i18n") }
  Loader {
    id: panelLoader
    active: true
    source: Qt.resolvedUrl("Panel.qml")
    visible: false
    onLoaded: { root.injectPanel(); Qt.callLater(root.injectPanel) }
  }
  WidgetButton {
    id: button
    anchors.fill: parent
    bar: root.bar
    labelVisible: false
    hasVisualContent: true
    fixedWidth: button.vertical ? -1 : root.barStyle.iconSlot
    fixedHeight: button.vertical ? root.barStyle.iconSlot : -1
    tooltipText: strings.t("tooltip")
    foreground: root.service && !root.service.busy && (root.service.status.state === "credentials" || root.service.status.state === "clipboard_error") ? Commons.Color.urgent : (root.barApi ? root.barApi.barForeground : Commons.Color.foreground)
    dimmed: !root.service || (!root.service.busy && (root.service.status.state === "paused" || root.service.status.state === "offline"))
    onPressed: function(b) { if (b === Qt.LeftButton) root.toggle() }
    Item {
      anchors.centerIn: parent
      width: root.barStyle.iconSize
      height: width
      scale: button.pressScale
      Image {
        id: symbolicIcon
        anchors.fill: parent
        source: Qt.resolvedUrl("assets/omaclipsync-symbolic.svg")
        sourceSize.width: Math.round(width * Screen.devicePixelRatio)
        sourceSize.height: Math.round(height * Screen.devicePixelRatio)
        visible: false
        layer.enabled: true
      }
      MultiEffect {
        anchors.fill: parent
        source: symbolicIcon
        colorization: 1.0
        colorizationColor: button.foreground
      }
    }
  }
}
