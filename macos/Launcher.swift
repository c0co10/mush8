// MK8 Native local launcher. Uses Mush8's existing phone pad and wireless transports.
import AppKit
import SwiftUI

@MainActor final class GameLauncher: ObservableObject {
    @Published var network = "off"
    @Published var lanPlayServer = ""
    @Published var phonePads = true
    @Published var language = "zh-Hans"

    init() {
        let env = ProcessInfo.processInfo.environment
        network = env["NX_NET"] ?? "off"
        lanPlayServer = env["NX_LANPLAY_SERVER"] ?? ""
        language = env["NX_LANGUAGE"] ?? "system"
        if let saved = UserDefaults.standard.dictionary(forKey: "gameSettings") {
            network = saved["network"] as? String ?? network
            lanPlayServer = saved["lanplay_server"] as? String ?? lanPlayServer
            phonePads = saved["phone_pads"] as? Bool ?? true
            language = saved["language"] as? String ?? language
        }
    }

    var serverAddress: String { lanPlayServer.trimmingCharacters(in: .whitespacesAndNewlines) }
    var canStart: Bool { network != "lanplay" || !serverAddress.isEmpty }
    var networkDescription: String {
        switch network {
        case "lanplay": return "Mac 和 iPhone 都选 Online，填写同一 LAN-Play 服务器地址，再进入游戏中的 Online 菜单。省略端口时使用 11451；当前支持无需登录的服务器。"
        case "wireless": return "附近的 Mac / iPhone 均选择 Wireless，保持 Wi-Fi 开启，再进入游戏中的 Wireless 菜单建房或加入。"
        case "lan": return "两端选择 LAN 并连接同一局域网，再进入游戏中的“局域网”菜单。"
        default: return "联网模式关闭，仍可使用同一 Wi-Fi 上的 iPhone 作为控制器。"
        }
    }

    func play() {
        guard canStart else { return }
        UserDefaults.standard.set(["network": network, "lanplay_server": serverAddress, "phone_pads": phonePads, "language": language], forKey: "gameSettings")
        for key in ["NX_LAN", "NX_LAN_IF", "NX_LANPLAY_SERVER"] { unsetenv(key) }
        setenv("NX_NET", network, 1)
        if network == "lanplay" { setenv("NX_LANPLAY_SERVER", serverAddress, 1) }
        setenv("NX_PHONE_PADS", phonePads ? "1" : "0", 1)
        if language == "system" { unsetenv("NX_LANGUAGE") }
        else { setenv("NX_LANGUAGE", language, 1) }
        NSApp.stopModal(withCode: .OK)
    }
}

struct MK8LauncherView: View {
    @StateObject private var game = GameLauncher()
    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("MK8 Native").font(.largeTitle.bold())
            Text("马里奥赛车 8 豪华版 · 本地原生运行").foregroundStyle(.secondary)
            VStack(alignment: .leading, spacing: 12) {
                Picker("多人连接", selection: $game.network) {
                    Text("关闭").tag("off")
                    Text("LAN").tag("lan")
                    Text("Online · Server").tag("lanplay")
                    Text("Wireless").tag("wireless")
                }.pickerStyle(.segmented)
                if game.network == "lanplay" {
                    TextField("Server (host:port)，例如 example.org:11451", text: $game.lanPlayServer)
                        .textFieldStyle(.roundedBorder)
                        .help("填写主机名或 IP，可加端口；不需要 http:// 或 https://。")
                    if game.serverAddress.isEmpty {
                        Text("请先填写 LAN-Play 服务器地址。").font(.caption).foregroundStyle(.orange)
                    }
                }
                Text(game.networkDescription)
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Toggle("允许 iPhone 作为控制器", isOn: $game.phonePads)
                Text("手机和 Mac 连接同一 Wi‑Fi。Mac 启动游戏后，手机在启动页点“Use as a controller for a Mac”，无需在手机启动游戏。")
                    .font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                Picker("游戏语言", selection: $game.language) {
                    Text("跟随系统").tag("system")
                    Text("简体中文").tag("zh-Hans")
                    Text("繁體中文").tag("zh-Hant")
                    Text("English (US)").tag("en-US")
                    Text("English (UK)").tag("en-GB")
                    Text("日本語").tag("ja")
                    Text("한국어").tag("ko")
                    Text("Français").tag("fr")
                    Text("Français (Canada)").tag("fr-CA")
                    Text("Deutsch").tag("de")
                    Text("Italiano").tag("it")
                    Text("Español").tag("es")
                    Text("Español (Latinoamérica)").tag("es-419")
                    Text("Nederlands").tag("nl")
                    Text("Português (Portugal)").tag("pt-PT")
                    Text("Русский").tag("ru")
                }
            }
            Button("开始游戏") { game.play() }.keyboardShortcut(.defaultAction).disabled(!game.canStart)
        }.padding(28).frame(width: 580)
    }
}

@MainActor private final class SettingsWindowDelegate: NSObject, NSWindowDelegate {
    func windowWillClose(_ notification: Notification) { NSApp.abortModal() }
}

// Runs before guest initialization, in nxhost itself, preserving the app's file permissions.
@_cdecl("mk8ShowMacSettings")
@MainActor public func mk8ShowMacSettings() -> Int32 {
    let app = NSApplication.shared
    app.setActivationPolicy(.regular)
    app.finishLaunching()
    let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 580, height: 450),
                          styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
    window.title = "MK8 Native 设置"
    window.isReleasedWhenClosed = false
    let delegate = SettingsWindowDelegate()
    window.delegate = delegate
    window.contentView = NSHostingView(rootView: MK8LauncherView())
    window.center()
    window.makeKeyAndOrderFront(nil)
    app.activate(ignoringOtherApps: true)
    let result = app.runModal(for: window)
    window.orderOut(nil)
    withExtendedLifetime(delegate) {}
    return result == .OK ? 1 : 0
}
