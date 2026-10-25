// Thin Apple media shim adapted from the user's v1shay/kai WebRTCVoiceMedia.
// Rust owns the app, state and authenticated Codex signaling. No keys/tokens here.
import AppKit
import AVFoundation
import WebKit

private typealias EventCallback = @convention(c) (UnsafeMutableRawPointer?, UnsafePointer<CChar>?) -> Void
private final class Relay:NSObject,WKScriptMessageHandler {
    weak var owner:CodeVoiceMedia?
    func userContentController(_ controller:WKUserContentController,didReceive message:WKScriptMessage){
        MainActor.assumeIsolated{owner?.receive(message)}
    }
}
@MainActor private final class CodeVoiceMedia:NSObject,WKNavigationDelegate,WKUIDelegate {
    let callback:EventCallback
    let context:UnsafeMutableRawPointer?
    let script:String
    weak var host:NSView?
    var view:WKWebView?
    var initialPrompt=""
    var session:String?
    init(host:NSView,script:String,context:UnsafeMutableRawPointer?,callback:@escaping EventCallback){self.host=host;self.script=script;self.context=context;self.callback=callback}
    func post(_ fields:[String:Any]){
        guard let session else{return}
        var event=fields;event["sessionId"]=session
        guard let data=try? JSONSerialization.data(withJSONObject:event),data.count<=256*1024,let text=String(data:data,encoding:.utf8) else{return}
        text.withCString{callback(context,$0)}
    }
    func start(_ id:String){
        stop();session=id
        switch AVCaptureDevice.authorizationStatus(for:.audio){
        case .authorized:open(id)
        case .notDetermined:AVCaptureDevice.requestAccess(for:.audio){[weak self] allowed in
            DispatchQueue.main.async{guard let self,self.session==id else{return};if allowed{self.open(id)}else{self.fail("Allow microphone access for Code in System Settings to start voice.")}}
        }
        default:fail("Allow microphone access for Code in System Settings to start voice.")
        }
    }
    func open(_ id:String){
        guard session==id,let host else{fail("The conversation window closed before voice started.");return}
        let configuration=WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        configuration.mediaTypesRequiringUserActionForPlayback=[]
        let relay=Relay();relay.owner=self
        configuration.userContentController.add(relay,name:"codeVoice")
        configuration.userContentController.addUserScript(WKUserScript(source:script,injectionTime:.atDocumentEnd,forMainFrameOnly:true))
        let web=WKWebView(frame:CGRect(x:0,y:0,width:1,height:1),configuration:configuration)
        web.alphaValue=0.01;web.navigationDelegate=self;web.uiDelegate=self;view=web
        host.addSubview(web,positioned:.below,relativeTo:nil)
        web.loadHTMLString("<!doctype html><html><head><meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; script-src 'unsafe-inline'; media-src blob:; connect-src 'none'\"></head><body></body></html>",baseURL:URL(string:"http://localhost")!)
    }
    func invoke(_ method:String,_ values:[Any]){
        guard let view,let data=try? JSONSerialization.data(withJSONObject:values),let args=String(data:data,encoding:.utf8) else{return}
        let id=session
        view.evaluateJavaScript("window.codeVoice.\(method)(...\(args));void 0"){[weak self] _,error in
            guard let self,self.session==id else{return};if let error{self.fail(error.localizedDescription)}
        }
    }
    func command(_ event:[String:Any]){
        guard let id=session,event["sessionId"] as? String==id,let method=event["type"] as? String else{return}
        switch method{
        case "initialPrompt":if let audio=event["audio"] as? String,audio.utf8.count<=32*1024*1024{initialPrompt=audio}
        case "answer":if let sdp=event["sdp"] as? String,sdp.hasPrefix("v=0"),sdp.utf8.count<=128*1024{invoke("answer",[id,sdp])}
        case "activate":invoke("activate",[id])
        case "mute":invoke("mute",[id,event["muted"] as? Bool ?? false])
        case "interrupt":invoke("interrupt",[id])
        default:break
        }
    }
    func stop(){
        session=nil
        if let view{
            view.evaluateJavaScript("window.codeVoice?.stop()",completionHandler:nil)
            view.configuration.userContentController.removeScriptMessageHandler(forName:"codeVoice")
            view.stopLoading();view.uiDelegate=nil;view.navigationDelegate=nil;view.removeFromSuperview()
            view.loadHTMLString("",baseURL:nil)
        }
        view=nil
    }
    func fail(_ message:String){post(["type":"error","message":String(message.prefix(1000))]);stop()}
    func receive(_ message:WKScriptMessage){
        guard message.webView===view,message.frameInfo.isMainFrame,let event=message.body as? [String:Any],let type=event["type"] as? String,let id=session else{return}
        if type=="ready"{let prompt=initialPrompt;initialPrompt="";invoke("start",[id,true,prompt,false]);return}
        guard event["sessionId"] as? String==id else{return}
        switch type{
        case "offer":if let sdp=event["sdp"] as? String,sdp.hasPrefix("v=0"),sdp.utf8.count<=128*1024{post(["type":"offer","sdp":sdp])}
        case "connected":post(["type":"connected"])
        case "activity":if let activity=event["activity"] as? String,["listening","thinking","speaking"].contains(activity){post(["type":"activity","activity":activity])}
        case "level":if let level=event["level"] as? Double,level.isFinite{post(["type":"level","level":max(0,min(1,level))])}
        case "error":fail(event["message"] as? String ?? "The Codex voice media connection failed.")
        default:break
        }
    }
    nonisolated func webView(_ webView:WKWebView,decidePolicyFor action:WKNavigationAction,decisionHandler:@escaping(WKNavigationActionPolicy)->Void){
        MainActor.assumeIsolated{let url=action.request.url;let local=url?.scheme=="http" && url?.host=="localhost" && url?.port==nil && ["","/"].contains(url?.path ?? "") && url?.query==nil;decisionHandler(webView===view && (url?.absoluteString=="about:blank" || local) ? .allow:.cancel)}
    }
    nonisolated func webView(_ webView:WKWebView,requestMediaCapturePermissionFor origin:WKSecurityOrigin,initiatedByFrame frame:WKFrameInfo,type:WKMediaCaptureType,decisionHandler:@escaping(WKPermissionDecision)->Void){
        MainActor.assumeIsolated{let local=(origin.host=="localhost" && origin.protocol=="http") || (origin.host.isEmpty && origin.protocol=="about");decisionHandler(webView===view && session != nil && frame.isMainFrame && local && type == .microphone ? .grant:.deny)}
    }
    nonisolated func webViewWebContentProcessDidTerminate(_ webView:WKWebView){MainActor.assumeIsolated{if webView===view{fail("Codex voice media stopped. Start voice again to reconnect.")}}}
    nonisolated func webView(_ webView:WKWebView,didFailProvisionalNavigation navigation:WKNavigation!,withError error:Error){MainActor.assumeIsolated{if webView===view{fail(error.localizedDescription)}}}
}

