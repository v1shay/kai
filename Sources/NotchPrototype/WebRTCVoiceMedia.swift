import AppKit
import WebKit

@MainActor protocol VoiceMedia:AnyObject {
    var onOffer:((String)->Void)? {get set}
    var onConnected:(()->Void)? {get set}
    var onError:((String)->Void)? {get set}
    var onLevels:(([CGFloat])->Void)? {get set}
    var onActivity:((VoiceActivity)->Void)? {get set}
    func start(sessionID:String,captureMicrophone:Bool,initialPrompt:Data?)
    func acceptAnswer(_ sdp:String)
    func activate()
    func stop()
}
private final class VoiceMessageRelay:NSObject,WKScriptMessageHandler {
    weak var owner:WebRTCVoiceMedia?
    func userContentController(_ controller:WKUserContentController,didReceive message:WKScriptMessage){
        MainActor.assumeIsolated{owner?.receive(message)}
    }
}

/// A private WebKit WebRTC peer, embedded in Kai. No auth tokens enter this view.
@MainActor final class WebRTCVoiceMedia:NSObject,VoiceMedia,WKNavigationDelegate,WKUIDelegate {
    var onOffer:((String)->Void)?,onConnected:(()->Void)?,onError:((String)->Void)?,onLevels:(([CGFloat])->Void)?
    var onActivity:((VoiceActivity)->Void)?
    var onDiagnosticAudio:((Int)->Void)?
    var onDiagnosticEvent:((String)->Void)?
    var diagnosticPromptOnly=false
    weak var hostView:NSView?
    private var view:WKWebView?,sessionID:String?,captureMicrophone=true
    private var initialPrompt:Data?
    private let origin=URL(string:"http://localhost")!

