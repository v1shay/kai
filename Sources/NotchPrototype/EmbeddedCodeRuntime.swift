import AppKit

/// C ABI boundary for Code's GPUI owner. Both sides run on the AppKit thread.
/// No socket, helper process, transcript scan, or independent agent connection.
@MainActor final class EmbeddedCodeRuntime {
    static let shared=EmbeddedCodeRuntime()
    typealias Command = @convention(c) (UnsafePointer<UInt8>?,Int)->Bool
    private(set) var active=false
    private var command:Command?
    private var app:KaiPetApp?
    weak var workspaceWindow:NSWindow?
    var onLevels:(([CGFloat])->Void)?
    var onState:((KaiState)->Void)?
    var onError:((String)->Void)?
    private var lastState:Data?

    func start(window:NSWindow,command:@escaping Command) {
        guard !active else{return}
        active=true;self.command=command;workspaceWindow=window
        let defaults=UserDefaults.standard
        if !defaults.bool(forKey:"unifiedPreferencesSeeded") {
            for (key,value) in defaults.persistentDomain(forName:"com.kai.notchprototype") ?? [:] where defaults.object(forKey:key) == nil && key != "experimentalWebMode"{defaults.set(value,forKey:key)}
            defaults.set(true,forKey:"unifiedPreferencesSeeded")
        }
        let app=KaiPetApp();self.app=app
        app.applicationDidFinishLaunching(Notification(name:NSApplication.didFinishLaunchingNotification))
        NotificationCenter.default.addObserver(forName:NSApplication.willTerminateNotification,object:nil,queue:.main){[weak app] note in
            MainActor.assumeIsolated{app?.applicationWillTerminate(note)}
        }
    }
    func send(_ data:Data) {
        guard let command else{return}
        let accepted=data.withUnsafeBytes{raw in command(raw.bindMemory(to:UInt8.self).baseAddress,raw.count)}
        if !accepted{onError?("The workspace is busy; your input is preserved. Try again.")}
    }
    func receive(_ data:Data) {
        guard data != lastState else{return}
        if let object=try? JSONSerialization.jsonObject(with:data) as? [String:Any],object["type"] as? String == "workspaceReveal" {
            app?.revealWorkspace();return
        }
        guard let state=try? JSONDecoder().decode(KaiState.self,from:data) else{onError?("Could not read Code’s owner state");fputs("Kai unified state decode failed\n",stderr);return}
        lastState=data;onState?(state)
    }
    var ready:Bool{active && onState != nil && lastState != nil}
    func toggle(){app?.toggleUnifiedSurface()}
    func collapse(){app?.collapseWorkspace()}
}

#if KAI_UNIFIED
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
@_cdecl("kai_unified_levels")
public func kaiUnifiedLevels(_ values:UnsafePointer<Float>?,_ count:Int){
    guard Thread.isMainThread,let values,count == 4 else{return}
    let levels=(0..<4).map{CGFloat(values[$0].isFinite ? min(1,max(0,values[$0])):0)}
    MainActor.assumeIsolated{EmbeddedCodeRuntime.shared.onLevels?(levels)}
}
#endif
