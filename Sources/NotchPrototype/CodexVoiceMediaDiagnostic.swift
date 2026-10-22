import AppKit

/// Opt-in transport test. Output stays muted; held-prompt mode never enables its
/// hardware microphone track and sends only the generated fixture.
@MainActor enum CodexVoiceMediaDiagnostic {
    static func run(heldPrompt:Bool=false){
        let app=NSApplication.shared;app.setActivationPolicy(.regular)
        let window=NSWindow(contentRect:CGRect(x:200,y:300,width:340,height:120),styleMask:[.titled,.closable],backing:.buffered,defer:false)
        window.title=heldPrompt ? "Kai held-prompt test — live microphone stays muted":"Kai voice test — no microphone"
        let media=WebRTCVoiceMedia();media.hostView=window.contentView
        media.diagnosticPromptOnly=heldPrompt
        func emit(_ value:[String:Any]){
            guard let data=try? JSONSerialization.data(withJSONObject:value) else{return}
            try? FileHandle.standardOutput.write(contentsOf:data+Data([10]))
        }
        media.onOffer={emit(["type":"offer","sdp":$0])}
        media.onConnected={emit(["type":"connected"]);if heldPrompt{media.activate()}}
        media.onDiagnosticAudio={emit(["type":"audioStats","bytes":$0])}
        media.onDiagnosticEvent={emit(["type":"event","eventType":$0])}
        media.onError={emit(["type":"error","message":$0]);app.terminate(nil)}
        Thread.detachNewThread {
            while let line=readLine(){
                guard let data=line.data(using:.utf8),let value=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any] else{continue}
                DispatchQueue.main.async {
                    if let sdp=value["sdp"] as? String{media.acceptAnswer(sdp)}
                    if let audio=value["testAudio"] as? String{media.injectDiagnosticAudio(audio)}
                    if heldPrompt,let encoded=value["initialPrompt"] as? String,let audio=Data(base64Encoded:encoded){media.start(sessionID:"kai-media-diagnostic",captureMicrophone:true,initialPrompt:audio)}
                    if value["type"] as? String == "stop"{media.stop();app.terminate(nil)}
                }
            }
            DispatchQueue.main.async{media.stop();app.terminate(nil)}
        }
        if !heldPrompt{media.start(sessionID:"kai-media-diagnostic",captureMicrophone:false)}
        DispatchQueue.main.asyncAfter(deadline:.now()+60){media.stop();emit(["type":"error","message":"Voice media diagnostic timed out"]);app.terminate(nil)}
        window.makeKeyAndOrderFront(nil);app.activate(ignoringOtherApps:true)
        app.run();media.stop();window.orderOut(nil)
    }
}
