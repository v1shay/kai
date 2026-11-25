import AppKit
import ObjectiveC

/// Fixed launch milestones only. Once anchored, elapsed time is monotonic and
/// shares the spawning probe's origin with Rust's KAI_LAUNCH timestamps.
@MainActor enum UnifiedLaunchStages {
    private static let origin: Double? = {
        let environment = ProcessInfo.processInfo.environment
        guard environment["KAI_PERFORMANCE_LOG"] != nil,
              let value = environment["KAI_LAUNCH_ORIGIN_UNIX_NS"],
              let nanoseconds = Double(value) else { return nil }
        let age = Date().timeIntervalSince1970 - nanoseconds / 1_000_000_000
        guard age.isFinite, age >= 0, age < 60 else { return nil }
        return ProcessInfo.processInfo.systemUptime - age
    }()
    static func mark(_ name: String) {
        guard let origin else { return }
        let elapsed = (ProcessInfo.processInfo.systemUptime - origin) * 1_000
        fputs("KAI_LAUNCH \(name) \(String(format: "%.3f", elapsed))ms\n", stderr)
    }
}

/// Hidden composers retain divergent drafts until the owner accepts the handoff.
/// Window visibility never grants permission to replace unsent local input.
enum UnifiedDraftHandoff {
    static func shouldAdopt(local:String,base:String,remote:String,selectionChanged:Bool,sendAccepted:Bool)->Bool {
        selectionChanged || sendAccepted || local == base || local == remote
    }
}

/// Writes are ordered on the owner command queue. A fast typist can enqueue
/// several edits before the first snapshot returns; each edit compares against
/// the preceding edit, not the last frame the notch happened to display.
struct UnifiedDraftWrites {
    private var owner:String?
    private var pending:[(id:String,text:String)]=[]
    var pendingText:String?{pending.last?.text}
    mutating func enqueue(id:String,text:String){pending.append((id,text))}
    mutating func receive(owner:String?,result:(String)->KaiRequestResult?)->String?{
        guard self.owner == owner else{self.owner=owner;pending=[];return nil}
        var accepted:String?
        while let first=pending.first,let reply=result(first.id){
            pending.removeFirst()
            if reply.error == nil{accepted=first.text}
        }
        return accepted
    }
}

/// C ABI boundary for Code's GPUI owner. Both sides run on the AppKit thread.
/// No socket, helper process, transcript scan, or independent agent connection.
@MainActor final class EmbeddedCodeRuntime {
    static let shared=EmbeddedCodeRuntime()
    typealias Command = @convention(c) (UnsafePointer<UInt8>?,Int)->Bool
    private(set) var active=false
    private var quitReady = false
    private var command:Command?
    private var app:KaiPetApp?
    var makeApp:()->KaiPetApp={KaiPetApp()}
    weak var workspaceWindow:NSWindow?
    var workspaceFocusProvider:(()->Bool)?
    var windowFocusProvider:((NSWindow)->Bool)?
    func isForeground(_ window:NSWindow?)->Bool{
        guard let window else{return false}
        return windowFocusProvider?(window) ?? (NSApp.isActive && window.isVisible && window.isKeyWindow)
    }
    var workspaceIsForeground:Bool{workspaceFocusProvider?() ?? (NSApp.isActive && workspaceWindow?.isVisible == true && workspaceWindow?.isKeyWindow == true)}
    var onLevels:(([CGFloat])->Void)?
    var onState:((KaiState)->Void)?
    var onError:((String)->Void)?
    private var wallpaperVisible:Bool?
    private let packetDecoder = JSONDecoder()
    private var lastState:Data?
    private var historyOwner:String?
    private var history=[KaiMessage]()
    private var diagnosticCommands=0
    #if canImport(libghostty)
    private lazy var chatTerminal = UnifiedChatTerminal()
    private lazy var dockTerminals = DockedGhosttyTerminals()
    #endif

