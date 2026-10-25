import AppKit

/// Animates the real IDE window. No second renderer or persistent screenshot.
/// Reversals begin at the current frame and stale completions cannot hide it.
@MainActor final class WorkspaceMorph {
    private var revision=0
    private var transitioning=false
    private var expandedFrame:CGRect?
    private weak var window:NSWindow?
    private var originalMinimum:CGSize?
    private(set) var expanded=false
    nonisolated static func destination(saved:CGRect?,screen:CGRect)->CGRect {
        let usable=screen.insetBy(dx:24,dy:24)
        let requested=saved ?? CGRect(x:usable.midX-600,y:usable.midY-390,width:1200,height:780)
        let width=min(max(940,requested.width),usable.width),height=min(max(620,requested.height),usable.height)
        return CGRect(x:min(max(requested.minX,usable.minX),usable.maxX-width),y:min(max(requested.minY,usable.minY),usable.maxY-height),width:width,height:height)
    }
    func expand(_ window:NSWindow,from source:CGRect) {
        revision += 1;let token=revision
        let wasExpanded=expanded,wasTransitioning=transitioning;expanded=true;self.window=window
        if originalMinimum == nil{originalMinimum=window.minSize}
        let screen=window.screen ?? NSScreen.main
        let target=Self.destination(saved:expandedFrame ?? window.frame,screen:screen?.visibleFrame ?? window.frame)
        window.minSize = .zero
        expandedFrame=target
        if !wasExpanded && !wasTransitioning{window.setFrame(source,display:false);window.alphaValue=0}
        transitioning=true
        window.orderFront(nil);NSApp.activate(ignoringOtherApps:true)
        NSAnimationContext.runAnimationGroup{context in
            context.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:0.46
            context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.85,0.22,1)
            window.animator().setFrame(target,display:true);window.animator().alphaValue=1
        } completionHandler:{[weak self,weak window] in
            MainActor.assumeIsolated {
                guard let self,self.revision == token,let window else{return}
                self.transitioning=false;window.minSize=self.originalMinimum ?? .zero;window.makeKeyAndOrderFront(nil)
            }
        }
    }
    func collapse(_ window:NSWindow,to target:CGRect,completion:@escaping()->Void) {
        revision += 1;let token=revision
        if !transitioning && (expanded || window.isVisible){expandedFrame=window.frame}
        expanded=false;transitioning=true;self.window=window
        if originalMinimum == nil{originalMinimum=window.minSize}
        window.minSize = .zero
        NSAnimationContext.runAnimationGroup{context in
            context.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:0.38
            context.timingFunction=CAMediaTimingFunction(controlPoints:0.18,0.8,0.25,1)
            window.animator().setFrame(target,display:true);window.animator().alphaValue=0
        } completionHandler:{[weak self,weak window] in
            MainActor.assumeIsolated {
                guard let self,self.revision == token,let window else{return}
                self.transitioning=false;window.orderOut(nil);window.alphaValue=1
                if let frame=self.expandedFrame{window.setFrame(frame,display:false)}
                window.minSize=self.originalMinimum ?? .zero;completion()
            }
        }
    }
    func cancel(){revision += 1;transitioning=false;if let window{window.alphaValue=1;window.minSize=originalMinimum ?? .zero}}
}
