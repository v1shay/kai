import AppKit

/// Visible inline pets share decoded sheets and animate in Core Animation.
final class InlineTaskPet:NSObject {
    let key:String,image:CGImage,timeline:SpriteTimeline
    init(key:String,image:CGImage,timeline:SpriteTimeline){self.key=key;self.image=image;self.timeline=timeline}
    func apply(to layer:CALayer){
        guard layer.name != key else{return};layer.name=key;layer.contents=image;layer.contentsGravity = .resizeAspect;layer.magnificationFilter = .linear;layer.contentsRect=timeline.frames[0];layer.removeAnimation(forKey:"inlinePet")
        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{layer.addKai(timeline.animation(),forKey:"inlinePet")}
    }
}
