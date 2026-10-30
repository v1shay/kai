import AppKit
import QuickLookUI
import WebKit

/// Lazy native surfaces. No WebKit process or Quick Look view exists at launch.
@MainActor final class NativeWorkspaceIntegrations:NSObject,NSWindowDelegate,WKNavigationDelegate {
    static let shared=NativeWorkspaceIntegrations()
    private var preview:NSWindowController?
    private var web:WKWebView?
    private var installed=false
    func install(){
        guard !installed else{return};installed=true
        NSApp.servicesProvider=self
        NSAppleEventManager.shared().setEventHandler(self,andSelector:#selector(openDocuments(_:withReplyEvent:)),forEventClass:AEEventClass(kCoreEventClass),andEventID:AEEventID(kAEOpenDocuments))
    }
    @objc func openInKai(_ pasteboard:NSPasteboard,userData:String?,error:AutoreleasingUnsafeMutablePointer<NSString>){
        let urls=(pasteboard.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL]) ?? []
        guard !urls.isEmpty else{error.pointee="Choose a folder or files in Finder";return}
        EmbeddedCodeRuntime.shared.sendAction("external_urls",["urls":urls.prefix(64).map(\.absoluteString)])
    }
    @objc private func openDocuments(_ event:NSAppleEventDescriptor,withReplyEvent reply:NSAppleEventDescriptor){
        guard let items=event.paramDescriptor(forKeyword:AEKeyword(keyDirectObject)) else{return}
        var urls=[String]()
        if items.numberOfItems>0 {for index in 1...min(64,items.numberOfItems){if let item=items.atIndex(index)?.coerce(toDescriptorType:DescType(typeFileURL)),let text=String(data:item.data,encoding:.utf8),URL(string:text)?.isFileURL==true{urls.append(text)}}}
        if !urls.isEmpty{EmbeddedCodeRuntime.shared.sendAction("external_urls",["urls":urls])}
    }
    func quickLook(_ urls:[URL]){
        guard let url=urls.first,url.isFileURL,FileManager.default.fileExists(atPath:url.path) else{return}
        closePreview()
        let window=makeWindow(url.lastPathComponent)
        let view=QLPreviewView(frame:NSRect(x:0,y:0,width:840,height:640),style:.normal)!
        view.autoresizingMask=[.width,.height];view.previewItem=url as NSURL
        window.contentView=view;show(window)
    }
    func artifact(_ artifact:[String:Any]){
        guard let body=artifact["content"] as? String,body.utf8.count<=512*1024 else{return}
        closePreview();let window=makeWindow(artifact["title"] as? String ?? "Document")
        if artifact["kind"] as? String == "html" {
            let configuration=WKWebViewConfiguration();configuration.websiteDataStore = .nonPersistent()
            // No native bridge, local file base URL, shared cookies or window opening.
            configuration.preferences.javaScriptCanOpenWindowsAutomatically=false
            let web=WKWebView(frame:NSRect(x:0,y:0,width:840,height:640),configuration:configuration);self.web=web
            web.navigationDelegate=self;web.autoresizingMask=[.width,.height]
            let policy="<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; frame-src 'none'; object-src 'none'; form-action 'none'; base-uri 'none'\">"
            web.loadHTMLString(policy+body,baseURL:nil);window.contentView=web
        }else{
            let scroll=NSScrollView(frame:NSRect(x:0,y:0,width:840,height:640));scroll.hasVerticalScroller=true
            let text=NSTextView(frame:scroll.bounds);text.isEditable=false;text.isSelectable=true;text.autoresizingMask=[.width];text.textContainer?.widthTracksTextView=true;text.textContainerInset=NSSize(width:24,height:20)
            text.font=NSFont.systemFont(ofSize:14);text.textColor = .labelColor
            if artifact["kind"] as? String == "markdown",let markdown=try? AttributedString(markdown:body,options:.init(interpretedSyntax:.full)){text.textStorage?.setAttributedString(NSAttributedString(markdown))}else{text.string=body}
            scroll.documentView=text;window.contentView=scroll
        }
        show(window)
    }
    private func makeWindow(_ title:String)->NSWindow{
        let window=NSWindow(contentRect:NSRect(x:0,y:0,width:840,height:640),styleMask:[.titled,.closable,.resizable,.miniaturizable],backing:.buffered,defer:false)
        window.title=title;window.isReleasedWhenClosed=false;window.delegate=self;window.center();return window
    }
    private func show(_ window:NSWindow){let controller=NSWindowController(window:window);preview=controller;controller.showWindow(nil);window.makeKeyAndOrderFront(nil)}
    private func closePreview(){web?.stopLoading();web?.navigationDelegate=nil;web=nil;preview?.close();preview=nil}
    func windowWillClose(_ notification:Notification){web?.stopLoading();web?.navigationDelegate=nil;web=nil;preview=nil}
    nonisolated func webView(_ webView:WKWebView,decidePolicyFor action:WKNavigationAction,decisionHandler:@escaping(WKNavigationActionPolicy)->Void){
        if action.navigationType == .other,action.request.url?.scheme == "about"{decisionHandler(.allow);return}
        if action.navigationType == .linkActivated,let url=action.request.url,["https","http"].contains(url.scheme ?? ""){NSWorkspace.shared.open(url)}
        decisionHandler(.cancel)
    }
}
