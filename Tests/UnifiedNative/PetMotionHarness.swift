import AppKit
private func command(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{true}
private func field(_ object:Any,_ key:String)->Any?{guard let value=Mirror(reflecting:object).children.first(where:{$0.label==key})?.value else{return nil};let mirror=Mirror(reflecting:value);return mirror.displayStyle == .optional ? mirror.children.first?.value:value}
@main struct PetMotionAudit {
 @MainActor static func main()throws{
  _=NSApplication.shared
  for key in ["soundsEnabled","spotifyMedia","youtubeMedia"]{UserDefaults.standard.set(false,forKey:key)}
  UserDefaults.standard.set(true,forKey:"unifiedPreferencesSeeded")
  let workspace=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false),quick=NotchPanel(contentRect:CGRect(x:350,y:200,width:680,height:520),styleMask:[.borderless],backing:.buffered,defer:false)
  workspace.isReleasedWhenClosed=false;quick.isReleasedWhenClosed=false
  let runtime=EmbeddedCodeRuntime.shared;runtime.workspaceFocusProvider={true};runtime.start(window:workspace,command:command)
  let app=field(runtime,"app") as! KaiPetApp,sprite=field(app,"sprite") as! CALayer,pet=field(app,"workspacePet") as! NSView
  defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
  let state:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"notice":"Ready","threadId":"code:pet-motion","threadName":"Pet movement","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"sessionActivity":["scene":"off","text":"Ready"],"activeTasks":[],"attachments":[],"selectedAgent":"codex","models":[],"ownerToken":"motion"]
  runtime.receive(try JSONSerialization.data(withJSONObject:state))
  func pump(_ seconds:Double){let end=Date().addingTimeInterval(seconds);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.005))}}
  app.showUnifiedNotchOnLaunch();pump(0.3);app.expandToRunningChat()
  let travel=sprite.animation(forKey:"responsiveTravel") as? CABasicAnimation
  print("notch travel key=\(travel?.keyPath ?? "none") model=\(sprite.position)")
  app.expandWorkspace()
  let crossing=sprite.animation(forKey:"responsiveTravel") as? CABasicAnimation
  print("after reparent: local position=\(sprite.position), animation from=\(String(describing:crossing?.fromValue)), to=\(String(describing:crossing?.toValue))")
  precondition(sprite.superlayer === pet.layer,"Pet did not transfer to the workspace")
  precondition(crossing==nil,"Notch-coordinate travel survived reparenting into the workspace")
  pump(0.9);app.foldWorkspace();pump(0.7)
  precondition(sprite.superlayer === field(app,"notchContent") as? CALayer,"Pet did not return to the extended notch")
  app.showUnifiedNotchOnLaunch();pump(0.04)
  if let animation=sprite.animation(forKey:"responsiveTravel") as? CABasicAnimation{precondition(animation.keyPath=="position","Pet still snaps Y before traveling X")}
  app.prepareQuickOpening();precondition(app.transitionQuickSurface(quick,show:true));pump(0.04)
  precondition(sprite.superlayer === pet.layer && pet.superview === quick.contentView,"Pet did not transfer into Mini Code")
  precondition(sprite.animation(forKey:"responsiveTravel")==nil,"Notch travel survived reparenting into Mini Code")
  pump(0.9);app.revealWorkspace(from:quick.frame,sourceIsNotch:false);pump(0.9)
  precondition(pet.superview === workspace.contentView,"Mini-to-workspace pet host is wrong")
  precondition(sprite.animation(forKey:"responsiveTravel")==nil,"Mini-to-workspace retained stale notch movement")
  print("Shared pet motion: full-position notch travel and clean geometry ownership across extended, compact, workspace and mini surfaces passed")
 }
}
