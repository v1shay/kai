import AppKit

/// Window routing matters: returning nil from NSView.hitTest does not pass a
/// click through a transparent NSWindow to another application.
enum NotchInputRegion {
    /// Visible controls win over padded neighbors. In the space between
    /// controls, choose the nearest edge, then the nearest center.
    static func nearestTarget(at point:CGPoint,frames:[CGRect],padding:CGFloat=6,limits:[CGRect]?=nil)->Int? {
        var best:(index:Int,inside:Bool,edge:CGFloat,center:CGFloat)?
        for (index,frame) in frames.enumerated() where frame.width>0 && frame.height>0 {
            var hit=frame.insetBy(dx:-padding,dy:-padding)
            if let limits,limits.indices.contains(index){hit=hit.intersection(limits[index])}
            guard !hit.isNull,hit.contains(point) else{continue}
            let inside=frame.contains(point)
            let dx=max(frame.minX-point.x,0,point.x-frame.maxX),dy=max(frame.minY-point.y,0,point.y-frame.maxY)
            let edge=dx*dx+dy*dy,center=pow(point.x-frame.midX,2)+pow(point.y-frame.midY,2)
            if let prior=best {
                if prior.inside && !inside{continue}
                if prior.inside == inside && (edge>prior.edge || (edge == prior.edge && center>=prior.center)){continue}
            }
            best=(index,inside,edge,center)
        }
        return best?.index
    }
    static func mainAcceptsInput(expanded:Bool,web:Bool)->Bool {expanded || web}
    static func compactAcceptsInput(open:Bool,expanded:Bool,web:Bool,working:Bool,voice:Bool=false)->Bool {
        open && !expanded && !web && (working || voice)
    }
    static func compactFrame(screen:CGRect,center:CGFloat,width:CGFloat,height:CGFloat)->CGRect {
        CGRect(x:center-width/2,y:screen.maxY-height,width:width,height:height)
    }
}
