import AppKit

/// Interruptible artwork swaps. Delayed cleanup never hides a newer status.
@MainActor final class NotchVisualTransition {
    private var outgoing:CALayer?
    private var revision=0
    private let curve=CAMediaTimingFunction(controlPoints:0.16,0.85,0.22,1)
    private var duration:Double {NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.16:0.48}

    func artwork(_ image:CGImage,on layer:CALayer) {
        revision += 1;let token=revision;outgoing?.removeFromSuperlayer();outgoing=nil
        if layer.contents != nil,let parent=layer.superlayer,!layer.isHidden,layer.opacity > 0 {
            let previous=CALayer();previous.frame=layer.presentation()?.frame ?? layer.frame
            previous.contents=layer.contents;previous.contentsGravity=layer.contentsGravity
            previous.cornerRadius=layer.cornerRadius;previous.masksToBounds=true
            parent.insertSublayer(previous,below:layer);outgoing=previous
            reveal(previous,visible:false)
        }
        CATransaction.begin();CATransaction.setDisableActions(true)
        layer.contents=image;layer.isHidden=false;layer.opacity=0;CATransaction.commit()
        reveal(layer,visible:true,fromOpacity:0)
        DispatchQueue.main.asyncAfter(deadline:.now()+duration){[weak self] in
            guard let self,self.revision == token else{return};self.outgoing?.removeFromSuperlayer();self.outgoing=nil
        }
    }
    func reveal(_ layer:CALayer,visible:Bool,fromOpacity:Float?=nil) {
        let opacity=fromOpacity ?? layer.presentation()?.opacity ?? layer.opacity
        let target:Float=visible ? 1:0
        let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=opacity;fade.toValue=target;fade.duration=duration;fade.timingFunction=curve
        CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=target;CATransaction.commit()
        layer.add(fade,forKey:"statusFade")
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else{return}
        let scale=CABasicAnimation(keyPath:"transform.scale");scale.fromValue=fromOpacity != nil ? 0.92:layer.presentation()?.value(forKeyPath:"transform.scale") ?? (visible ? 0.86:1);scale.toValue=visible ? 1:0.86;scale.duration=duration;scale.timingFunction=curve;layer.add(scale,forKey:"statusScale")
        let drift=CABasicAnimation(keyPath:"transform.translation.y");drift.fromValue=layer.presentation()?.value(forKeyPath:"transform.translation.y") ?? (visible ? -3:0);drift.toValue=visible ? 0:3;drift.duration=duration;drift.timingFunction=curve;layer.add(drift,forKey:"statusDrift")
    }
    func hideArtwork(_ layer:CALayer) {
        revision += 1;let token=revision;outgoing?.removeFromSuperlayer();outgoing=nil
        reveal(layer,visible:false)
        DispatchQueue.main.asyncAfter(deadline:.now()+duration){[weak self,weak layer] in
            guard let self,self.revision == token else{return};layer?.isHidden=true;layer?.contents=nil
        }
    }
    func resize(_ layer:CALayer,to frame:CGRect,animated:Bool) {
        let previous=layer.presentation()?.frame ?? layer.frame
        CATransaction.begin();CATransaction.setDisableActions(true);layer.frame=frame;layer.cornerRadius=min(8,frame.height*0.22);CATransaction.commit()
        guard animated else{return}
        let bounds=CABasicAnimation(keyPath:"bounds");bounds.fromValue=CGRect(origin:.zero,size:previous.size);bounds.toValue=layer.bounds;bounds.duration=0.22;bounds.timingFunction=curve;layer.add(bounds,forKey:"mediaResize")
        let position=CABasicAnimation(keyPath:"position");position.fromValue=CGPoint(x:previous.midX,y:previous.midY);position.toValue=layer.position;position.duration=0.22;position.timingFunction=curve;layer.add(position,forKey:"mediaResizePosition")
    }
    nonisolated static func artworkFrame(size:CGFloat,notchHeight:CGFloat,center:CGPoint,scale:CGFloat)->CGRect {
        let requested=size.isFinite ? size:32
        let side=min(max(16,requested),max(16,notchHeight-3)),pixels=max(1,scale)
        func px(_ value:CGFloat)->CGFloat{round(value*pixels)/pixels}
        return CGRect(x:px(center.x-side/2),y:px(center.y-side/2),width:px(side),height:px(side))
    }
}
