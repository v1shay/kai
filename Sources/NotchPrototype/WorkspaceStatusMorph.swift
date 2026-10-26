import AppKit

/// A short contour echo joins the background workspace to its live status.
/// The real Code window is retained behind the user's application throughout.
@MainActor final class WorkspaceStatusMorph {
    private var panel:NSPanel?,retire:DispatchWorkItem?
    private(set) var active=false
    func cancel(){retire?.cancel();retire=nil;panel?.orderOut(nil);panel=nil;active=false}
    func show(from source:CGRect,to target:CGRect,motion:Bool){
        cancel()
        guard motion,!NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else{return}
        let frame=source.union(target)
        let panel=NSPanel(contentRect:frame,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=false;panel.ignoresMouseEvents=true
        panel.level=NSWindow.Level(rawValue:NSWindow.Level.statusBar.rawValue-1)
        panel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary,.stationary]
        let view=NSView(frame:CGRect(origin:.zero,size:frame.size));view.wantsLayer=true;panel.contentView=view
        let layer=CAShapeLayer();layer.frame=view.bounds;layer.fillColor=NSColor.black.cgColor
        let start=WorkspaceMorph.outline(source.offsetBy(dx:-frame.minX,dy:-frame.minY),notch:false,radius:10)
        let end=WorkspaceMorph.outline(target.offsetBy(dx:-frame.minX,dy:-frame.minY),notch:true,radius:14)
        layer.path=end;layer.opacity=0;view.layer?.addSublayer(layer)
        let shape=CABasicAnimation(keyPath:"path");shape.fromValue=start;shape.toValue=end;shape.duration=0.48
        shape.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.84,0.20,1)
        let fade=CAKeyframeAnimation(keyPath:"opacity");fade.values=[0,0.20,0.12,0];fade.keyTimes=[0,0.12,0.65,1];fade.duration=0.48
        layer.add(shape,forKey:"statusContour");layer.add(fade,forKey:"statusFade")
        self.panel=panel;active=true;panel.orderFrontRegardless()
        let finish=DispatchWorkItem{[weak self] in self?.cancel()};retire=finish
        DispatchQueue.main.asyncAfter(deadline:.now()+0.50,execute:finish)
    }
}