    func start(window:NSWindow,command:@escaping Command,presentInitialNotch:Bool=true) {
        guard !active else{return}
        UnifiedLaunchStages.mark("native-swift-entered")
        active=true;self.command=command;workspaceWindow=window
        installTerminationGate()
        NativeWorkspaceIntegrations.shared.install()
        let defaults=UserDefaults.standard
        if NativePreferencePersistence.enabled && !defaults.bool(forKey:"unifiedPreferencesSeeded") {
            for (key,value) in defaults.persistentDomain(forName:"com.kai.notchprototype") ?? [:] where defaults.object(forKey:key) == nil && key != "experimentalWebMode"{defaults.set(value,forKey:key)}
            defaults.set(true,forKey:"unifiedPreferencesSeeded")
        }
        if window.isVisible {window.orderOut(nil)}
        UnifiedLaunchStages.mark("native-delegate-objects-begin")
        let app=makeApp();self.app=app
        UnifiedLaunchStages.mark("native-delegate-objects-ready")
        app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
        if presentInitialNotch {self.app?.showUnifiedNotchOnLaunch()}
        NotificationCenter.default.addObserver(forName:NSApplication.willTerminateNotification,object:nil,queue:.main){[weak app] note in
            MainActor.assumeIsolated{app?.applicationWillTerminate(note)}
        }
    }
    // Dock Quit and native terminate: must use the same durable shutdown as
    // Command-Q. Permit AppKit termination only after the owner flushes history.
    private func installTerminationGate() {
        guard let delegate = NSApp.delegate, let cls = object_getClass(delegate) else { return }
        let selector = NSSelectorFromString("applicationShouldTerminate:")
        guard class_getInstanceMethod(cls, selector) == nil else { return }
        let gate: @convention(block) (AnyObject, NSApplication) -> UInt = { _, _ in
            MainActor.assumeIsolated {
                let runtime = EmbeddedCodeRuntime.shared
                if !runtime.active || runtime.quitReady { return UInt(NSApplication.TerminateReply.terminateNow.rawValue) }
                runtime.app?.quitKai()
                return UInt(NSApplication.TerminateReply.terminateCancel.rawValue)
            }
        }
        class_addMethod(cls, selector, imp_implementationWithBlock(gate), "Q@:@")
    }
    @discardableResult func send(_ data:Data)->Bool {
        guard let command else{return false}
        let nativeTrace = NativeFrameTiming.begin(.runtimeSend)
        defer { NativeFrameTiming.end(nativeTrace) }
        if diagnosticCommands<64,ProcessInfo.processInfo.environment["KAI_PERFORMANCE_LOG"] != nil,
           let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any],let action=value["action"] as? String {
            diagnosticCommands+=1;fputs("KAI_NATIVE_COMMAND \(action)\n",stderr)
        }
        let accepted=data.withUnsafeBytes{raw in command(raw.bindMemory(to:UInt8.self).baseAddress,raw.count)}
        if !accepted{onError?("The workspace is busy; your input is preserved. Try again.")}
        return accepted
    }
    func receive(_ data:Data) {
        if NativeFrameTiming.consumeFlushPacket(data) { return }
        let nativeTrace = NativeFrameTiming.begin(.runtimeReceive)
        defer { NativeFrameTiming.end(nativeTrace) }
        guard data != lastState else{return}
        guard let packet=try? packetDecoder.decode(OwnerPacket.self,from:data) else{onError?("Could not read Code’s owner state");fputs("Kai unified state decode failed\n",stderr);return}
        // Only events with dynamic payloads need a second pass. Owner state and
        // the typed presentation events are already fully decoded.
        if let type = packet.type, Self.hasDynamicPayload(type),
           let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            if type == "workspaceFeedback" {
                if let text = object["message"] as? String {
                    if let host = app?.unifiedVoiceHost() {
                        app?.paneFeedback.notify(text, error: object["error"] as? Bool ?? false, in: host)
                    }
                } else if let host = workspaceWindow?.contentView,
                          let owner = object["owner"] as? String,
                          let phase = object["phase"] as? String,
                          let r = object["rect"] as? [Double], r.count == 4 {
                    app?.updatePaneFeedback(owner: owner, phase: phase,
                        rect: CGRect(x: r[0], y: r[1], width: r[2], height: r[3]), in: host)
                }
                return
            }
            if object["type"] as? String == "nativeMenuAction",let id=object["id"] as? String {
                DispatchQueue.main.async{[weak self] in self?.app?.activateNativeSetting(id)};return
            }
            if object["type"] as? String == "terminalInputGate" {
                let blocked = object["blocked"] as? Bool ?? true
                app?.workspaceOverlayOcclusion(blocked)
                #if canImport(libghostty)
                dockTerminals.setInputBlocked(blocked); chatTerminal.setInputBlocked(blocked)
                #endif
                return
            }
            #if canImport(libghostty)
            if let type = object["type"] as? String, type.hasPrefix("dockTerminal"),
               let id = object["id"] as? String {
                switch type {
                case "dockTerminalSlot":
                    if let rect = object["rect"] as? [Double], rect.count == 4,
                       let clip = object["clip"] as? [Double], clip.count == 4,
                       let window = workspaceWindow {
                        dockTerminals.update(id: id, directory: object["directory"] as? String, command: object["command"] as? String,
                            rect: CGRect(x: rect[0], y: rect[1], width: rect[2], height: rect[3]),
                            clip: CGRect(x: clip[0], y: clip[1], width: clip[2], height: clip[3]),
                            focused: object["focused"] as? Bool ?? false, window: window)
                    }
                case "dockTerminalAnimateResize": dockTerminals.animateNextResize(id: id)
                case "dockTerminalHide": dockTerminals.hide(id: id)
                case "dockTerminalClose": dockTerminals.close(id: id)
                default: break
                }
                return
            }
            if object["type"] as? String == "chatTerminalOwner" {
                chatTerminal.setVisibleChats(active: object["id"] as? String, peers: object["peers"] as? [String] ?? [])
                return
            }
            if object["type"] as? String == "chatTerminalSlot",
               let id = object["id"] as? String, let rect = object["rect"] as? [Double], rect.count == 4 {
                chatTerminal.setChatBounds(CGRect(x: rect[0], y: rect[1], width: rect[2], height: rect[3]), for: id)
                return
            }
            #endif
            if let type = object["type"] as? String,
               ["chatTerminalOpen", "chatTerminalToggle", "chatTerminalClose", "chatTerminalAdopt"].contains(type),
               let id = object["id"] as? String,
               let launchId = object["launchId"] as? String, UUID(uuidString: launchId) != nil {
                let arguments = object["arguments"] as? [String]
                let directory = object["directory"] as? String
                let environment = object["environment"] as? [[String]] ?? []
                let newId = object["newId"] as? String
                let title = object["title"] as? String
                let agent = object["agent"] as? String ?? "Agent"
                let selected = object["selected"] as? Bool ?? true
                let acknowledge = object["acknowledge"] as? Bool ?? false
                #if canImport(libghostty)
                if type == "chatTerminalClose", chatTerminal.hasSurface(id: id, launchId: launchId) {
                    // Retire the visible surface before the next owner packet hides it.
                    chatTerminal.close(id: id, launchId: launchId, acknowledge: acknowledge)
                    return
                }
                #endif
                DispatchQueue.main.async { [weak self] in
                    guard let self else { return }
                    #if canImport(libghostty)
                    if type == "chatTerminalToggle" { self.chatTerminal.select(id: id, launchId: launchId, selected: selected) }
                    else if type == "chatTerminalClose" { self.chatTerminal.close(id: id, launchId: launchId, acknowledge: acknowledge) }
                    else if type == "chatTerminalAdopt",let newId { self.chatTerminal.adopt(id: id, newId: newId, launchId: launchId) }
                    else if type == "chatTerminalOpen", let arguments, let directory, let window = self.workspaceWindow {
                        self.chatTerminal.open(id: id, arguments: arguments,
                                               directory: directory, title: title ?? "CLI", window: window,
                                               environment: environment, launchId: launchId, agent: agent)
                    } else if type == "chatTerminalOpen" {
                        self.sendAction("chat_terminal_closed", ["id": id, "launchId": launchId, "error": "Cannot open this CLI session.", "requestId": UUID().uuidString])
                    }
                    #else
                    if type == "chatTerminalOpen" {
                        self.sendAction("chat_terminal_fallback", ["id": id, "launchId": launchId, "requestId": UUID().uuidString])
                    }
                    #endif
                }
                return
            }
            if type.hasPrefix("adaptiveText"){AdaptiveTextBackdrop.shared.receive(object);return}
            if object["type"] as? String == "inlineQuickLook", let slot = object["slot"] as? String {
                let rect = (object["rect"] as? [Double]).flatMap { values -> CGRect? in
                    guard values.count == 4 else { return nil }
                    return CGRect(x: values[0], y: values[1], width: values[2], height: values[3])
                }
                NativeWorkspaceIntegrations.shared.inlineQuickLook(slot: slot,
                    url: (object["path"] as? String).map { URL(fileURLWithPath: $0) },
                    windowNumber: object["window"] as? Int, rect: rect)
                return
            }
            if object["type"] as? String == "quickLook",let paths=object["paths"] as? [String]{NativeWorkspaceIntegrations.shared.quickLook(paths.map{URL(fileURLWithPath:$0)});return}
            if object["type"] as? String == "copyFile",let path=object["path"] as? String{NativeWorkspaceIntegrations.shared.copyFile(URL(fileURLWithPath:path));return}
            if object["type"] as? String == "artifactPreview",let artifact=object["artifact"] as? [String:Any]{NativeWorkspaceIntegrations.shared.artifact(artifact);return}
        }
        switch packet.type {
        case "quitReady": quitReady = true; return
        case "paneMotion":
            if let millis = packet.durationMs, (160...300).contains(millis) { KaiMotion.paneDuration = millis / 1000 }
            return
        case "nativeSettingsCatalog":DispatchQueue.main.async{[weak self] in self?.app?.publishNativeSettingsCatalog()};return
        case "keyboardCatalog":keyboardCommands=packet.commands ?? [];return
        case "menuBarToggle":DispatchQueue.main.async{[weak self] in self?.app?.toggleMenuBarPanel()};return
        case "quickMiniPrepare":app?.prepareMiniOpening();return
        case "quickPrepare":app?.prepareQuickOpening();return
        case "workspaceReveal":app?.revealWorkspace();return
        case "launchWorkspace":app?.launchUnifiedWorkspace();return
        case "launchExtended":app?.expandToRunningChat();return
        case "launchCompact":app?.showUnifiedNotchOnLaunch();return
        case "workspaceDismiss":DispatchQueue.main.async{[weak self] in self?.app?.dismissUnifiedSurface()};return
        case "probeDoubleCommand":if CommandLine.arguments.contains("--unified-smoke-test"){app?.probeDoubleCommand()};return
        case "probeMenuBarClick":if CommandLine.arguments.contains("--unified-smoke-test"){app?.probeMenuBarClick()};return
        case "quickVisibility":if let visible=packet.visible{app?.setQuickChatVisible(visible)};return
        case "ninjaAtmosphereSlot":if let rect=packet.rect,rect.count==4{app?.ninjaAtmosphereGeometry(CGRect(x:rect[0],y:rect[1],width:rect[2],height:rect[3]))};return
        case "workspacePetSlot":
            if let frame = packet.rect, frame.count == 4 {
                app?.workspacePetGeometry(CGRect(x: frame[0], y: frame[1], width: frame[2], height: frame[3]))
            } else {
                app?.workspacePetGeometry(nil)
            }
            return
        case "quickPetSlot":if let frame=packet.rect,frame.count==4{app?.quickPetGeometry(CGRect(x:frame[0],y:frame[1],width:frame[2],height:frame[3]))};return
        default:break
        }
        guard var state=packet.state else{return}
        let carriesHistory=packet.hasHistory
        if carriesHistory{history=state.history;historyOwner=state.threadId}
        else if historyOwner==state.threadId{state.history=history}
        else{history=[];historyOwner=state.threadId}
        lastState=data;onState?(state)
    }
    private static func hasDynamicPayload(_ type: String) -> Bool {
        if type == "workspaceFeedback" { return true }

        switch type {
        case "nativeMenuAction", "inlineQuickLook", "quickLook", "copyFile", "artifactPreview":
            return true
        default:
            return type == "terminalInputGate" || type.hasPrefix("adaptiveText") || type.hasPrefix("chatTerminal") || type.hasPrefix("dockTerminal")
        }
    }

    struct KeyboardCommand:Decodable {let key:String,id:String,label:String;let toggle:Bool?}
    private(set) var keyboardCommands=[KeyboardCommand]()
    private struct OwnerPacket:Decodable {
        let type:String?,visible:Bool?,rect:[Double]?,state:KaiState?,hasHistory:Bool
        let commands:[KeyboardCommand]?
        let durationMs: Double?
        private enum Keys:String,CodingKey{case type,visible,rect,history,commands,durationMs}
        init(from decoder:Decoder)throws{
            let fields=try decoder.container(keyedBy:Keys.self)
            commands=try fields.decodeIfPresent([KeyboardCommand].self,forKey:.commands)
            durationMs=try fields.decodeIfPresent(Double.self,forKey:.durationMs)
            type=try fields.decodeIfPresent(String.self,forKey:.type)
            if type == nil || type == "state"{state=try KaiState(from:decoder);hasHistory=fields.contains(.history);visible=nil;rect=nil}
            else{state=nil;hasHistory=false;visible=try fields.decodeIfPresent(Bool.self,forKey:.visible);rect=try fields.decodeIfPresent([Double].self,forKey:.rect)}
        }
    }
    var ready:Bool{active && onState != nil && lastState != nil}
    func setWallpaperVisible(_ visible:Bool){guard active,visible != wallpaperVisible else{return};if sendAction("workspace_wallpaper_visibility",["visible":visible]){wallpaperVisible=visible}}
    func receiveWallpaper(_ bytes:UnsafePointer<UInt8>?,count:Int,width:Int,height:Int,opacity:Float){
        guard let bytes else{app?.setWorkspaceWallpaper(nil,opacity:0);return}
        guard (1...512).contains(width),(1...512).contains(height),count==width*height*4,opacity.isFinite else{return}
        let data=Data(bytes:bytes,count:count)
        guard let provider=CGDataProvider(data:data as CFData),let image=CGImage(width:width,height:height,bitsPerComponent:8,bitsPerPixel:32,bytesPerRow:width*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGBitmapInfo(rawValue:CGImageAlphaInfo.last.rawValue),provider:provider,decode:nil,shouldInterpolate:true,intent:.defaultIntent) else{return}
        app?.setWorkspaceWallpaper(image,opacity:opacity)
    }
    func presentation(_ phase:String){var now=timespec();clock_gettime(CLOCK_MONOTONIC_RAW,&now);sendAction("workspace_presentation",["phase":phase,"started":Double(now.tv_sec)+Double(now.tv_nsec)/1_000_000_000])}
    func probeDockTerminal(id: String, input: Bool, value: String) -> Bool {
        #if canImport(libghostty)
        return dockTerminals.probe(id: id, input: input, value: value)
        #else
        return false
        #endif
    }

    #if canImport(libghostty)
    func chatTerminalOwnsResponder(_ terminal: WorkspaceGhosttyView) -> Bool {
        chatTerminal.ownsResponder(terminal)
    }
    #endif

    func probeFocusedTerminalShortcut(keyCode: UInt16, character: String) -> Bool {
        #if canImport(libghostty)
        guard let window = workspaceWindow, window.isVisible else {
            fputs("KAI_NATIVE_SHORTCUT_PROBE workspace unavailable\n", stderr)
            return false
        }
        guard window.firstResponder is WorkspaceGhosttyView || dockTerminals.focusVisibleTerminalForProbe(in: window) else {
            fputs("KAI_NATIVE_SHORTCUT_PROBE terminal responder unavailable\n", stderr)
            return false
        }
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero,
                   modifierFlags: .control, timestamp: ProcessInfo.processInfo.systemUptime,
                   windowNumber: window.windowNumber, context: nil, characters: "\n",
                   charactersIgnoringModifiers: character, isARepeat: false, keyCode: keyCode) else { return false }
        let handled = app?.performNativeKeyboardCommand(event) ?? false
        if !handled {
            let binding = keyboardCommands.first { $0.key == "ctrl-" + character }
            fputs("KAI_NATIVE_SHORTCUT_PROBE key=\(event.charactersIgnoringModifiers ?? "?") target=\(event.window === window) keyWindow=\(NSApp.keyWindow === window) commands=\(keyboardCommands.count) toggle=\(binding?.toggle == true)\n", stderr)
        }
        return handled
        #else
        return false
        #endif
    }

    func sendWorkspaceTerminalCommand(_ command: String) -> Bool {
        #if canImport(libghostty)
        if let terminal = workspaceWindow?.firstResponder as? WorkspaceGhosttyView {
            return chatTerminal.sendCommand(command, from: terminal) || dockTerminals.sendCommand(command, from: terminal)
        }
        #endif
        return false
    }

    @discardableResult
    func sendAction(_ action: String, _ fields: [String: Any] = [:]) -> Bool {
        var value = fields
        value["action"] = action
        if action == "quick_dismiss" {
            var now = timespec()
            clock_gettime(CLOCK_MONOTONIC_RAW, &now)
            value["issuedAt"] = Double(now.tv_sec) + Double(now.tv_nsec) / 1_000_000_000
        }
        guard let data = try? JSONSerialization.data(withJSONObject: value) else { return false }
        return send(data)
    }
    var voiceHost:NSView?{app?.unifiedVoiceHost()}
    func diagnostics()->UInt32{app?.unifiedPresentationDiagnostics() ?? 0}
    func toggle(){app?.toggleUnifiedSurface()}
    func collapse(){app?.foldWorkspace()}
    func quickSurface(_ window:NSWindow,show:Bool)->Bool{app?.transitionQuickSurface(window,show:show) ?? false}
}

