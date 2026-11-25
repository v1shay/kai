import AppKit

@MainActor private final class AuditBackdrop:NSVisualEffectView {
    static var copies=0
    @objc var kaiBackdropRadius:Int=37
    @objc func copyKaiTransitionBackdrop()->NSView{
        Self.copies+=1
        let copy=AuditBackdrop(frame:.zero);copy.kaiBackdropRadius=kaiBackdropRadius
        copy.material=material;copy.blendingMode=blendingMode;copy.state=state
        return copy
    }
}
@MainActor private final class AuditWindow:NSWindow {
    var resizeCount=0
    override func setFrame(_ frameRect:NSRect,display flag:Bool){resizeCount+=1;super.setFrame(frameRect,display:flag)}
}
private func member(_ object:Any,_ key:String)->Any?{
    guard let value=Mirror(reflecting:object).children.first(where:{$0.label==key})?.value else{return nil}
    let mirror=Mirror(reflecting:value);return mirror.displayStyle == .optional ? mirror.children.first?.value:value
}
@main struct GlassContinuityAudit {
 @MainActor static func main(){
    _=NSApplication.shared;NSApp.setActivationPolicy(.accessory)
    let frame=CGRect(x:120,y:120,width:980,height:680)
    let window=AuditWindow(contentRect:frame,styleMask:[.borderless],backing:.buffered,defer:false)
    window.isReleasedWhenClosed=false;window.isOpaque=false;window.backgroundColor = .clear
    let root=NSView(frame:CGRect(origin:.zero,size:frame.size));root.wantsLayer=true;window.contentView=root
    let backdrop=AuditBackdrop(frame:root.bounds);backdrop.identifier=NSUserInterfaceItemIdentifier("kai.workspace-backdrop")
    backdrop.material = .underWindowBackground;backdrop.blendingMode = .behindWindow;backdrop.state = .active;backdrop.wantsLayer=true
    let tint=NSView(frame:root.bounds);tint.identifier=NSUserInterfaceItemIdentifier("kai.workspace-tint");tint.wantsLayer=true;tint.layer?.backgroundColor=NSColor.black.withAlphaComponent(0.3).cgColor
    let renderer=NSView(frame:root.bounds),pet=NSView(frame:CGRect(x:0,y:0,width:20,height:20))
    renderer.wantsLayer=true;pet.wantsLayer=true
    [backdrop,tint,renderer,pet].forEach{root.addSubview($0)}
    let morph=WorkspaceMorph(),notch=CGRect(x:400,y:760,width:400,height:64)
    func pump(_ seconds:Double){let end=Date().addingTimeInterval(seconds);while Date()<end{RunLoop.main.run(until:Date().addingTimeInterval(0.005))}}
    func checkMaterial(){precondition(backdrop.layer?.opacity==1 && tint.layer?.opacity==1 && root.layer?.opacity==1,"Morph faded the native glass instead of the foreground content")}
    let initialFrame=window.frame;window.resizeCount=0
    morph.expand(window,from:notch,destination:initialFrame)
    checkMaterial();precondition(window.frame==initialFrame && window.resizeCount==0,"Warm reveal synchronously resized its workspace")
    let shell=member(morph,"shell") as! NSPanel
    precondition(shell.frame.height<=24.1 && shell.frame.intersection(window.frame).height<=0,"Bridge covered the live window instead of just its external menu-bar gap")
    let bridge=member(morph,"glassBridge") as! WorkspaceGlassBridge
    let nativeCopy=(member(bridge,"surface") as! NSView).subviews.first as! AuditBackdrop
    precondition(nativeCopy.kaiBackdropRadius==37 && nativeCopy.material==backdrop.material && AuditBackdrop.copies==1,"Bridge did not copy the settled material/filter profile")
    pump(0.9);checkMaterial();precondition(!shell.isVisible && window.isVisible)
    morph.collapse(window,to:notch,retireContent:true,completion:{})
    checkMaterial();precondition(renderer.layer?.opacity==0 && pet.layer?.opacity==0,"GPU content and native pet did not retire together")
    precondition((member(morph,"shell") as! NSPanel) === shell && AuditBackdrop.copies==1,"Warm collapse recreated its blur surface")
    pump(0.2);checkMaterial();precondition(window.isVisible,"Window retired before its existing morph deadline")
    morph.expand(window,from:notch,destination:initialFrame)
    checkMaterial();precondition(AuditBackdrop.copies==1,"Rapid reversal recreated an unchanged material")
    pump(0.9);precondition(renderer.layer?.opacity==1 && pet.layer?.opacity==1 && !morph.transitioning)
    backdrop.kaiBackdropRadius=12
    morph.collapse(window,to:notch,retireContent:true,completion:{})
    precondition(AuditBackdrop.copies==2,"A changed blur profile reused a stale bridge")
    morph.cancel();precondition(renderer.layer?.opacity==1 && !shell.isVisible && !window.ignoresMouseEvents)
    // Fixed exclusion remains correct even for L-shaped external regions.
    let union=CGRect(x:100,y:100,width:1200,height:1000)
    bridge.configure(window:window,frame:union,opaqueColor:.black)
    let exclusion=member(bridge,"exclusion") as! CAShapeLayer
    let inside=CGPoint(x:window.frame.midX-union.minX,y:window.frame.midY-union.minY)
    precondition(exclusion.path?.contains(inside,using:.evenOdd)==false,"Bridge stacked glass beneath the workspace")
    precondition(exclusion.path?.contains(CGPoint(x:1190,y:990),using:.evenOdd)==true,"Bridge lost the external contour")
    // Opaque mode keeps its color while its content fades, then restores it.
    window.orderOut(nil);let opaque=AuditWindow(contentRect:frame,styleMask:[.borderless],backing:.buffered,defer:false)
    opaque.isReleasedWhenClosed=false;opaque.isOpaque=true;opaque.backgroundColor = .darkGray
    opaque.contentView=NSView(frame:CGRect(origin:.zero,size:frame.size));opaque.contentView?.wantsLayer=true
    let plain=WorkspaceMorph();plain.retainOpen(opaque);plain.collapse(opaque,to:notch,retireContent:true,completion:{})
    precondition((member(plain,"opaqueFill") as? CALayer)?.superlayer != nil && opaque.contentView?.layer?.opacity==0,"Opaque mode lost its solid material during retirement")
    plain.cancel();precondition(opaque.isOpaque && opaque.backgroundColor == .darkGray && opaque.contentView?.layer?.opacity==1)
    morph.motionEnabled=false;morph.expand(window,from:notch,destination:initialFrame);pump(0.16)
    precondition(!morph.transitioning && !shell.isVisible && renderer.layer?.opacity==1,"Reduced motion retained a glass bridge or hidden content")
    morph.park(window);plain.park(opaque);NSApp.windows.forEach{$0.orderOut(nil)}
    print("Continuous glass: live material, gap-only bridge, cache reuse, profile refresh, fixed frames, reversals, opaque/reduced-motion restore passed")
 }
}
