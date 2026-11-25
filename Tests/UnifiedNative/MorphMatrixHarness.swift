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
    show()
    var approval=state;approval["approval"]=true;approval["approvalThreadId"]="code:background";approval["approvalRequestId"]="matrix:approval";approval["approvalSummary"]="Fixture approval"
    runtime.receive(try JSONSerialization.data(withJSONObject:approval));pump(0.05)
    precondition(member(app,"approvalPopout") as? Bool == false,"Foreground mini Code duplicated approval in the notch")
    quickForeground=false;app.workspaceFocusChanged();pump(0.05)
    precondition(member(app,"approvalPopout") as? Bool == true,"Background mini Code did not surface approval in the notch")
    quickForeground=true;app.workspaceFocusChanged()
    precondition(member(app,"approvalPopout") as? Bool == false,"Returning to mini Code did not retire the duplicate notch approval")
    runtime.receive(try JSONSerialization.data(withJSONObject:state));hide();precondition(panel.isVisible && member(app,"miniOpen") as? Bool == false,"Compact notch return was lost")
    app.expandToRunningChat();pump(0.35);show();hide();precondition(panel.isVisible && member(app,"miniOpen") as? Bool == true,"Extended notch return was lost")
    foreground=true;app.revealWorkspace();settle();show();precondition(!workspace.isVisible,"Foreground workspace did not morph into mini Code");hide();precondition(workspace.isVisible && !panel.isVisible,"Mini Code did not return to the workspace")
    app.collapseWorkspace();settle();show();app.toggleUnifiedSurface();hide();precondition(panel.isVisible && member(app,"miniOpen") as? Bool == true,"Control-Option did not return mini Code directly to the extended notch")
    show();app.revealWorkspace();hide();precondition(workspace.isVisible && !panel.isVisible,"Mini-to-workspace handoff lost its destination")
    show();app.revealWorkspace();_ = app.transitionQuickSurface(quick,show:false);pump(0.025);app.prepareQuickOpening();_ = app.transitionQuickSurface(quick,show:true);settle();pump(0.7);precondition(quick.isVisible && !workspace.isVisible,"Interrupted workspace handoff ordered the old surface over mini Code");hide();precondition(workspace.isVisible,"Reversing a workspace handoff lost its return destination")
    app.collapseWorkspace();settle();show();app.dismissUnifiedSurface();hide();precondition(member(app,"open") as? Bool == false && !panel.isVisible,"Double-Command did not dismiss mini Code to the physical notch")
    app.expandToRunningChat();pump(0.35)
    for _ in 0..<8{_ = app.transitionQuickSurface(quick,show:true);pump(0.025);_ = app.transitionQuickSurface(quick,show:false);pump(0.025)}
    _ = app.transitionQuickSurface(quick,show:true);settle();pump(0.7);precondition(quick.isVisible && !panel.isVisible && !quick.ignoresMouseEvents,"A stale interrupted completion hid the current surface")
    hide();precondition(panel.isVisible,"Interrupted morph forgot its return surface")
    morph.motionEnabled=false;show();hide();precondition(quick.alphaValue == 1 && !quick.ignoresMouseEvents,"Reduced motion left the window invisible or blocked")
    morph.motionEnabled=true;foreground=true;app.revealWorkspace();settle();foreground=false;app.workspaceFocusChanged();show();precondition(!workspace.isVisible,"Mini Code left another interactive workspace visible");hide();precondition(!workspace.isVisible,"Dismissing mini Code unexpectedly reopened a background workspace")
    // Code's explicit fold must not become a background overlay when native
    // focus lags a reveal or changes before the owner request is dispatched.
    app.revealWorkspace();pump(0.025);runtime.collapse();settle()
    precondition(!workspace.isVisible && panel.isVisible && member(app,"miniOpen") as? Bool == true,"Explicit fold during reveal became a background overlay")
    app.revealWorkspace();settle();runtime.collapse();settle()
    precondition(!workspace.isVisible && panel.isVisible && member(app,"miniOpen") as? Bool == true,"Explicit fold depended on WindowServer focus")
    foreground=true;app.revealWorkspace();settle();app.foldWorkspace();pump(0.20)
    precondition(workspace.isVisible && !morph.expanded,"Fixture did not catch the retiring Code window")
    show();hide();precondition(!workspace.isVisible && panel.isVisible && member(app,"miniOpen") as? Bool == true,"Mini Code reopened a workspace that was already folding to the notch")
    precondition((member(app,"latestState") as? KaiState)?.threadId == "code:matrix","A presentation transition navigated the conversation")
    print("Native morph matrix passed: compact/extended/workspace ↔ mini Code, direct Control-Option, double-Command, rapid reversals, reduced motion, background retention, shared conversation, restored input and window levels")
 }
}
