import AppKit
import AVFoundation
@MainActor private var media:UnsafeMutableRawPointer?
private func event(_ context:UnsafeMutableRawPointer?,_ text:UnsafePointer<CChar>?){
    guard let text,let data=String(cString:text).data(using:.utf8),let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any] else{return}
    try? FileHandle.standardOutput.write(contentsOf:data+Data([10]))
    if value["type"] as? String == "connected"{MainActor.assumeIsolated{if let media{let packet="{\"type\":\"activate\",\"sessionId\":\"\(value["sessionId"] as! String)\"}";packet.withCString{codeVoiceCommand(media,$0)}}}}
}
@main struct CodeMediaDiagnostic{
 @MainActor static func main()throws{
    guard CGDisplayIsAsleep(CGMainDisplayID()) == 0 else{
        print("{\"type\":\"error\",\"message\":\"Wake and unlock the Mac before testing live WebKit voice capture\"}");return
    }
    let app=NSApplication.shared;app.setActivationPolicy(.accessory)
    let window=NSWindow(contentRect:CGRect(x:100,y:100,width:800,height:600),styleMask:[.titled],backing:.buffered,defer:false);window.isReleasedWhenClosed=false;window.orderOut(nil)
    let screen=NSScreen.main!.visibleFrame
    let notch=NSPanel(contentRect:CGRect(x:screen.midX-220,y:screen.maxY-36,width:440,height:36),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
    notch.isReleasedWhenClosed=false;notch.hidesOnDeactivate=false;notch.level = .statusBar;notch.backgroundColor = .black;notch.orderFrontRegardless()
    let script=try String(contentsOfFile:ProcessInfo.processInfo.environment["KAI_VOICE_SCRIPT"]!)
    media=script.withCString{codeVoiceCreate(Unmanaged.passUnretained(notch.contentView!).toOpaque(),$0,nil,event)}
    let id=UUID().uuidString
    Thread.detachNewThread{while let line=readLine(){guard let data=line.data(using:.utf8),var value=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any] else{continue};DispatchQueue.main.async{
        guard let media else{return}
        if value["type"] as? String == "stop"{codeVoiceDestroy(media);CodeMediaDiagnostic.finish();return}
        if let audio=value["initialPrompt"] as? String{id.withCString{codeVoiceStart(media,$0)};value=["type":"initialPrompt","audio":audio]}
        if let sdp=value["sdp"] as? String{value=["type":"answer","sdp":sdp]}
        value["sessionId"]=id;if let bytes=try? JSONSerialization.data(withJSONObject:value),let text=String(data:bytes,encoding:.utf8){text.withCString{codeVoiceCommand(media,$0)}}
    }}}
    DispatchQueue.main.asyncAfter(deadline:.now()+100){if let media{codeVoiceDestroy(media)};finish()}
    app.run()
 }
 @MainActor static func finish(){media=nil;NSApp.stop(nil);let wake=NSEvent.otherEvent(with:.applicationDefined,location:.zero,modifierFlags:[],timestamp:0,windowNumber:0,context:nil,subtype:0,data1:0,data2:0)!;NSApp.postEvent(wake,atStart:false)}
}
