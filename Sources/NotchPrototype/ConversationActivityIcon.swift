import AppKit

/// Reuses the native vector poses that generate the handoff status SVG artwork.
/// Cache one static drawing per icon; transcript activity needs no animation.
@MainActor enum ConversationActivityIcon {
    private static var cache=[String:NSImage]()
    static func image(_ id:String)->NSImage {
        if let image=cache[id]{return image}
        let scene:IndicatorScene
        switch id {case "terminal":scene=IndicatorScenes.terminal;case "image":scene=IndicatorScenes.image;case "files":scene=IndicatorScenes.files;case "search":scene=IndicatorScenes.search;default:scene=IndicatorScenes.tool}
        let poses=scene.firstPoses
        let image=NSImage(size:CGSize(width:14,height:9),flipped:false){_ in
            guard let context=NSGraphicsContext.current?.cgContext else{return false}
            context.saveGState();defer{context.restoreGState()}
            context.scaleBy(x:0.5,y:0.5)
            for pose in poses where pose.opacity>0 {
                context.saveGState()
                context.translateBy(x:pose.x,y:pose.y);context.rotate(by:pose.rotation)
                let rect=CGRect(x:-pose.width/2,y:-pose.height/2,width:pose.width,height:pose.height)
                let color=NSColor.white.withAlphaComponent(0.42*pose.opacity).cgColor
                if pose.shape == .ellipse {
                    context.setStrokeColor(color);context.setLineWidth(pose.lineWidth)
                    context.strokeEllipse(in:rect.insetBy(dx:pose.lineWidth/2,dy:pose.lineWidth/2))
                } else {
                    context.setFillColor(color)
                    context.addPath(CGPath(roundedRect:rect,cornerWidth:pose.radius,cornerHeight:pose.radius,transform:nil));context.fillPath()
                }
                context.restoreGState()
            }
            return true
        }
        cache[id]=image;return image
    }
}
