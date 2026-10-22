import AppKit

struct NotchSizing {
    static func path(canvas:CGSize,width:CGFloat,height:CGFloat,radius:CGFloat=14)->CGPath {
        let x = (canvas.width-width)/2, r: CGFloat = min(max(0,radius),min(width/2,height/2)), sx: CGFloat = 7, sy: CGFloat = 9, top = canvas.height, bottom = top-height, right = x+width, k = 1-0.5523
        let p = CGMutablePath(); p.move(to: CGPoint(x:x-sx,y:top)); p.addLine(to:CGPoint(x:right+sx,y:top))
        p.addCurve(to:CGPoint(x:right,y:top-sy),control1:CGPoint(x:right+sx*k,y:top),control2:CGPoint(x:right,y:top-sy*k))
        p.addLine(to:CGPoint(x:right,y:bottom+r)); p.addCurve(to:CGPoint(x:right-r,y:bottom),control1:CGPoint(x:right,y:bottom+r*k),control2:CGPoint(x:right-r*k,y:bottom))
        p.addLine(to:CGPoint(x:x+r,y:bottom)); p.addCurve(to:CGPoint(x:x,y:bottom+r),control1:CGPoint(x:x+r*k,y:bottom),control2:CGPoint(x:x,y:bottom+r*k))
        p.addLine(to:CGPoint(x:x,y:top-sy)); p.addCurve(to:CGPoint(x:x-sx,y:top),control1:CGPoint(x:x,y:top-sy*k),control2:CGPoint(x:x-sx*k,y:top)); p.closeSubpath(); return p
    }

    static func updateTextResolution(_ layer:CALayer, scale:CGFloat) {
        if layer is CATextLayer,layer.contentsScale != scale {layer.contentsScale=scale;layer.setNeedsDisplay()}
        if let mask=layer.mask {updateTextResolution(mask,scale:scale)}
        for child in layer.sublayers ?? [] {updateTextResolution(child,scale:scale)}
    }
    static func constrained(_ size: CGSize, baseline: CGSize, screen: CGSize) -> CGSize {
        func clamp(_ value: CGFloat, fallback: CGFloat, maximum: CGFloat) -> CGFloat {
            // NSWindow rounds its frame outward to whole points. Use the same
            // size for the mask and content so their top edge cannot fall short.
            min(floor(maximum), ceil(max(fallback, value.isFinite ? value : fallback)))
        }
        return CGSize(width: clamp(size.width, fallback: baseline.width, maximum: max(baseline.width, screen.width-80)),
                      height: clamp(size.height, fallback: baseline.height, maximum: max(baseline.height, screen.height-80)))
    }
    static func layout(content: CGSize, baseline: CGSize) -> (scale: CGFloat, logical: CGSize) {
        let scale = max(1, min(content.width / max(1,baseline.width), content.height / max(1,baseline.height)))
        return (scale, CGSize(width: content.width/scale, height: content.height/scale))
    }
    static func dragged(_ initial: CGSize, delta: CGPoint, mode: NotchResizeMode) -> CGSize {
        CGSize(width: initial.width + (mode == .height ? 0 : delta.x*2),
               height: initial.height - (mode == .width ? 0 : delta.y))
    }
}

enum NotchResizeMode { case width, height, both }

/// Bottom center changes height, horizontal grips change width, corners change both.
final class NotchResizeHandles: NSView {
    var onBegin: ((NotchResizeMode)->Void)?
    var onDrag: ((CGPoint)->Void)?
    var onEnd: (()->Void)?
    private var origin = CGPoint.zero
    private var direction: CGFloat = 1
    private func mode(at point: CGPoint) -> (NotchResizeMode,CGFloat)? {
        guard bounds.contains(point) else{return nil}
        if point.x < 22 {return (.both,-1)}
        if point.x > bounds.width-22 {return (.both,1)}
        if abs(point.x-bounds.midX)<24 {return (.height,1)}
        if abs(point.x-bounds.width*0.25)<20 {return (.width,-1)}
        if abs(point.x-bounds.width*0.75)<20 {return (.width,1)}
        return nil
    }
    override func hitTest(_ point: NSPoint) -> NSView? {
        isHidden || mode(at:convert(point,from:superview)) == nil ? nil : self
    }
    override func resetCursorRects() {
        addCursorRect(CGRect(x:bounds.midX-24,y:0,width:48,height:bounds.height),cursor:.resizeUpDown)
        for x in [bounds.width*0.25,bounds.width*0.75] {addCursorRect(CGRect(x:x-20,y:0,width:40,height:bounds.height),cursor:.resizeLeftRight)}
        for x in [CGFloat(0),bounds.width-22] {addCursorRect(CGRect(x:x,y:0,width:22,height:bounds.height),cursor:.crosshair)}
    }
    override func mouseDown(with event: NSEvent) {
        guard let (mode,direction)=mode(at:convert(event.locationInWindow,from:nil)) else{return}
        self.direction=direction;origin=NSEvent.mouseLocation;onBegin?(mode)
    }
    override func mouseDragged(with event: NSEvent) {
        let point=NSEvent.mouseLocation
        onDrag?(CGPoint(x:(point.x-origin.x)*direction,y:point.y-origin.y))
    }
    override func mouseUp(with event: NSEvent) {onEnd?()}
    override func draw(_ dirtyRect: NSRect) {
        NSColor.white.withAlphaComponent(0.25).setStroke()
        for x in [bounds.width*0.25,bounds.midX,bounds.width*0.75] {
            let path=NSBezierPath();path.lineWidth=1.5;path.lineCapStyle = .round
            path.move(to:CGPoint(x:x-7,y:7));path.line(to:CGPoint(x:x+7,y:7));path.stroke()
        }
        for (x,sign) in [(CGFloat(8),CGFloat(1)),(bounds.width-8,CGFloat(-1))] {
            let path=NSBezierPath();path.lineWidth=1;path.move(to:CGPoint(x:x,y:10));path.line(to:CGPoint(x:x+sign*5,y:5));path.line(to:CGPoint(x:x+sign*10,y:5));path.stroke()
        }
    }
}
