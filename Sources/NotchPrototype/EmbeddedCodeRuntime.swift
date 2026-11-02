import AppKit

/// Hidden composers retain divergent drafts until the owner accepts the handoff.
/// Window visibility never grants permission to replace unsent local input.
enum UnifiedDraftHandoff {
    static func shouldAdopt(local:String,base:String,remote:String,selectionChanged:Bool,sendAccepted:Bool)->Bool {
        selectionChanged || sendAccepted || local == base || local == remote
    }
}

/// C ABI boundary for Code's GPUI owner. Both sides run on the AppKit thread.
/// No socket, helper process, transcript scan, or independent agent connection.
@MainActor final class EmbeddedCodeRuntime {
    static let shared=EmbeddedCodeRuntime()
    typealias Command = @convention(c) (UnsafePointer<UInt8>?,Int)->Bool
    private(set) var active=false
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
    private var lastState:Data?
    private var historyOwner:String?
    private var history=[KaiMessage]()
    private var diagnosticCommands=0

    func start(window:NSWindow,command:@escaping Command) {
        guard !active else{return}
        active=true;self.command=command;workspaceWindow=window
        NativeWorkspaceIntegrations.shared.install()
        let defaults=UserDefaults.standard
        if !defaults.bool(forKey:"unifiedPreferencesSeeded") {
            for (key,value) in defaults.persistentDomain(forName:"com.kai.notchprototype") ?? [:] where defaults.object(forKey:key) == nil && key != "experimentalWebMode"{defaults.set(value,forKey:key)}
            defaults.set(true,forKey:"unifiedPreferencesSeeded")
        }
        window.orderOut(nil)
        let app=makeApp();self.app=app
        app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
        self.app?.showUnifiedNotchOnLaunch()
        NotificationCenter.default.addObserver(forName:NSApplication.willTerminateNotification,object:nil,queue:.main){[weak app] note in
            MainActor.assumeIsolated{app?.applicationWillTerminate(note)}
        }
    }
    func send(_ data:Data) {
        guard let command else{return}
        if diagnosticCommands<64,ProcessInfo.processInfo.environment["KAI_PERFORMANCE_LOG"] != nil,
           let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any],let action=value["action"] as? String {
            diagnosticCommands+=1;fputs("KAI_NATIVE_COMMAND \(action)\n",stderr)
        }
        let accepted=data.withUnsafeBytes{raw in command(raw.bindMemory(to:UInt8.self).baseAddress,raw.count)}
        if !accepted{onError?("The workspace is busy; your input is preserved. Try again.")}
    }
    func receive(_ data:Data) {
        guard data != lastState else{return}
        guard let packet=try? JSONDecoder().decode(OwnerPacket.self,from:data) else{onError?("Could not read Code’s owner state");fputs("Kai unified state decode failed\n",stderr);return}
        if let object=try? JSONSerialization.jsonObject(with:data) as? [String:Any]{
            if (object["type"] as? String)?.hasPrefix("adaptiveText") == true{AdaptiveTextBackdrop.shared.receive(object);return}
            if object["type"] as? String == "quickLook",let paths=object["paths"] as? [String]{NativeWorkspaceIntegrations.shared.quickLook(paths.map{URL(fileURLWithPath:$0)});return}
            if object["type"] as? String == "artifactPreview",let artifact=object["artifact"] as? [String:Any]{NativeWorkspaceIntegrations.shared.artifact(artifact);return}
        }
        if let object=try? JSONSerialization.jsonObject(with:data) as? [String:Any],object["type"] as? String == "ninjaAtmosphereSlot",let rect=object["rect"] as? [Double],rect.count==4{app?.ninjaAtmosphereGeometry(CGRect(x:rect[0],y:rect[1],width:rect[2],height:rect[3]));return}
        switch packet.type {
        case "keyboardCatalog":keyboardCommands=packet.commands ?? [];return
        case "quickPrepare":app?.prepareQuickOpening();return
        case "workspaceReveal":app?.revealWorkspace();return
        case "launchWorkspace":app?.launchUnifiedWorkspace();return
        case "launchExtended":app?.expandToRunningChat();return
        case "launchCompact":app?.showUnifiedNotchOnLaunch();return
        case "workspaceDismiss":app?.dismissUnifiedSurface();return
        case "probeDoubleCommand":if CommandLine.arguments.contains("--unified-smoke-test"){app?.probeDoubleCommand()};return
        case "probeMenuBarClick":if CommandLine.arguments.contains("--unified-smoke-test"){app?.probeMenuBarClick()};return
        case "quickVisibility":if let visible=packet.visible{app?.setQuickChatVisible(visible)};return
        case "workspacePetSlot":if let frame=packet.rect,frame.count==4{app?.workspacePetGeometry(CGRect(x:frame[0],y:frame[1],width:frame[2],height:frame[3]))};return
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
    struct KeyboardCommand:Decodable {let key:String,id:String,label:String}
    private(set) var keyboardCommands=[KeyboardCommand]()
    private struct OwnerPacket:Decodable {
        let type:String?,visible:Bool?,rect:[Double]?,state:KaiState?,hasHistory:Bool
        let commands:[KeyboardCommand]?
        private enum Keys:String,CodingKey{case type,visible,rect,history,commands}
        init(from decoder:Decoder)throws{
            let fields=try decoder.container(keyedBy:Keys.self)
            commands=try fields.decodeIfPresent([KeyboardCommand].self,forKey:.commands)
            type=try fields.decodeIfPresent(String.self,forKey:.type)
            if type == nil || type == "state"{state=try KaiState(from:decoder);hasHistory=fields.contains(.history);visible=nil;rect=nil}
            else{state=nil;hasHistory=false;visible=try fields.decodeIfPresent(Bool.self,forKey:.visible);rect=try fields.decodeIfPresent([Double].self,forKey:.rect)}
        }
    }
    var ready:Bool{active && onState != nil && lastState != nil}
    func setWallpaperVisible(_ visible:Bool){guard active,visible != wallpaperVisible else{return};wallpaperVisible=visible;sendAction("workspace_wallpaper_visibility",["visible":visible])}
    func receiveWallpaper(_ bytes:UnsafePointer<UInt8>?,count:Int,width:Int,height:Int,opacity:Float){
        guard let bytes else{app?.setWorkspaceWallpaper(nil,opacity:0);return}
        guard (1...512).contains(width),(1...512).contains(height),count==width*height*4,opacity.isFinite else{return}
        let data=Data(bytes:bytes,count:count)
        guard let provider=CGDataProvider(data:data as CFData),let image=CGImage(width:width,height:height,bitsPerComponent:8,bitsPerPixel:32,bytesPerRow:width*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGBitmapInfo(rawValue:CGImageAlphaInfo.last.rawValue),provider:provider,decode:nil,shouldInterpolate:true,intent:.defaultIntent) else{return}
        app?.setWorkspaceWallpaper(image,opacity:opacity)
    }
    func presentation(_ phase:String){var now=timespec();clock_gettime(CLOCK_MONOTONIC_RAW,&now);sendAction("workspace_presentation",["phase":phase,"started":Double(now.tv_sec)+Double(now.tv_nsec)/1_000_000_000])}
    func sendAction(_ action:String,_ fields:[String:Any]=[:]){var value=fields;value["action"]=action;if action == "quick_dismiss"{var now=timespec();clock_gettime(CLOCK_MONOTONIC_RAW,&now);value["issuedAt"]=Double(now.tv_sec)+Double(now.tv_nsec)/1_000_000_000};if let data=try? JSONSerialization.data(withJSONObject:value){send(data)}}
    var voiceHost:NSView?{app?.unifiedVoiceHost()}
    func diagnostics()->UInt32{app?.unifiedPresentationDiagnostics() ?? 0}
    func toggle(){app?.toggleUnifiedSurface()}
    func collapse(){app?.foldWorkspace()}
    func quickSurface(_ window:NSWindow,show:Bool)->Bool{app?.transitionQuickSurface(window,show:show) ?? false}
}

#if KAI_UNIFIED
@_cdecl("kai_unified_wallpaper")
public func kaiUnifiedWallpaper(_ bytes:UnsafePointer<UInt8>?,_ count:Int,_ width:Int,_ height:Int,_ opacity:Float){guard Thread.isMainThread else{return};MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.receiveWallpaper(bytes,count:count,width:width,height:height,opacity:opacity)}}
@_cdecl("kai_unified_start")
public func kaiUnifiedStart(_ window:UnsafeMutableRawPointer?,_ command:(@convention(c)(UnsafePointer<UInt8>?,Int)->Bool)?) {
    guard Thread.isMainThread,let window,let command else{return}
    MainActor.assumeIsolated {
        EmbeddedCodeRuntime.shared.start(window:Unmanaged<NSWindow>.fromOpaque(window).takeUnretainedValue(),command:command)
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
