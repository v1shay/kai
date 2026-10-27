import AppKit
private func unwrap(_ value:Any)->Any{let m=Mirror(reflecting:value);if m.displayStyle == .optional,let child=m.children.first{return unwrap(child.value)};return value}
private func member(_ object:Any,_ name:String)->Any?{Mirror(reflecting:object).children.first{$0.label==name}.map{unwrap($0.value)}}
private func auditCommand(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{guard let bytes else{return false};let data=Data(bytes:bytes,count:count);MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any]{ComposerAudit.commands.append(value)}};return true}
@main struct ComposerAudit{
 @MainActor static var commands=[[String:Any]]()
 @MainActor static func main()throws{
  _=NSApplication.shared;UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded");UserDefaults.standard.set(false,forKey:"soundsEnabled");UserDefaults.standard.set(false,forKey:"spotifyMedia");UserDefaults.standard.set(false,forKey:"youtubeMedia")
  let window=NSWindow(contentRect:CGRect(x:0,y:0,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false);window.isReleasedWhenClosed=false
  let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={false};runtime.start(window:window,command:auditCommand)
  guard let app=member(runtime,"app") as? KaiPetApp,let ui=member(app,"miniUI") as? MiniAppInterface else{fatalError("Missing native UI")}
  defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
  func pump(_ seconds:Double=0.4){let end=Date().addingTimeInterval(seconds);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}}
  var owner="composer-owner"
  func packet(_ access:String,_ model:String)->Data{
   let models=["gpt-6-luna","gpt-6.1-terra","gpt-6.1-sol","gpt-6-astra"].map{["model":$0,"displayName":$0,"supportedReasoningEfforts":[["reasoningEffort":"low"],["reasoningEffort":"medium"],["reasoningEffort":"high"]]] as [String:Any]}
   let value:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:composer","threadName":"A native composer","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"sessionActivity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","attachments":[],"selectedAgent":"codex","agents":[["id":"codex","name":"Codex","available":true]],"models":models,"ownerToken":owner,"composerDraft":"","effectiveModel":model,"effectiveReasoningEffort":"medium","workspaceAccess":access,"workspaceApprovalPolicy":"ask","workspaceExecutionPath":"/private/tmp/project"]
   return try! JSONSerialization.data(withJSONObject:value)
  }
  runtime.receive(packet("workspace","gpt-6.1-sol"));pump();app.expandToRunningChat();pump()
  guard let input=member(ui,"input") as? CALayer,let model=member(ui,"modelChip") as? CALayer,let permission=member(ui,"permissionGlyph") as? CAGradientLayer,let name=member(ui,"modelName") as? CATextLayer else{fatalError("Missing composer controls")}
  precondition(input.bounds.height==58,"Modern composer did not reserve a separate editing row")
  precondition(!model.frame.intersects((member(ui,"voiceButton") as! CALayer).frame),"Model overlaps voice control")
  precondition(model.frame.minX>permission.frame.maxX,"Model collides with permission glyph")
  precondition(name.string as? String == "GPT-6.1 Sol","Actual model missing")
  for id in ["gpt-6-luna","gpt-6.1-terra","gpt-6.1-sol","gpt-6-astra"]{precondition(ModelIdentity.image(id) != nil,"Missing model vector \(id)")}
  let modelMenu=ui.settingsMenu("model")!;precondition(modelMenu.items.filter{$0.image != nil}.count==4,"Model menus do not carry all four family glyphs")
  ui.setComposerHasText(true);precondition(model.opacity==1 && permission.opacity==1,"Typing hides the composer controls");ui.setComposerHasText(false)
  let effortMenu=ui.settingsMenu("effort")!,slider=effortMenu.items.compactMap{$0.view as? ReasoningSliderControl}.first!
  precondition(slider.levels==["low","medium","high"] && slider.selected==1,"Wrong native model-specific levels")
  let before=commands.count;_ = slider.accessibilityPerformIncrement();precondition(commands.count==before+1 && commands.last?["effort"] as? String == "high","Slider does not dispatch native effort")
  let staleMenu=ui.permissionMenu()!,full=staleMenu.items.first{$0.title=="Full access"}!
  _=NSApp.sendAction(full.action!,to:full.target,from:full);precondition(commands.last?["access"] as? String == "full" && commands.last?["approvals"] == nil,"Sandbox symbol coupled access to approvals")
  owner="another-owner";runtime.receive(packet("workspace","gpt-6.1-terra"));pump(0.1);let after=commands.count;_ = slider.accessibilityPerformDecrement();_ = NSApp.sendAction(full.action!,to:full.target,from:full);precondition(commands.count==after,"Stale slider or permission menu changed another chat")
  var palettes=[[CGColor]]()
  for (access,id) in [("read-only","gpt-6-luna"),("workspace","gpt-6.1-terra"),("full","gpt-6-astra")]{runtime.receive(packet(access,id));pump(0.3);palettes.append(permission.colors as! [CGColor])}
  precondition(palettes[0] != palettes[1] && palettes[1] != palettes[2],"Permission colors do not reflect actual sandbox")
  let originalFrame=ui.layer.frame
  for width:CGFloat in [280,360,450]{ui.layout(in:CGRect(x:0,y:0,width:width,height:240));precondition(model.frame.maxX <= (member(ui,"voiceButton") as! CALayer).frame.minX,"Narrow composer collision")}
  ui.layout(in:originalFrame)
  // Render the actual expanded native panel's own content, never the desktop.
  app.expandToRunningChat();pump();if let panel=member(app,"panel") as? NSPanel,let view=panel.contentView,let bitmap=view.bitmapImageRepForCachingDisplay(in:view.bounds){view.cacheDisplay(in:view.bounds,to:bitmap);if let png=bitmap.representation(using:.png,properties:[:]){try png.write(to:URL(fileURLWithPath:"/private/tmp/kai-v084-native-composer.png"))}}
  print("Native composer: four vector model glyphs, per-model animated slider, exact effort dispatch, independent gradient permissions, stale-owner rejection, typed-draft controls, and narrow layouts passed")
 }
}