    static var scriptURL:URL {
        if let resource=Bundle.main.resourceURL?.appendingPathComponent("voice/session.js"),FileManager.default.fileExists(atPath:resource.path){return resource}
        return URL(fileURLWithPath:#filePath).deletingLastPathComponent().appendingPathComponent("Resources/voice/session.js")
    }
    func start(sessionID:String,captureMicrophone:Bool=true,initialPrompt:Data?=nil){
        stop();self.sessionID=sessionID;self.captureMicrophone=captureMicrophone;self.initialPrompt=initialPrompt
        do {
            let script=try String(contentsOf:Self.scriptURL,encoding:.utf8)
            let configuration=WKWebViewConfiguration()
            configuration.websiteDataStore = .nonPersistent()
            configuration.mediaTypesRequiringUserActionForPlayback=[]
            let relay=VoiceMessageRelay();relay.owner=self
            configuration.userContentController.add(relay,name:"kaiVoice")
            configuration.userContentController.addUserScript(WKUserScript(source:script,injectionTime:.atDocumentEnd,forMainFrameOnly:true))
            let view=WKWebView(frame:CGRect(x:0,y:0,width:1,height:1),configuration:configuration)
            self.view=view;view.navigationDelegate=self;view.uiDelegate=self
            if let hostView{view.alphaValue=captureMicrophone ? 0.01:1;if !captureMicrophone{view.frame=hostView.bounds};hostView.addSubview(view,positioned:.below,relativeTo:nil)}
            view.loadHTMLString("<!doctype html><html><head><meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; script-src 'unsafe-inline'; media-src blob:; connect-src 'none'\"></head><body></body></html>",baseURL:origin)
        }catch{fail("Could not load Codex voice media: \(error.localizedDescription)")}
    }
    private func invoke(_ method:String,_ values:[Any]=[]){
        guard let view,let data=try? JSONSerialization.data(withJSONObject:values),let args=String(data:data,encoding:.utf8) else{return}
        let id=sessionID
        view.evaluateJavaScript("window.kaiVoice.\(method)(...\(args));void 0"){[weak self] _,error in
            guard let self,self.sessionID == id else{return}
            if let error{self.fail(error.localizedDescription)}
        }
    }
    func acceptAnswer(_ sdp:String){guard let sessionID,!sdp.isEmpty,sdp.utf8.count <= 128*1024 else{return};invoke("answer",[sessionID,sdp])}
    func activate(){guard let sessionID else{return};invoke("activate",[sessionID])}
    func injectDiagnosticAudio(_ base64:String){
        guard !captureMicrophone,let sessionID,base64.utf8.count <= 2*1024*1024 else{return}
        invoke("diagnosticAudio",[sessionID,base64])
    }
    func stop(){
        sessionID=nil;initialPrompt=nil
        if let view {
            view.evaluateJavaScript("window.kaiVoice?.stop()",completionHandler:nil)
            view.configuration.userContentController.removeScriptMessageHandler(forName:"kaiVoice")
            view.stopLoading();view.uiDelegate=nil;view.navigationDelegate=nil;view.removeFromSuperview()
            // Retire the page as well, including a pending getUserMedia prompt.
            view.loadHTMLString("",baseURL:nil)
        }
        view=nil
    }
    private func fail(_ error:String){stop();onError?(error)}
    fileprivate func receive(_ message:WKScriptMessage){
        guard message.webView === view,message.frameInfo.isMainFrame,let value=message.body as? [String:Any],let type=value["type"] as? String,let sessionID else{return}
        if type == "ready"{onDiagnosticEvent?("documentReady");invoke("start",[sessionID,captureMicrophone,initialPrompt?.base64EncodedString() ?? "",diagnosticPromptOnly]);initialPrompt=nil;return}
        guard value["sessionId"] as? String == sessionID else{return}
        switch type {
        case "offer":if let sdp=value["sdp"] as? String,!sdp.isEmpty,sdp.utf8.count <= 128*1024{onOffer?(sdp)}
        case "connected":onConnected?()
        case "promptSent":onDiagnosticEvent?("initialPromptSent")
        case "activity":if let raw=value["activity"] as? String,let activity=VoiceActivity(rawValue:raw){onActivity?(activity)}
        case "error":fail(value["message"] as? String ?? "Codex voice media failed")
        case "level":if let level=value["level"] as? Double,level.isFinite{let bounded=CGFloat(max(0,min(1,level)));onLevels?([bounded*0.82,bounded*0.92,bounded,bounded*0.88])}
        case "stats":if let count=value["audioBytes"] as? Int{onDiagnosticAudio?(count)}
        case "event":if let event=value["eventType"] as? String{onDiagnosticEvent?(event)}
        default:break
        }
    }
    nonisolated func webView(_ webView:WKWebView,decidePolicyFor navigationAction:WKNavigationAction,decisionHandler:@escaping(WKNavigationActionPolicy)->Void){
        MainActor.assumeIsolated {
            let url=navigationAction.request.url
            let localDocument=url?.scheme == "http" && url?.host == "localhost" && url?.port == nil && ["","/"].contains(url?.path ?? "") && url?.query == nil
            decisionHandler(webView === view && (url?.absoluteString == "about:blank" || localDocument) ? .allow:.cancel)
        }
    }
    nonisolated func webView(_ webView:WKWebView,requestMediaCapturePermissionFor origin:WKSecurityOrigin,initiatedByFrame frame:WKFrameInfo,type:WKMediaCaptureType,decisionHandler:@escaping(WKPermissionDecision)->Void){
        MainActor.assumeIsolated {
            let local=(origin.host == "localhost" && origin.protocol == "http") || (origin.host.isEmpty && origin.protocol == "about")
            decisionHandler(webView === view && sessionID != nil && captureMicrophone && frame.isMainFrame && local && type == .microphone ? .grant:.deny)
        }
    }
    nonisolated func webView(_ webView:WKWebView,didFinish navigation:WKNavigation!){
        MainActor.assumeIsolated{if webView === view{onDiagnosticEvent?("documentLoaded")}}
    }
    nonisolated func webViewWebContentProcessDidTerminate(_ webView:WKWebView){MainActor.assumeIsolated{if webView === view{fail("Codex voice media process stopped; start voice again")}}}
    nonisolated func webView(_ webView:WKWebView,didFailProvisionalNavigation navigation:WKNavigation!,withError error:Error){MainActor.assumeIsolated{if webView === view{fail(error.localizedDescription)}}}
}