#if KAI_UNIFIED
@_cdecl("kai_unified_wallpaper")
public func kaiUnifiedWallpaper(_ bytes:UnsafePointer<UInt8>?,_ count:Int,_ width:Int,_ height:Int,_ opacity:Float){guard Thread.isMainThread else{return};MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.receiveWallpaper(bytes,count:count,width:width,height:height,opacity:opacity)}}
@_cdecl("kai_unified_probe_terminal_toggle")
public func kaiUnifiedProbeTerminalToggle() -> Bool {
    guard Thread.isMainThread,
          CommandLine.arguments.contains("--unified-smoke-test"),
          CommandLine.arguments.contains("--unified-chat-split-probe") else { return false }
    return MainActor.assumeIsolated {
        EmbeddedCodeRuntime.shared.probeFocusedTerminalShortcut(keyCode: 38, character: "j")
    }
}

@_cdecl("kai_unified_probe_dock_terminal")
public func kaiUnifiedProbeDockTerminal(
    _ id: UnsafePointer<CChar>?, _ input: Bool, _ value: UnsafePointer<CChar>?
) -> Bool {
    guard Thread.isMainThread, let id, let value,
          CommandLine.arguments.contains("--unified-smoke-test"),
          CommandLine.arguments.contains("--unified-chat-split-probe"),
          ProcessInfo.processInfo.environment["KAI_PROBE_METAL_TERMINAL"] == "1" else { return false }
    return MainActor.assumeIsolated {
        EmbeddedCodeRuntime.shared.probeDockTerminal(id: String(cString: id), input: input, value: String(cString: value))
    }
}