@_cdecl("code_voice_create")
public func codeVoiceCreate(_ host:UnsafeMutableRawPointer?,_ source:UnsafePointer<CChar>?,_ context:UnsafeMutableRawPointer?,_ callback:@escaping @convention(c)(UnsafeMutableRawPointer?,UnsafePointer<CChar>?)->Void)->UnsafeMutableRawPointer?{
    guard Thread.isMainThread,let host,let source else{return nil}
    return MainActor.assumeIsolated{
        let view=Unmanaged<NSView>.fromOpaque(host).takeUnretainedValue()
        return Unmanaged.passRetained(CodeVoiceMedia(host:view,script:String(cString:source),context:context,callback:callback)).toOpaque()
    }
}
@_cdecl("code_voice_start")
public func codeVoiceStart(_ opaque:UnsafeMutableRawPointer?,_ id:UnsafePointer<CChar>?){guard Thread.isMainThread,let opaque,let id else{return};MainActor.assumeIsolated{Unmanaged<CodeVoiceMedia>.fromOpaque(opaque).takeUnretainedValue().start(String(cString:id))}}
@_cdecl("code_voice_command")
public func codeVoiceCommand(_ opaque:UnsafeMutableRawPointer?,_ text:UnsafePointer<CChar>?){guard Thread.isMainThread,let opaque,let text else{return};MainActor.assumeIsolated{guard let data=String(cString:text).data(using:.utf8),let event=(try? JSONSerialization.jsonObject(with:data)) as? [String:Any] else{return};Unmanaged<CodeVoiceMedia>.fromOpaque(opaque).takeUnretainedValue().command(event)}}
@_cdecl("code_voice_destroy")
public func codeVoiceDestroy(_ opaque:UnsafeMutableRawPointer?){guard Thread.isMainThread,let opaque else{return};MainActor.assumeIsolated{let media=Unmanaged<CodeVoiceMedia>.fromOpaque(opaque).takeRetainedValue();media.stop()}}
