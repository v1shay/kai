import AppKit
private func unwrap(_ value:Any)->Any{let m=Mirror(reflecting:value);if m.displayStyle == .optional,let child=m.children.first{return unwrap(child.value)};return value}
private func member(_ object:Any,_ name:String)->Any?{Mirror(reflecting:object).children.first{$0.label==name}.map{unwrap($0.value)}}
private func auditCommand(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{guard let bytes else{return false};let data=Data(bytes:bytes,count:count);MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any]{ComposerAudit.commands.append(value)}};return true}
@main struct ComposerAudit{
 @MainActor static var commands=[[String:Any]]()
 @MainActor static func main()throws{
  _=NSApplication.shared;UserDefaults.standard.set(ProcessInfo.processInfo.environment["KAI_COMPOSER_WALLPAPER"]=="1",forKey:"notchCodeWallpaper");UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded");UserDefaults.standard.set(false,forKey:"soundsEnabled");UserDefaults.standard.set(false,forKey:"spotifyMedia");UserDefaults.standard.set(false,forKey:"youtubeMedia")
  let window=NSWindow(contentRect:CGRect(x:0,y:0,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false);window.isReleasedWhenClosed=false
  let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={false};runtime.start(window:window,command:auditCommand)
  guard let app=member(runtime,"app") as? KaiPetApp,let ui=member(app,"miniUI") as? MiniAppInterface else{fatalError("Missing native UI")}
  defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
  func pump(_ seconds:Double=0.4){let end=Date().addingTimeInterval(seconds);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.01))}}
  var owner="composer-owner"
  func packet(_ access:String,_ model:String)->Data{
   let models=["gpt-6-luna","gpt-6.1-terra","gpt-6.1-sol","gpt-6-astra"].map{["model":$0,"displayName":$0,"supportedReasoningEfforts":[["reasoningEffort":"low"],["reasoningEffort":"medium"],["reasoningEffort":"high"]]] as [String:Any]}
   let value:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":"code:composer","threadName":"A native composer","history":[["id":"first-user","kind":"user","lines":[["text":"A short new conversation","style":""]]],["id":"first-reply","kind":"assistant","lines":[["text":"Replies stay just above the composer.","style":""]]]],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"sessionActivity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","attachments":[],"selectedAgent":"codex","fastSupported":true,"fastMode":false,"usageSummary":"Codex · 83% left","usageWindows":[["label":"Weekly limit","remaining":"83% left","fraction":0.83]],"usageAccount":"Pro","usageUpdated":"Updated just now","agents":[["id":"codex","name":"Codex","available":true]],"models":models,"ownerToken":owner,"composerDraft":"","effectiveModel":model,"effectiveReasoningEffort":"medium","workspaceAccess":access,"workspaceApprovalPolicy":"ask","workspaceExecutionPath":"/private/tmp/project"]
   return try! JSONSerialization.data(withJSONObject:value)
  }
  runtime.receive(packet("workspace","gpt-6.1-sol"));pump();app.expandToRunningChat();pump()
  guard let transcript=member(app,"conversationText") as? ConversationTextView,let scroll=member(app,"conversationScroll") as? NSScrollView else{fatalError("Missing bottom-aligned native transcript")}
  precondition(transcript.string.contains("Replies stay"),"The real notch did not render its history")
  precondition(transcript.leadingSpace>0,"Short native conversation stayed at the top")
  let used=transcript.layoutManager!.usedRect(for:transcript.textContainer!).maxY
  precondition(abs(transcript.textContainerOrigin.y+ceil(used)-(scroll.contentSize.height-transcript.textContainerInset.height))<1,"Native messages are detached from the composer")
  guard let input=member(ui,"input") as? CALayer,let model=member(ui,"modelChip") as? CALayer,let permission=member(ui,"permissionGlyph") as? CAGradientLayer,let name=member(ui,"modelName") as? CATextLayer else{fatalError("Missing composer controls")}
  precondition(input.bounds.height==58,"Modern composer did not reserve a separate editing row")
  precondition(!model.frame.intersects((member(ui,"voiceButton") as! CALayer).frame),"Model overlaps voice control")
  precondition(model.frame.minX>permission.frame.maxX,"Model collides with permission glyph")
  precondition(name.string as? String == "GPT-6.1 Sol","Actual model missing")
  for id in ["gpt-6-luna","gpt-6.1-terra","gpt-6.1-sol","gpt-6-astra"]{precondition(ModelIdentity.image(id) != nil,"Missing model vector \(id)")}
  let modelMenu=ui.settingsMenu("model")!;precondition(modelMenu.items.filter{$0.image != nil}.count==4,"Model menus do not carry all four family glyphs")
  ui.setComposerHasText(true);precondition(model.opacity==1 && permission.opacity==1,"Typing hides the composer controls");ui.setComposerHasText(false)
  let bolt=member(ui,"fastTint") as! CALayer;precondition(!bolt.isHidden,"Advertised Fast Mode is missing");let shimmer=member(ui,"fastShimmer") as! CALayer;precondition(shimmer.sublayers?.count==160,"Fast and intensity must share tile animation geometry");let fastBefore=commands.count;_ = ui.handleClick(CGPoint(x:input.frame.minX+bolt.frame.midX,y:input.frame.minY+bolt.frame.midY));precondition(commands.count==fastBefore+1 && commands.last?["action"] as? String == "fast_mode","Fast Mode did not reach owner")
  let quota=member(ui,"usageLabel") as! CATextLayer;let quotaTrack=member(ui,"usageTrack") as! CALayer;precondition(abs(quota.frame.midY-quotaTrack.frame.midY)<0.01,"Usage bar/text do not share the footer center");let chipShimmer=member(ui,"effortShimmer") as! EffortTileLayer;precondition(chipShimmer.previewing,"Selected thinking button has no tile animation");precondition((quota.string as? String)?.contains("83% left")==true,"Usage footer does not show remaining");_ = ui.handleClick(CGPoint(x:quota.frame.midX,y:quota.frame.midY));let popup=member(ui,"composerPopover") as! ComposerGlassPopover;precondition(popup.visible,"Usage did not open its custom glass panel");popup.dismiss()
  let chip=member(ui,"effortChip") as! CALayer;ui.handleHover(CGPoint(x:input.frame.minX+chip.frame.midX,y:input.frame.minY+chip.frame.midY));precondition(!popup.visible,"Thinking hover must not open a panel")
  let effortPoint=CGPoint(x:input.frame.minX+chip.frame.midX,y:input.frame.minY+chip.frame.midY)
  _=ui.handleClick(effortPoint);precondition(popup.visible,"Thinking click did not open the panel")
  _=ui.handleClick(effortPoint);precondition(!popup.visible,"Second click must close thinking")
  _=ui.handleClick(effortPoint);_ = ui.handleClick(CGPoint(x:quota.frame.midX,y:quota.frame.midY));precondition(popup.visible && (member(popup,"effortRows") as! [EffortTileRow]).isEmpty,"Different trigger must replace thinking with usage")
  _ = ui.handleClick(CGPoint(x:quota.frame.midX,y:quota.frame.midY));precondition(!popup.visible,"Second click must close usage")
  _=ui.handleClick(effortPoint);_ = ui.handleClick(CGPoint(x:input.frame.midX,y:input.frame.maxY-10));precondition(!popup.visible,"Clicking elsewhere must close thinking")
  _=ui.handleClick(effortPoint)
  precondition(chip.frame.minX-model.frame.maxX>=2 && chip.frame.minX-model.frame.maxX<=5,"Thinking intensity is detached from the model name")
  let scalableForFooter=member(app,"scalableUI") as! NSView,footerScale=scalableForFooter.frame.height/scalableForFooter.bounds.height
  let composerBottom=scalableForFooter.frame.minY+input.frame.minY*footerScale,usageCenter=scalableForFooter.frame.minY+quota.frame.midY*footerScale
  precondition(abs(usageCenter-composerBottom/2)<0.5,"Usage is not centered between the prompt and physical notch bottom")
  let edit=member(app,"composerField") as! NSTextField;precondition(edit.frame.height==24,"The editing row clips wrapped input")
  let shownChoices=member(popup,"effortRows") as! [EffortTileRow],selectedChoice=shownChoices.first{$0.selected}!,otherChoice=shownChoices.first{!$0.selected}!
  func previewing(_ row:EffortTileRow)->Bool{(member(row,"tiles") as! EffortTileLayer).previewing}
  precondition(previewing(selectedChoice) && !previewing(otherChoice),"Selected thinking option does not animate by default")
  otherChoice.onPreview?(otherChoice.value,true);precondition(!previewing(selectedChoice) && previewing(otherChoice),"Hover did not transfer the thinking preview")
  otherChoice.onPreview?(otherChoice.value,false);precondition(previewing(selectedChoice) && !previewing(otherChoice),"Leaving hover did not restore the selected thinking preview")
  popup.dismiss();precondition(shownChoices.allSatisfy{!previewing($0)},"Closed thinking picker retained animations")
  let choices=ui.thinkingRows(),choice=choices.last!
  precondition(choices.map(\.value)==["low","medium","high"],"Wrong native model-specific levels");precondition((member(choice,"tiles") as? CALayer)?.sublayers?.count==160,"Missing Monocode tile shimmer")
  let before=commands.count;_ = choice.accessibilityPerformPress();precondition(commands.count==before+1 && commands.last?["effort"] as? String == "high","Thinking option does not dispatch native effort")
  let staleMenu=ui.permissionMenu()!,full=staleMenu.items.first{$0.title=="Full access"}!
  _=NSApp.sendAction(full.action!,to:full.target,from:full);precondition(commands.last?["access"] as? String == "full" && commands.last?["approvals"] == nil,"Sandbox symbol coupled access to approvals")
  owner="another-owner";runtime.receive(packet("workspace","gpt-6.1-terra"));pump(0.1);let after=commands.count;_ = choice.accessibilityPerformPress();_ = NSApp.sendAction(full.action!,to:full.target,from:full);precondition(commands.count==after,"Stale thinking option or permission menu changed another chat")
  var palettes=[[CGColor]]()
  for (access,id) in [("read-only","gpt-6-luna"),("workspace","gpt-6.1-terra"),("full","gpt-6-astra")]{runtime.receive(packet(access,id));pump(0.3);palettes.append(permission.colors as! [CGColor])}
  precondition(palettes[0] != palettes[1] && palettes[1] != palettes[2],"Permission colors do not reflect actual sandbox")
  if ProcessInfo.processInfo.environment["KAI_COMPOSER_WALLPAPER"]=="1" {
   let pixels=[UInt8](repeating:255,count:8*8*4)
   pixels.withUnsafeBufferPointer{runtime.receiveWallpaper($0.baseAddress,count:$0.count,width:8,height:8,opacity:0.5)}
   let background=member(app,"workspaceBackground") as! NotchWorkspaceBackground
   precondition(!background.isHidden && background.hasImage,"Selected Code wallpaper did not appear")
   precondition((member(app,"glassBackground") as! NSView).isHidden,"Wallpaper mode kept glass active")
   precondition(commands.contains{$0["action"] as? String=="workspace_wallpaper_visibility" && $0["visible"] as? Bool==true},"Native wallpaper did not subscribe to Code's surface")
  }
  let originalFrame=ui.layer.frame
  for width:CGFloat in [280,360,450]{ui.layout(in:CGRect(x:0,y:0,width:width,height:240));precondition(model.frame.maxX <= (member(ui,"voiceButton") as! CALayer).frame.minX,"Narrow composer collision")}
  ui.layout(in:originalFrame)
  // Render the actual expanded native panel's own content, never the desktop.
  app.expandToRunningChat();pump();if let panel=member(app,"panel") as? NSPanel,let view=panel.contentView,let bitmap=view.bitmapImageRepForCachingDisplay(in:view.bounds){view.cacheDisplay(in:view.bounds,to:bitmap);if let png=bitmap.representation(using:.png,properties:[:]){try png.write(to:URL(fileURLWithPath:"/private/tmp/kai-v084-native-composer.png"))}}
  if let panel=member(app,"panel") as? NSPanel,let field=member(app,"composerField") as? NSTextField,let scalable=member(app,"scalableUI") as? NSView,let main=member(ui,"main") as? CALayer,let bar=member(ui,"actionBar") as? CALayer,let hits=member(ui,"actionHits") as? [(CGRect,String)],let new=hits.first(where:{$0.1=="new"}) {
   precondition(panel.contentView!.acceptsFirstMouse(for:nil) && scalable.acceptsFirstMouse(for:nil) && field.acceptsFirstMouse(for:nil),"The notch rejects first clicks while another app is active")
   func click(_ point:CGPoint){let up=NSEvent.mouseEvent(with:.leftMouseUp,location:point,modifierFlags:[],timestamp:ProcessInfo.processInfo.systemUptime,windowNumber:panel.windowNumber,context:nil,eventNumber:1,clickCount:1,pressure:0)!;NSApp.postEvent(up,atStart:false);let down=NSEvent.mouseEvent(with:.leftMouseDown,location:point,modifierFlags:[],timestamp:ProcessInfo.processInfo.systemUptime,windowNumber:panel.windowNumber,context:nil,eventNumber:0,clickCount:1,pressure:1)!;NSApp.sendEvent(down);pump(0.1)}
   click(field.convert(CGPoint(x:field.bounds.midX,y:field.bounds.midY),to:nil))
   precondition(panel.firstResponder is NSTextView,"The actual notch window did not focus its composer")
   precondition(field.cell is ComposerTextCell,"The live composer retained the top-aligned default text cell")
   let focusedEditor=field.currentEditor() as! NSTextView
   let lineHeight=focusedEditor.layoutManager!.defaultLineHeight(for:field.font!)
   let insertionLine=field.convert(CGRect(origin:focusedEditor.textContainerOrigin,size:CGSize(width:1,height:lineHeight)),from:focusedEditor)
   precondition(abs(insertionLine.midY-field.bounds.midY)<2,"The typing cursor is not vertically centered with the editing row")
   print("Native composer: focused typing cursor centered in the actual editing row")
   let actualEffort=scalable.convert(CGPoint(x:ui.layer.frame.minX+effortPoint.x,y:ui.layer.frame.minY+effortPoint.y),to:nil)
   click(actualEffort);precondition(popup.visible,"Actual thinking click was swallowed")
   click(actualEffort);precondition(!popup.visible,"Actual trigger click reopened a dismissed panel")
   click(actualEffort);click(field.convert(CGPoint(x:field.bounds.midX,y:field.bounds.midY),to:nil));precondition(!popup.visible,"Actual composer click left thinking open")
   print("Native controls: hover is visual only; trigger toggles, panel switching and outside clicks passed")
   let rect=new.0.offsetBy(dx:ui.layer.frame.minX+main.frame.minX+bar.frame.minX,dy:ui.layer.frame.minY+main.frame.minY+bar.frame.minY),before=commands.count
   click(scalable.convert(CGPoint(x:rect.midX,y:rect.midY),to:nil))
   precondition(commands.count>before && commands.last?["action"] as? String == "new","The actual notch window swallowed its New button")
   ui.cancelPending()
   print("Native window input: actual mouse events focus the composer and dispatch New")
  }else{fatalError("Missing native click targets")}
  if let field=member(app,"composerField") as? NSTextField,let panel=field.window {
   var state=try JSONSerialization.jsonObject(with:packet("workspace","gpt-6.1-sol")) as! [String:Any]
   state["slashCommands"]=[["name":"motion","description":"Toggle motion"]]
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1)
   panel.makeKeyAndOrderFront(nil);field.selectText(nil)
   let editor=field.currentEditor() as! NSTextView,draft="🦀 please /motion continue",caret="🦀 please /motion".utf16.count
   editor.string=draft;field.stringValue=draft;editor.setSelectedRange(NSRange(location:caret,length:0))
   app.controlTextDidChange(Notification(name:NSControl.textDidChangeNotification,object:field))
   let picker=member(app,"slashPicker") as! NativeSlashPicker
   precondition(picker.active,"The actual notch did not show mid-sentence commands")
   precondition(app.control(field,textView:editor,doCommandBy:#selector(NSResponder.insertNewline(_:))),"Notch Enter did not accept the command")
   let command=commands.last!;precondition(command["action"] as? String == "composer_command" && command["name"] as? String == "motion" && command["caret"] as? Int == caret,"Native command lost its exact caret or route")
   state["composerDraft"]="🦀 please  continue";state["completedRequestId"]=command["requestId"]
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1)
   precondition(field.stringValue == "🦀 please  continue","Native command acknowledgement failed to synchronize the remaining draft")
   state["approval"]=true;state["approvalThreadId"]="code:background";state["approvalRequestId"]="epoch:confirm";state["approvalSummary"]="Connected tool needs input\n\nAllow the tool to update a draft?"
   state["nativeReview"]=["owner":"code:background","token":"epoch:confirm","kind":"questions","title":"Connected tool needs input","detail":"Allow the tool to update a draft?","choices":[],"questions":[["id":"confirm","header":"Confirm","question":"Allow the tool to update a draft?","secret":false,"multiple":false,"required":true,"options":[["id":"true","label":"Yes"],["id":"false","label":"No"]]]]] as [String:Any]
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.2)
   let approval=member(app,"approvalUI") as! CompactApprovalView
   func pressApproval(_ action:String){let hits=member(ui,"actionHits") as! [(CGRect,String)],bar=member(ui,"actionBar") as! CALayer;let rect=hits.first{$0.1==action}!.0;precondition(ui.handleClick(bar.convert(CGPoint(x:rect.midX,y:rect.midY),to:ui.layer)),"Extended approval SVG was not clickable")}
   pressApproval("review");pump(0.35);precondition(approval.reviewExpanded && ui.pendingRequestID==nil,"Opening review consumed the pending request")
   approval.setReviewExpanded(false);app.expandToRunningChat();pump(0.35)
   precondition(member(app,"miniOpen") as? Bool == true,"Review could not return to the extended notch")
   pressApproval("reject")
   precondition(commands.last?["action"] as? String == "native_answer" && commands.last?["answers"] as? [String:[String]] == ["confirm":["false"]],"Native tool confirmation sent an approval instead of a typed answer")
   precondition(commands.last?["threadId"] as? String == "code:background" && commands.last?["approvalRequestId"] as? String == "epoch:confirm","Notch question lost its background owner")
   state["completedRequestId"]=commands.last?["requestId"];state["approval"]=false;state.removeValue(forKey:"nativeReview");runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1)
   state["approval"]=true;state["approvalRequestId"]="epoch:command"
   state["nativeReview"]=["owner":"code:background","token":"epoch:command","kind":"approval","title":"Approve terminal command","detail":"printf 'Approval test'","questions":[],"choices":[["id":"accept","label":"Allow once"],["id":"cancel","label":"Stop turn"],["id":"acceptForSession","label":"Allow for this session"]]] as [String:Any]
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1);pressApproval("review");pump(0.35)
   let visibleActions=approval.subviews.compactMap{$0 as? ApprovalButton}.filter{!$0.isHidden}
   precondition(Set(visibleActions.compactMap{$0.accessibilityLabel()})==Set(["Allow once","Stop turn","Allow for this session","Review request"]),"Expanded review duplicated its primary action controls")
   precondition(visibleActions.allSatisfy{approval.bounds.contains($0.frame)},"Review actions escaped the animated footer")
   if let host=approval.window?.contentView,let bitmap=host.bitmapImageRepForCachingDisplay(in:host.bounds){host.cacheDisplay(in:host.bounds,to:bitmap);if let png=bitmap.representation(using:.png,properties:[:]){try png.write(to:URL(fileURLWithPath:"/private/tmp/kai-v095-review.png"))}}
   visibleActions.first{$0.accessibilityLabel()=="Allow for this session"}!.performClick(nil)
   precondition(commands.last?["choice"] as? String=="acceptForSession" && commands.last?["approvalRequestId"] as? String=="epoch:command","Review footer did not preserve the exact provider choice")
   state["completedRequestId"]=commands.last?["requestId"];state["approval"]=false;state.removeValue(forKey:"nativeReview");runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1);app.expandToRunningChat();pump(0.3)
   print("Native approval review: one SVG action row, bounded footer, exact provider choice and background owner passed")
   // The snapshot can be one event behind the owner while changing form
   // factors. It must never refuse a send locally on that stale busy bit.
   state["composerBusy"]=true;state["readOnly"]=true
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1)
   field.stringValue="Continue from the same notch chat";let beforeSend=commands.count;ui.onSend?()
   precondition(commands.count>beforeSend && commands.last?["action"] as? String=="send" && commands.last?["text"] as? String==field.stringValue,"The notch rejected a shared-owner submission using stale presentation state")
   state["completedRequestId"]=commands.last?["requestId"];state["composerBusy"]=false;state["readOnly"]=false;state["composerDraft"]="";runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1);app.expandToRunningChat();pump(0.3)
   state["history"]=[["id":"prompt","kind":"user","lines":[["text":"Preserve the previous prompt","style":"text"]]],["id":"final","kind":"assistant","lines":[["text":"The final response arrived without typing","style":"text"]]]]
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.4)
   precondition(transcript.string.contains("Preserve the previous prompt") && transcript.string.contains("The final response arrived without typing"),"A finished response waited for another prompt in the extended notch")
   print("Native shared chat: stale busy/history flags do not block owner submission; final text paints without another prompt")
   print("Native notch: mid-sentence command input, exact Unicode caret, synchronized draft acknowledgement, and background tool Yes/No response passed")
  }
  if let requested=ProcessInfo.processInfo.environment["KAI_COMPOSER_SOAK_SECONDS"].flatMap(Double.init),requested>0 {
   let seconds=min(180,requested),end=Date().addingTimeInterval(seconds);var iteration=0
   while Date()<end {
    runtime.receive(packet("workspace",["gpt-6-luna","gpt-6.1-terra","gpt-6.1-sol","gpt-6-astra"][(iteration/25)%4]));pump(0.2);iteration+=1
    precondition(member(app,"miniOpen") as? Bool == true,"Idle updates retired the extended notch")
   }
   print("Native expanded-notch idle audit: \(Int(seconds)) seconds and \(iteration) status deliveries without losing its surface")
  }
  if ProcessInfo.processInfo.environment["KAI_LOCAL_LIBRARY_VERIFY"] == "1",let field=member(app,"composerField") as? NSTextField{
   let menu=member(app,"typographyMenu") as! TypographyMenu,fonts=menu.item.submenu!.items[0].submenu!
   menu.menuNeedsUpdate(fonts)
   precondition(fonts.items.contains{$0.title=="Google Sans"} && fonts.items.contains{$0.title=="Geist"},"Native settings failed to discover user-installed families")
   let geist=fonts.items.first{$0.title=="Geist"}!
   _=NSApp.sendAction(geist.action!,to:geist.target,from:geist)
   precondition(commands.last?["action"] as? String == "typography" && commands.last?["interfaceFont"] as? String == "Geist","Native font setting did not reach Code's shared owner")
   var state=try JSONSerialization.jsonObject(with:packet("workspace","gpt-6.1-sol")) as! [String:Any]
   state["interfaceFont"]="Geist";state["codeFont"]="Menlo"
   runtime.receive(try JSONSerialization.data(withJSONObject:state));pump(0.1)
   precondition(field.font?.familyName=="Geist","Owner font did not repaint the actual notch editor")
   let catalog=member(app,"catalog") as! Catalog
   precondition(catalog.pets.filter{$0.kind=="local"}.count>=6,"The live native pet picker did not discover the user's library")
   precondition(catalog.pets.filter{$0.kind=="local"}.allSatisfy{$0.sourceURL != nil},"Local pets still point at packaged copies")
   print("Native libraries: Google Sans and Geist in settings, shared font dispatch, real editor repaint, and live external pet sources passed")
  }
  // Worker completion may update status/pets, but must not navigate away
  // from the orchestrator whose final brief is still being produced.
  var completion=try JSONSerialization.jsonObject(with:packet("workspace","gpt-6.1-sol")) as! [String:Any]
  completion["threadId"]="code:orchestrator";completion["threadName"]="Orchestrator";completion["finishedTasks"]=[]
  completion["activeTasks"]=[["id":"code:orchestrator","name":"Orchestrator","scene":"thinking","text":"Coordinating","runId":"parent-run"],["id":"code:worker","name":"Worker","scene":"thinking","text":"Working","runId":"worker-run","parentId":"code:orchestrator"]]
  runtime.receive(try JSONSerialization.data(withJSONObject:completion));pump()
  completion["activeTasks"]=[["id":"code:orchestrator","name":"Orchestrator","scene":"thinking","text":"Coordinating","runId":"parent-run"]]
  completion["finishedTasks"]=[["id":"code:worker","runId":"worker-run","parentId":"code:orchestrator"]]
  let reviewBefore=commands.count;runtime.receive(try JSONSerialization.data(withJSONObject:completion));pump()
  let reviews=member(app,"floatingReviews") as! [KaiActiveTask];precondition(!reviews.contains{$0.id=="code:worker"},"Finished worker stole the parent conversation")
  precondition(!commands.dropFirst(reviewBefore).contains{$0["action"] as? String == "chat"},"Worker completion navigated the shared chat")
  print("Native orchestration: worker completion updates status without stealing parent selection")
  print("Native composer: four vector model glyphs, per-model tile shimmer choices, exact effort dispatch, independent gradient permissions, stale-owner rejection, typed-draft controls, and narrow layouts passed")
 }
}
