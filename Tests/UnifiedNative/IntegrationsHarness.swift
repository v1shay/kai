import AppKit
import QuickLookUI
import WebKit
private func field<T>(_ object:Any,_ name:String,as:T.Type)->T?{let value=Mirror(reflecting:object).children.first{$0.label==name}?.value;guard let value else{return nil};if let direct=value as? T{return direct};return Mirror(reflecting:value).children.first?.value as? T}
private func integrationCommand(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{guard let bytes else{return false};let data=Data(bytes:bytes,count:count);MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any]{IntegrationsAudit.commands.append(value)}};return true}
@main struct IntegrationsAudit{
 @MainActor static var commands=[[String:Any]]()
 @MainActor static func main()throws{
  _=NSApplication.shared
  let root=FileManager.default.temporaryDirectory.appendingPathComponent("kai-native-integrations-\(UUID().uuidString)",isDirectory:true);try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true)
  defer{try? FileManager.default.removeItem(at:root);NSApp.windows.forEach{$0.orderOut(nil)}}
  let file=root.appendingPathComponent("A file #100%.txt");try "Preview this generated file".write(to:file,atomically:true,encoding:.utf8)
  let window=NSWindow(contentRect:CGRect(x:0,y:0,width:900,height:700),styleMask:[.titled],backing:.buffered,defer:false);window.isReleasedWhenClosed=false
  EmbeddedCodeRuntime.shared.start(window:window,command:integrationCommand)
  let integration=NativeWorkspaceIntegrations.shared
  let pasteboard=NSPasteboard.withUniqueName();pasteboard.writeObjects([file as NSURL]);defer{pasteboard.releaseGlobally()}
  var error:NSString="";integration.openInKai(pasteboard,userData:nil,error:&error)
  precondition(error.length==0);precondition((commands.last?["urls"] as? [String])==[file.absoluteString],"Finder service lost the exact URL")
  let rect = CGRect(x: 24, y: 40, width: 340, height: 280)
  func inlinePacket(_ geometry: CGRect?) throws {
      var packet: [String: Any] = ["type": "inlineQuickLook", "slot": "files"]
      if let geometry {
          packet["window"] = window.windowNumber
          packet["path"] = file.path
          packet["rect"] = [geometry.minX, geometry.minY, geometry.width, geometry.height]
      }
      EmbeddedCodeRuntime.shared.receive(try JSONSerialization.data(withJSONObject: packet))
  }
  try inlinePacket(rect)
  let inline = window.contentView!.subviews.compactMap { $0 as? QLPreviewView }.first!
  precondition(inline.previewItem?.previewItemURL == file)
  try inlinePacket(rect.offsetBy(dx: 12, dy: 0))
  precondition(window.contentView!.subviews.contains { $0 === inline } && inline.frame.minX == 36)
  try inlinePacket(nil)
  precondition(inline.superview == nil && inline.previewItem == nil)
  integration.quickLook([file]);let preview=field(integration,"preview",as:NSWindowController.self)!
  precondition(preview.window?.contentView is QLPreviewView,"Quick Look did not create a native preview")
  integration.artifact(["title":"Persistent document","kind":"markdown","content":"# A saved document\n\n**Important** context."])
  let document=field(integration,"preview",as:NSWindowController.self)!.window!.contentView as! NSScrollView
  precondition((document.documentView as! NSTextView).isEditable==false)
  integration.artifact(["title":"Isolated report","kind":"html","content":"<h1>Report</h1><script>window.reportLoaded=true</script>"])
  let web=field(integration,"web",as:WKWebView.self)!
  precondition(!web.configuration.websiteDataStore.isPersistent,"Report reused user web credentials")
  precondition(!web.configuration.preferences.javaScriptCanOpenWindowsAutomatically)
  var result:Any?;let until=Date().addingTimeInterval(8)
  while result==nil && Date()<until{RunLoop.main.run(until:Date().addingTimeInterval(0.03));web.evaluateJavaScript("window.reportLoaded===true && document.querySelector('meta[http-equiv]').content.includes(\"connect-src 'none'\")"){value,_ in if (value as? Bool)==true{result=value}}}
  precondition(result as? Bool==true,"Interactive report did not load under its security policy")
  var bridged:Any?;web.evaluateJavaScript("Object.keys(window.webkit?.messageHandlers ?? {}).length"){value,_ in bridged=value};let bridgeDeadline=Date().addingTimeInterval(2);while bridged==nil && Date()<bridgeDeadline{RunLoop.main.run(until:Date().addingTimeInterval(0.03))};precondition((bridged as? Int)==0,"Report has a native command bridge")
  field(integration,"preview",as:NSWindowController.self)?.close()
  print("Native integrations passed: exact Finder file URLs, native Quick Look, Markdown, isolated interactive HTML and no native bridge")
 }
}
