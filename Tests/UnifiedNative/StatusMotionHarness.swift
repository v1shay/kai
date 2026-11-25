import AppKit

private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool { true }
private func member(_ object:Any,_ name:String)->Any? {
    guard let value=Mirror(reflecting:object).children.first(where:{$0.label==name})?.value else{return nil}
    let mirror=Mirror(reflecting:value)
    return mirror.displayStyle == .optional ? mirror.children.first?.value:value
}
@main struct StatusMotionAudit {
    @MainActor static func main() throws {
        _=NSApplication.shared
        for key in ["soundsEnabled","speakResponses","youtubeMedia","spotifyMedia","floatingTasks.enabled"] {UserDefaults.standard.set(false,forKey:key)}
        UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded")
        let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
        window.isReleasedWhenClosed=false
        window.backgroundColor = .systemBlue
        let runtime=EmbeddedCodeRuntime.shared
        var foreground=true
        runtime.workspaceFocusProvider={foreground}
        runtime.start(window:window,command:command,presentInitialNotch:false)
        let app=member(runtime,"app") as! KaiPetApp
        defer {app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));window.orderOut(nil)}
        let state:[String:Any] = ["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:motion","threadName":"Motion fixture","history":[],"readOnly":false,"approval":false,"activity":["scene":"thinking","text":"Working"],"sessionActivity":["scene":"thinking","text":"Working"],"activeTasks":[["id":"code:motion","name":"Motion fixture","runId":"run","scene":"thinking","text":"Working","agent":"codex"]],"activeTurnId":"run","notice":"Working","attachments":[],"selectedAgent":"codex","models":[],"ownerToken":"motion","composerDraft":"Preserved motion draft"]
        runtime.receive(try JSONSerialization.data(withJSONObject:state))
        app.launchUnifiedWorkspace()
        let panel=member(app,"panel") as! NSPanel
        let mask=member(app,"mask") as! CAShapeLayer
        let morph=member(app,"workspaceMorph") as! WorkspaceMorph
        let status=member(app,"workspaceStatusMorph") as! WorkspaceStatusMorph
        let retained=window.frame,number=window.windowNumber
        func pump(_ seconds:Double) {let end=Date().addingTimeInterval(seconds);while Date()<end {RunLoop.main.run(until:Date().addingTimeInterval(0.003))}}
        func leave() {foreground=false;app.workspaceFocusChanged()}
        func enter() {foreground=true;app.workspaceFocusChanged()}
        for duration in [0.16,0.22,0.30] {
            KaiMotion.paneDuration=duration
            leave()
            precondition(panel.isVisible && !panel.isKeyWindow && panel.ignoresMouseEvents,"Status stole focus or failed to show")
            precondition(!status.active && !morph.transitioning,"App switch animated the full workspace")
            let opening=mask.animation(forKey:"width") as! CABasicAnimation
            precondition(abs(opening.duration-duration)<0.0001,"Notch ignored motion profile")
            precondition(opening.preferredFrameRateRange.preferred==KaiMotion.displayFrameRate,"Notch did not request maximum display refresh")
            let from=opening.fromValue as! CGPath,to=opening.toValue as! CGPath
            precondition(from.boundingBoxOfPath.width<to.boundingBoxOfPath.width,"Notch reused a stale expanded source")
            pump(duration+0.03)
            if duration==0.22,let output=ProcessInfo.processInfo.environment["KAI_MOTION_EVIDENCE"] {
                let task=Process();task.executableURL=URL(fileURLWithPath:"/usr/sbin/screencapture")
                task.arguments=["-x","-l",String(panel.windowNumber),output+"/status.png"]
                try task.run();task.waitUntilExit()
            }
            enter()
            precondition(status.active && panel.isVisible && !morph.transitioning,"Return skipped local retraction")
            let closing=mask.animation(forKey:"width") as! CABasicAnimation
            precondition((closing.fromValue as! CGPath).boundingBoxOfPath.width>(closing.toValue as! CGPath).boundingBoxOfPath.width,"Return did not retract")
            // Real agents continue publishing while focus and animation settle.
            // Every packet used to reopen the notch and consume the return edge.
            for _ in 0..<8 {
                runtime.receive(try JSONSerialization.data(withJSONObject:state))
                precondition(member(app,"open") as? Bool == false,"Progress reopened the retracting notch")
                pump(duration/8)
            }
            pump(0.04)
            runtime.receive(try JSONSerialization.data(withJSONObject:state))
            precondition(!status.active && !panel.isVisible && window.isVisible,"Return left a duplicate or hid workspace")
            precondition(member(app,"open") as? Bool == false,"Progress resurrected foreground status after return")
        }
        KaiMotion.paneDuration=0.22
        for _ in 0..<12 {leave();pump(0.025);enter();pump(0.025)}
        leave();pump(0.35)
        precondition(panel.isVisible && member(app,"open") as? Bool==true,"Stale return hid reversed reveal")
        enter();pump(0.35)
        precondition(!panel.isVisible,"Rapid reversals stranded notch")
        precondition(window.frame==retained && window.windowNumber==number,"Status changed workspace geometry or identity")
        precondition((member(app,"latestState") as? KaiState)?.threadId=="code:motion","Status switched chats")
        precondition((member(app,"composerField") as? NSTextField)?.stringValue=="Preserved motion draft","Status lost draft")
        let group=CAAnimationGroup();group.animations=[CABasicAnimation(keyPath:"opacity")]
        KaiMotion.configure(group)
        precondition(group.preferredFrameRateRange.preferred==KaiMotion.displayFrameRate && group.animations!.allSatisfy{$0.preferredFrameRateRange.preferred==KaiMotion.displayFrameRate},"Nested animations missed frame policy")
        print("STATUS_MOTION_OK: three motion profiles, maximum refresh hints, local reveal/retract, twelve rapid reversals, progress during and after retraction, retained workspace and draft")
    }
}
