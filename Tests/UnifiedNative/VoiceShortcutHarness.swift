import AppKit

@MainActor final class HeldRecorder:VoicePromptRecording {
    var onLevels:(([CGFloat])->Void)?,onError:((String)->Void)?
    var audio:Data?,starts=0,stops=0
    func start(){starts+=1;onLevels?([0.3,0.5,0.7,0.4])}
    func stop()->Data?{stops+=1;return audio}
    func cancel(){}
}
private func member(_ object:Any,_ name:String)->Any?{
    guard let value=Mirror(reflecting:object).children.first(where:{$0.label == name})?.value else{return nil}
    let mirror=Mirror(reflecting:value)
    return mirror.displayStyle == .optional ? mirror.children.first?.value:value
}
private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
    guard let bytes,let fields=try? JSONSerialization.jsonObject(with:Data(bytes:bytes,count:count)) as? [String:Any] else{return false}
    MainActor.assumeIsolated{VoiceShortcutHarness.commands.append(fields);VoiceShortcutHarness.onCommand?(fields)};return true
}
@main struct VoiceShortcutHarness {
    @MainActor static var commands=[[String:Any]]()
    @MainActor static var onCommand:(([String:Any])->Void)?
    @MainActor static func main()throws{
        _=NSApplication.shared
        let suite="VoiceShortcutHarness-"+UUID().uuidString,preferences=UserDefaults(suiteName:suite)!
        defer{preferences.removePersistentDomain(forName:suite)}
        let defaults=UserDefaults.standard
        for (key,value) in ["unifiedPreferencesSeeded":true,"soundsEnabled":false,"speakResponses":false,"youtubeMedia":false,"spotifyMedia":false]{defaults.set(value,forKey:key)}
        let recorder=HeldRecorder();var held=NSEvent.ModifierFlags(),captureCount=0
        let app=KaiPetApp(voiceRecorder:recorder,shortcutDefaults:preferences,modifierFlags:{held},capture:{_,done in captureCount+=1;done(ApplicationContext(text:"Captured fixture editor",imagePath:nil))})
        let runtime=EmbeddedCodeRuntime.shared;runtime.makeApp={app};runtime.workspaceFocusProvider={false}
        let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false);window.isReleasedWhenClosed=false
        runtime.start(window:window,command:command)
        defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
        var thread="code:existing",live=false,request:String?,scene="off",revision=0
        func publish(){
            var state:[String:Any] = ["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":thread,"threadName":"Selected voice chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Voice fixture \(revision)","attachments":[],"composerDraft":"","ownerToken":"fixture-\(revision)","workspaceVoiceActive":live,"workspaceVoiceScene":scene,"selectedAgent":"codex","models":[]]
            if let request{state["completedRequestId"]=request}
            runtime.receive(try! JSONSerialization.data(withJSONObject:state))
        }
        onCommand={fields in
            request=fields["requestId"] as? String;revision+=1
            switch fields["action"] as? String{
            case "new":thread="code:new-\(revision)";publish()
            case "workspace_voice_prompt":live=true;scene="thinking";publish()
            case "voice_stop":live=false;scene="off";publish()
            default:break
            }
        }
        func pump(_ seconds:Double){RunLoop.current.run(until:Date().addingTimeInterval(seconds))}
        func modifiers(_ flags:NSEvent.ModifierFlags){held=flags;let raw=CGEvent(keyboardEventSource:nil,virtualKey:55,keyDown:!flags.isEmpty)!;raw.type = .flagsChanged;raw.flags=CGEventFlags(rawValue:UInt64(flags.rawValue));let event=NSEvent(cgEvent:raw)!;app.flags(event)}
        let prefs=ShortcutPreferences(defaults:preferences)
        for trigger in DictationTrigger.allCases{prefs.setHoldDuration(0.1,for:trigger)}
        for trigger in DictationTrigger.allCases{
        let flag:NSEvent.ModifierFlags=trigger == .command ? .command:trigger == .option ? .option:.function
        for action in ShortcutAction.allCases where action.isVoice{
            live=false;thread="code:existing";scene="off";request=nil;publish();app.dismissUnifiedSurface();pump(0.3)
            prefs.set(action,for:trigger);let before=commands.count;recorder.audio=nil
            let starts=recorder.starts;modifiers(flag)
            precondition(recorder.starts==starts+1,"Audio before the hold threshold was dropped")
            pump(0.15)
            precondition(runtime.voiceHost?.window?.isVisible == true,"Held voice did not supply the live notch media host")
            precondition(recorder.starts>0,"Configured hold never armed microphone")
            modifiers([]);pump(0.1)
            precondition(!commands.dropFirst(before).contains{["new","chat","workspace_voice_prompt"].contains($0["action"] as? String ?? "")},"Silent release created a chat or voice session")
            precondition(!live,"Silent recording opened realtime voice")
            // An actual audible PCM fixture crosses the same App/bridge path.
            let buffer=VoicePromptBuffer();let samples=[Float](repeating:0.05,count:8000);_=samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:24000)};let audio=buffer.wave()!
            recorder.audio=audio;let beforeSpeech=commands.count,beforeCapture=captureCount
            pump(0.4);modifiers(flag);pump(0.15)
            precondition(!commands.dropFirst(beforeSpeech).contains{$0["action"] as? String == "workspace_voice_prompt"},"Voice started before key release")
            modifiers([]);pump(0.1)
            let outgoing=commands.dropFirst(beforeSpeech)
            guard let voice=outgoing.last(where:{$0["action"] as? String == "workspace_voice_prompt"}) else{fatalError("Production modifier handler did not start voice for \(action)")}
            precondition(Data(base64Encoded:voice["audio"] as! String)==audio,"Recorded prompt was lost")
            precondition(voice["targetThreadId"] as? String == thread,"Voice targeted another chat")
            precondition(action.createsChat ? thread.hasPrefix("code:new-"):thread=="code:existing")
            precondition((voice["appshot"] as? [String:Any])?["text"] as? String == (action.capturesContext ? "Captured fixture editor":nil))
            precondition(captureCount-beforeCapture == (action.capturesContext ? 1:0))
            scene="listening";publish();pump(0.03)
            precondition(member(app,"compactVoiceMode") as? Bool == true,"The native call lost shortcut indicator ownership")
            precondition(member(app,"open") as? Bool == true && member(app,"miniOpen") as? Bool == false,"Voice did not retain the compact listening surface")
            scene="speaking";publish();pump(0.3)
            precondition((member(app,"indicator") as? NotchIndicatorEngine)?.mode == .listening,"Speaking lost the original audio waveform")
            // Holding the assigned voice shortcut again stops the same call.
            pump(0.4);modifiers(flag);pump(0.15);modifiers([]);pump(0.05)
            precondition(!live,"Original stop shortcut did not stop the native call")
        }
        }
        // Ordinary Command-key combinations cancel pre-roll without closing an
        // already extended chat. This is the actual keyDown monitor's handler.
        live=false;scene="off";thread="code:existing";request=nil;publish();pump(0.4)
        prefs.set(.realtimeVoice,for:.command);app.expandToRunningChat();pump(0.3)
        modifiers(.command);app.cancelVoicePromptForKeypress();modifiers([]);pump(0.1)
        precondition(member(app,"miniOpen") as? Bool == true,"A normal Command-key combination closed the chat")
        pump(0.4);modifiers(.command);modifiers([.command,.shift]);modifiers([]);pump(0.1)
        precondition(member(app,"miniOpen") as? Bool == true,"A multi-modifier keyboard shortcut closed the chat")
        app.dismissUnifiedSurface();pump(0.3)
        // A short modifier tap must leave a foreground workspace intact and
        // must not send provider commands or activate its notch overlay.
        live=false;scene="off";thread="code:existing";request=nil;publish();pump(0.4)
        runtime.workspaceFocusProvider={true};app.revealWorkspace();pump(1)
        precondition(runtime.voiceHost?.window === window,"Expanded voice did not select the live Code host")
        prefs.set(.realtimeVoice,for:.command);let tapCommands=commands.count,frame=window.frame
        modifiers(.command);pump(0.02);modifiers([]);pump(0.15)
        precondition(window.isVisible && window.frame == frame,"A short modifier tap disturbed the workspace")
        precondition(member(app,"workspaceNotchOverlay") as? Bool == false,"A short tap opened the notch over Code")
        precondition(!commands.dropFirst(tapCommands).contains{$0["action"] as? String == "workspace_voice_prompt"},"A short tap started voice")
        // The same real modifier handler must still dismiss the big form.
        pump(0.4);modifiers(.command);pump(0.02);modifiers([]);pump(0.03);modifiers(.command);pump(0.02);modifiers([]);pump(1)
        precondition(!window.isVisible,"Double Command no longer dismissed the big form")
        runtime.workspaceFocusProvider={false}
        // The real Control–Option hold keeps the original compact -> mini
        // shape animation, including owner updates and a same-size layout pass.
        app.dismissUnifiedSurface();pump(0.4)
        modifiers([.control,.option]);pump(1.06)
        let shortcutMask=member(app,"mask") as! CAShapeLayer
        guard let opening=shortcutMask.animation(forKey:"width") as? CABasicAnimation else{fatalError("Control–Option did not animate the notch")}
        let openingStart=opening.fromValue as! CGPath,openingEnd=opening.toValue as! CGPath
        precondition(openingStart.boundingBoxOfPath.height<openingEnd.boundingBoxOfPath.height && opening.duration==0.28,"Shortcut lost the original expanding notch morph")
        NotificationCenter.default.post(name:NSApplication.didChangeScreenParametersNotification,object:NSApp)
        publish();pump(0.02)
        guard let continued=shortcutMask.animation(forKey:"width") as? CABasicAnimation else{fatalError("A layout/owner refresh snapped the opening notch to its final size")}
        precondition(continued.beginTime==opening.beginTime && continued.duration==opening.duration,"Refreshing restarted the shortcut morph")
        modifiers([]);pump(0.3);app.dismissUnifiedSurface();pump(0.3)
        // Release at/after the deadline even when the AppKit timer could not
        // run. The pending audio must own the surface before recorder reset.
        live=false;scene="off";thread="code:existing";request=nil;publish();app.dismissUnifiedSurface();pump(0.4)
        prefs.set(.realtimeVoice,for:.command);recorder.audio=Data([1,2,3]);let boundary=commands.count
        modifiers(.command);Thread.sleep(forTimeInterval:0.15);modifiers([]);pump(0.03)
        precondition(commands.dropFirst(boundary).contains{$0["action"] as? String == "workspace_voice_prompt"},"A delayed arm timer lost audio released at the deadline")
        app.dismissUnifiedSurface();pump(0.05)
        print("Production shortcut handler: all voice actions on Command/Option/Fn retain pre-roll, discard silence, preserve audio/chat/context and listening indicators, stop via hold, and preserve tap/double-Command workspace behavior")
    }
}
