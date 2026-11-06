import AppKit

private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{true}
@MainActor private final class FocusAuditWindow:NSWindow {
 override func makeKeyAndOrderFront(_ sender:Any?){precondition(NSApp.isActive,"Morph requested GPUI key focus before application activation");super.makeKeyAndOrderFront(sender)}
}
private func member(_ object:Any,_ key:String)->Any?{guard let value=Mirror(reflecting:object).children.first(where:{$0.label == key})?.value else{return nil};let mirror=Mirror(reflecting:value);return mirror.displayStyle == .optional ? mirror.children.first?.value:value}

@main struct MorphMatrixAudit {
 @MainActor static func main()throws{
    _=NSApplication.shared
    UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded");UserDefaults.standard.set(false,forKey:"soundsEnabled");UserDefaults.standard.set(false,forKey:"speakResponses");UserDefaults.standard.set(false,forKey:"spotifyMedia");UserDefaults.standard.set(false,forKey:"youtubeMedia")
    let workspace=FocusAuditWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
    // GPUI's borderless mini window explicitly permits key focus. Plain
    // NSWindow refuses it and cannot represent foreground approval routing.
    let quick=NotchPanel(contentRect:CGRect(x:350,y:200,width:680,height:520),styleMask:[.borderless],backing:.buffered,defer:false)
    workspace.isReleasedWhenClosed=false;quick.isReleasedWhenClosed=false;quick.level = .floating
    let runtime=EmbeddedCodeRuntime.shared;var foreground=false,quickForeground=true
    runtime.workspaceFocusProvider={foreground}
    // Focus is supplied at the native boundary, as with the workspace fixture.
    // This exercises real routing even without an interactive WindowServer.
    runtime.windowFocusProvider={$0 === quick && quickForeground}
    runtime.start(window:workspace,command:command)
    let app=member(runtime,"app") as! KaiPetApp,panel=member(app,"panel") as! NSPanel,morph=member(app,"workspaceMorph") as! WorkspaceMorph,mini=member(app,"quickMorph") as! WorkspaceMorph
    defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
    let state:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:matrix","threadName":"Matrix","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"sessionActivity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","attachments":[],"selectedAgent":"codex","models":[],"ownerToken":"matrix","composerDraft":"Shared draft"]
    runtime.receive(try JSONSerialization.data(withJSONObject:state))
    func pump(_ seconds:Double){let end=Date().addingTimeInterval(seconds);while Date()<end{
        RunLoop.main.run(until:Date().addingTimeInterval(0.005))
        let backdrops=[morph,mini].compactMap{member($0,"shell") as? NSPanel}.filter(\.isVisible)
        precondition(backdrops.count<=1,"Mini Code/workspace handoff stacked two glass backdrops")
    }}
    func settle(){let end=Date().addingTimeInterval(3);repeat{pump(0.02)}while (morph.transitioning || mini.transitioning) && Date()<end;precondition(!morph.transitioning && !mini.transitioning,"A morph never completed")}
    func show(){app.prepareQuickOpening();precondition(app.transitionQuickSurface(quick,show:true));settle();precondition(quick.isVisible && !quick.ignoresMouseEvents,"Mini Code did not become interactive");precondition(!panel.isVisible,"Mini Code duplicated the notch");precondition(quick.level == .floating,"Transition level was not restored");precondition(app.unifiedVoiceHost() === quick.contentView,"Mini Code voice selected a hidden window");precondition((member(app,"workspacePet") as? NSView)?.superview === quick.contentView,"Mini Code lost the shared local pet")}
    func hide(){precondition(app.transitionQuickSurface(quick,show:false));settle();precondition(!quick.isVisible && !quick.ignoresMouseEvents,"Mini Code did not retire cleanly")}
    if member(app,"islandMode") as? Bool == true {
        app.showUnifiedNotchOnLaunch();pump(0.4)
        let screen=panel.screen!,mask=member(app,"mask") as! CAShapeLayer,sprite=member(app,"sprite") as! CALayer
        let drop=member(app,"dropPanel") as! NSPanel
        precondition(abs(screen.frame.maxY-panel.frame.maxY-12)<1,"Island lost its floating gap")
        precondition(sprite.contents != nil && sprite.bounds.width>0 && sprite.bounds.height>0,"Island pet has no drawable sprite")
        precondition(mask.path!.contains(sprite.position),"Pet is clipped outside the compact island")
        precondition(!drop.ignoresMouseEvents && abs(drop.frame.maxY-panel.frame.maxY)<1,"Compact island input is detached")
        let bounds=mask.path!.boundingBoxOfPath
        precondition(!mask.path!.contains(CGPoint(x:bounds.minX+1,y:bounds.maxY-1)),"Floating island still has attached notch shoulders")
        let root=panel.contentView!.layer!,size=root.bounds.size
        let rep=NSBitmapImageRep(bitmapDataPlanes:nil,pixelsWide:Int(size.width*2),pixelsHigh:Int(size.height*2),bitsPerSample:8,samplesPerPixel:4,hasAlpha:true,isPlanar:false,colorSpaceName:.deviceRGB,bytesPerRow:0,bitsPerPixel:0)!
        let graphics=NSGraphicsContext(bitmapImageRep:rep)!
        graphics.cgContext.scaleBy(x:2,y:2);root.render(in:graphics.cgContext)
        try rep.representation(using:.png,properties:[:])!.write(to:URL(fileURLWithPath:"/tmp/kai-island-compact.png"))
        print("PASS: floating gap, visible pet, rounded capsule, aligned compact input; rendered /tmp/kai-island-compact.png")
    }
    app.expandToRunningChat();pump(0.4)
    precondition(panel.isVisible && member(app,"miniOpen") as? Bool == true,"Island did not expand to chat")
    foreground=true;app.revealWorkspace();settle()
    precondition(workspace.isVisible && !panel.isVisible,"Island did not morph into workspace")
    app.foldWorkspace();settle()
    precondition(panel.isVisible && !workspace.isVisible && member(app,"miniOpen") as? Bool == true,"Workspace did not return to expanded island")
    show();hide()
    precondition(panel.isVisible && member(app,"miniOpen") as? Bool == true,"Mini app did not restore island")
    morph.motionEnabled=false;app.revealWorkspace();settle();app.foldWorkspace();settle()
    precondition(panel.isVisible && !workspace.isVisible,"Reduced motion lost island")
    print("PASS: island compact → expanded chat → full workspace → island; mini app round trip; reduced motion")
 }
}