@_cdecl("kai_unified_start")
public func kaiUnifiedStart(_ window:UnsafeMutableRawPointer?,_ command:(@convention(c)(UnsafePointer<UInt8>?,Int)->Bool)?, _ presentInitialNotch:Bool) {
    guard Thread.isMainThread,let window,let command else{return}
    MainActor.assumeIsolated {
        EmbeddedCodeRuntime.shared.start(window:Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue(),command:command,presentInitialNotch:presentInitialNotch)
    }
}
@_cdecl("kai_unified_publish")
public func kaiUnifiedPublish(_ bytes:UnsafePointer<UInt8>?,_ count:Int) {
    guard Thread.isMainThread,let bytes,count>0,count<=2*1024*1024 else{return}
    MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.receive(Data(bytes:bytes,count:count))}
}
@_cdecl("kai_unified_toggle")
public func kaiUnifiedToggle(){guard Thread.isMainThread else{return};MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.toggle()}}
@_cdecl("kai_unified_collapse")
public func kaiUnifiedCollapse(){guard Thread.isMainThread else{return};MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.collapse()}}
@_cdecl("kai_unified_ready")
public func kaiUnifiedReady()->Bool{guard Thread.isMainThread else{return false};return MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.ready}}
@_cdecl("kai_unified_workspace_visible")
public func kaiUnifiedWorkspaceVisible()->Bool{guard Thread.isMainThread else{return false};return MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.workspaceWindow?.isVisible == true}}
@_cdecl("kai_unified_diagnostics")
public func kaiUnifiedDiagnostics()->UInt32{guard Thread.isMainThread else{return 0};return MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.diagnostics()}}
@_cdecl("kai_unified_levels")
public func kaiUnifiedLevels(_ values:UnsafePointer<Float>?,_ count:Int){
    guard Thread.isMainThread,let values,count == 4 else{return}
    let levels=(0..<4).map{CGFloat(values[$0].isFinite ? min(1,max(0,values[$0])):0)}
    MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.onLevels?(levels)}
}
@_cdecl("kai_unified_voice_host")
public func kaiUnifiedVoiceHost()->UnsafeMutableRawPointer?{
    guard Thread.isMainThread else{return nil}
    return MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.voiceHost.map{Unmanaged.passUnretained($0).toOpaque()}}
}
@_cdecl("kai_unified_quick_surface")
public func kaiUnifiedQuickSurface(_ pointer:UnsafeMutableRawPointer?,_ visible:Bool)->Bool{
    guard Thread.isMainThread,let pointer else{return false}
    return MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.quickSurface(Unmanaged<NSWindow>.fromOpaque(pointer).takeUnretainedValue(),show:visible)}
}
#endif
