import AppKit
private func noop(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{
 guard let bytes else{return false}
 let data=Data(bytes:bytes,count:count)
 MainActor.assumeIsolated{if let value=try? JSONSerialization.jsonObject(with:data) as? [String:Any],value["action"] as? String == "workspace_presentation",let phase=value["phase"] as? String{QuickSettledHarness.presentations.append(phase)}}
 return true
}
private func unwrap(_ value:Any)->Any{let m=Mirror(reflecting:value);if m.displayStyle == .optional,let c=m.children.first{return unwrap(c.value)};return value}
private func member(_ object:Any,_ name:String)->Any?{Mirror(reflecting:object).children.first{$0.label==name}.map{unwrap($0.value)}}
private func optionalMemberPresent(_ object:Any,_ name:String)->Bool {
 guard let value=Mirror(reflecting:object).children.first(where:{$0.label==name})?.value else{fatalError("Missing native property \(name)")}
 let mirror=Mirror(reflecting:value);precondition(mirror.displayStyle == .optional)
 return !mirror.children.isEmpty
}
@main struct QuickSettledHarness{
 @MainActor static var presentations=[String]()
 @MainActor static func main(){
  setbuf(stdout,nil)
  _=NSApplication.shared;NSApp.setActivationPolicy(.accessory);NSApp.finishLaunching()
  let defaults=UserDefaults.standard;defaults.set(true,forKey:"unifiedPreferencesSeeded");defaults.set(false,forKey:"soundsEnabled")
  let workspace=NSWindow(contentRect:CGRect(x:100,y:100,width:900,height:600),styleMask:[.titled],backing:.buffered,defer:false);workspace.isReleasedWhenClosed=false
  let runtime=EmbeddedCodeRuntime.shared;runtime.start(window:workspace,command:noop,presentInitialNotch:false)
  guard let app=member(runtime,"app") as? KaiPetApp,let morph=member(app,"quickMorph") as? WorkspaceMorph else{fatalError("Native owner missing")}
  defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
  let beforePresentation=Self.presentations.count;app.launchUnifiedWorkspace()
  precondition(Self.presentations.count>beforePresentation && Self.presentations.last=="visible","Initial visible acknowledgement was not emitted synchronously")
  print("INITIAL_VISIBLE_ACK_SYNCHRONOUS_PASS")
  // Actual native launch guards must decline without publishing any visible
  // acknowledgement. A no-op Rust command leaves menu preparation pending.
  app.setQuickChatVisible(true)
  precondition(member(app,"quickChatVisible") as? Bool == true)
  var declinedCount=Self.presentations.count
  app.launchUnifiedWorkspace()
  precondition(Self.presentations.count==declinedCount,"Visible Quick did not decline the delayed initial workspace launch")
  app.setQuickChatVisible(false)
  precondition(member(app,"quickChatVisible") as? Bool == false)
  app.toggleMenuBarPanel()
  precondition(optionalMemberPresent(app,"quickPrepared"),"Menu command did not leave a real native Quick preparation pending")
  declinedCount=Self.presentations.count
  app.launchUnifiedWorkspace()
  precondition(Self.presentations.count==declinedCount,"Prepared Quick did not decline the delayed initial workspace launch")
  app.setQuickChatVisible(false)
  precondition(!optionalMemberPresent(app,"quickPrepared"),"Public Quick dismissal did not clear pending preparation")
  print("INITIAL_VISIBLE_ACK_REAL_QUICK_DECLINE_PASS visible/prepared guards emit_no_ack cleared_via_public_method")
  let quick=NSWindow(contentRect:CGRect(x:160,y:160,width:520,height:540),styleMask:[.borderless],backing:.buffered,defer:false);quick.isReleasedWhenClosed=false
  quick.contentView=NSView(frame:CGRect(origin:.zero,size:CGSize(width:520,height:540)))
  let host=quick.contentView!.superview ?? quick.contentView!;host.wantsLayer=true
  let sentinel=CAShapeLayer();sentinel.path=CGPath(rect:host.bounds,transform:nil);host.layer!.mask=sentinel
  let originalLevel=quick.level;quick.backgroundColor = .windowBackgroundColor;quick.isOpaque=true;quick.hasShadow=true;quick.ignoresMouseEvents=false
  func wait(_ condition:()->Bool){let deadline=Date().addingTimeInterval(2);while !condition(),Date()<deadline{RunLoop.main.run(until:Date().addingTimeInterval(0.005))};precondition(condition(),"Native Quick completion timed out")}
  precondition(app.unifiedPresentationDiagnostics() & 1024 == 0)
  let reduced=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
  for enabled in [true,false]{
   morph.motionEnabled=enabled
   morph.expand(quick,from:CGRect(x:200,y:600,width:24,height:20),replay:true,destination:quick.frame,sourceNotch:false,crossfade:true,sourceRadius:10,destinationRadius:20){}
   precondition(app.unifiedPresentationDiagnostics() & 1024 != 0 && morph.transitioning)
   if enabled && !reduced{precondition(host.layer!.mask !== sentinel && quick.ignoresMouseEvents,"Animated opening skipped its actual contour")}
   wait{!morph.transitioning}
   precondition(app.unifiedPresentationDiagnostics() & 1024 == 0)
   precondition(host.layer!.mask === sentinel,"Completion retained the clipping mask")
   precondition(!quick.ignoresMouseEvents && quick.level==originalLevel && quick.isOpaque && quick.hasShadow,"Completion did not restore native ownership/interaction")
   print("QUICK_MASK_COMPLETE motion_enabled=\(enabled) system_reduced=\(reduced) restored_previous_mask=true transition_bit=false")
  }
  // A rapid reversal must not let an older completion clear a new contour.
  morph.motionEnabled=true
  morph.expand(quick,from:CGRect(x:200,y:600,width:24,height:20),replay:true,destination:quick.frame,sourceNotch:false){}
  morph.collapse(quick,to:CGRect(x:200,y:600,width:24,height:20),targetNotch:false,retireContent:true,backdrop:false){}
  morph.expand(quick,from:CGRect(x:200,y:600,width:24,height:20),replay:true,destination:quick.frame,sourceNotch:false){}
  wait{!morph.transitioning}
  precondition(host.layer!.mask === sentinel && app.unifiedPresentationDiagnostics() & 1024 == 0)
  for _ in 0..<100{_=app.unifiedPresentationDiagnostics()}
  let count=member(app,"surfaceScrollDiagnosticCount") as? Int
  let logging=CommandLine.arguments.contains("--unified-smoke-test") && CommandLine.arguments.contains("--unified-surface-probe")
  precondition(count==(logging ? 64:0),"Scroll diagnostics escaped mode/count bounds")
  print("QUICK_SETTLED_NATIVE_PASS previous_mask_restore/reduced_motion/reversal/diagnostic_bit/scroll_log_bounds")
 }
}
