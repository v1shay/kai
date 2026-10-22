import AppKit
import ImageIO

struct PetGalleryMotion {
    let row:Int, columns:[Int], durationsMs:[Int], loop:Bool
}

struct PetGallerySpec {
    let id:String, spritesheet:String, rows:Int, motions:[String:PetGalleryMotion], colors:[CGColor]
}

@MainActor final class PetGallery:NSObject {
    private final class Entry {
        let spec:PetGallerySpec, glow=CAGradientLayer(), sprite=CALayer(), ring=CAShapeLayer()
        var cell=CGRect.zero
        init(_ spec:PetGallerySpec){self.spec=spec}
    }

    let layer=CALayer()
    private var root:URL?
    private let thumbnails=SpriteAssetCache(maxPixelSize:960)
    private var loadGeneration=UUID()
    private var entries=[Entry](), motionID="idle", hovered:String?
    private let curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1)
    private(set) var visible=false

    override init(){super.init();layer.opacity=0;layer.isHidden=true}

    func configure(_ specs:[PetGallerySpec],at root:URL) {
        self.root=root;loadGeneration=UUID()
        entries.forEach{$0.glow.removeFromSuperlayer();$0.sprite.removeFromSuperlayer();$0.ring.removeFromSuperlayer()}
        entries=specs.map{spec in
            let entry=Entry(spec)
            entry.glow.type = .radial;entry.glow.startPoint=CGPoint(x:0.5,y:0.5);entry.glow.endPoint=CGPoint(x:1,y:1)
            let colors=spec.colors.isEmpty ? [NSColor.white.cgColor] : spec.colors
            entry.glow.colors=[colors.first!,colors[colors.count/2],colors.last!,NSColor.clear.cgColor]
            entry.glow.locations=[0,0.3,0.62,1];entry.glow.opacity=0.2
            entry.sprite.contentsGravity = .resizeAspect;entry.sprite.magnificationFilter = .nearest;entry.sprite.contentsScale=NSScreen.main?.backingScaleFactor ?? 2
            entry.ring.fillColor=nil;entry.ring.strokeColor=colors.last;entry.ring.lineWidth=1;entry.ring.opacity=0
            layer.addSublayer(entry.glow);layer.addSublayer(entry.ring);layer.addSublayer(entry.sprite)
            return entry
        }
    }

    func layout(in bounds:CGRect,selected:String) {
        layer.bounds=CGRect(origin:.zero,size:bounds.size)
        let columns=6,rows=max(1,Int(ceil(Double(entries.count)/Double(columns)))),cellWidth=bounds.width/CGFloat(columns),cellHeight=bounds.height/CGFloat(rows)
        CATransaction.begin();CATransaction.setDisableActions(true)
        for (index,entry) in entries.enumerated() {
            let column=index%columns,row=index/columns,cell=CGRect(x:CGFloat(column)*cellWidth,y:bounds.height-CGFloat(row+1)*cellHeight,width:cellWidth,height:cellHeight)
            entry.cell=cell
            let size=min(cell.width*0.72,cell.height*0.82),spriteSize=CGSize(width:size*192/208,height:size)
            entry.glow.frame=cell.insetBy(dx:3,dy:2)
            entry.sprite.frame=CGRect(x:cell.midX-spriteSize.width/2,y:cell.midY-spriteSize.height/2,width:spriteSize.width,height:spriteSize.height)
            let ringSize=max(spriteSize.width,spriteSize.height)+5
            entry.ring.frame=CGRect(x:cell.midX-ringSize/2,y:cell.midY-ringSize/2,width:ringSize,height:ringSize);entry.ring.path=CGPath(ellipseIn:entry.ring.bounds,transform:nil);entry.ring.opacity=entry.spec.id == selected ? 0.5:0
        }
        CATransaction.commit()
    }

    func show(selected:String,motion:String) {
        visible=true
        let generation=UUID();loadGeneration=generation
        if let root {
            for entry in entries where entry.sprite.contents == nil {
                thumbnails.load(root.appendingPathComponent(entry.spec.spritesheet)) { [weak self,weak entry] image in
                    guard let self,let entry,self.visible,self.loadGeneration == generation,let image else{return}
                    CATransaction.begin();CATransaction.setDisableActions(true);entry.sprite.contents=image;CATransaction.commit()
                    let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=0;fade.toValue=1;fade.duration=0.18;entry.sprite.add(fade,forKey:"loaded")
                }
            }
        }
        visible=true;layer.isHidden=false;play(motion)
        let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? 0;fade.toValue=1;fade.duration=0.32;fade.timingFunction=curve
        CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=1;CATransaction.commit();layer.add(fade,forKey:"galleryReveal")
        let now=CACurrentMediaTime()
        for (index,entry) in entries.enumerated() {
            entry.ring.opacity=entry.spec.id == selected ? 0.5:0
            let appear=CAAnimationGroup(),opacity=CABasicAnimation(keyPath:"opacity"),scale=CABasicAnimation(keyPath:"transform.scale"),rise=CABasicAnimation(keyPath:"transform.translation.y")
            opacity.fromValue=0;opacity.toValue=1;scale.fromValue=0.72;scale.toValue=1;rise.fromValue = -5;rise.toValue=0
            appear.animations=[opacity,scale,rise];appear.duration=0.36;appear.beginTime=now+Double(index%6)*0.025+Double(index/6)*0.035;appear.fillMode = .backwards;appear.timingFunction=curve
            entry.sprite.add(appear,forKey:"galleryPetReveal");entry.glow.add(appear,forKey:"galleryGlowReveal")
        }
    }

    func hide() {
        guard visible else{return};visible=false;hovered=nil;entries.forEach{$0.sprite.removeAnimation(forKey:"spriteFrames")}
        let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? 1;fade.toValue=0;fade.duration=0.24;fade.timingFunction=curve
        CATransaction.begin();CATransaction.setDisableActions(true);layer.opacity=0;CATransaction.commit();layer.add(fade,forKey:"galleryHide")
        DispatchQueue.main.asyncAfter(deadline:.now()+0.24){[weak self] in guard let self,!self.visible else{return};self.layer.isHidden=true;CATransaction.begin();CATransaction.setDisableActions(true);self.entries.forEach{$0.sprite.contents=nil};CATransaction.commit()}
    }

    func play(_ id:String) {
        motionID=id
        guard visible else{return}
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        for entry in entries {
            entry.sprite.removeAnimation(forKey:"spriteFrames")
            guard let motion=entry.spec.motions[id],let timeline=SpriteTimeline(row:motion.row,rows:entry.spec.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop) else{continue}
            let reduceMotion=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
            entry.sprite.contentsRect=motion.loop || reduceMotion ? timeline.frames[0]:timeline.frames[timeline.frames.count-1]
            if !reduceMotion,timeline.frames.count>1 {entry.sprite.add(timeline.animation(),forKey:"spriteFrames")}
        }
    }

    func pet(at point:CGPoint)->String?{NotchInputRegion.nearestTarget(at:point,frames:entries.map(\.cell),padding:4).map{entries[$0].spec.id}}

    func hover(at point:CGPoint?) {
        let id=point.flatMap{pet(at:$0)};guard id != hovered else{return};hovered=id
        for entry in entries {
            let active=entry.spec.id == id,scale:CGFloat=active ? 1.13:1,targetGlow:Float=active ? 0.34:0.2
            let transform=CABasicAnimation(keyPath:"transform.scale");transform.fromValue=entry.sprite.presentation()?.transform.m11 ?? entry.sprite.transform.m11;transform.toValue=scale;transform.duration=0.22;transform.timingFunction=curve
            let glow=CABasicAnimation(keyPath:"opacity");glow.fromValue=entry.glow.presentation()?.opacity ?? entry.glow.opacity;glow.toValue=targetGlow;glow.duration=0.22;glow.timingFunction=curve
            CATransaction.begin();CATransaction.setDisableActions(true);entry.sprite.transform=CATransform3DMakeScale(scale,scale,1);entry.glow.opacity=targetGlow;CATransaction.commit();entry.sprite.add(transform,forKey:"galleryHover");entry.glow.add(glow,forKey:"galleryHover")
        }
    }

}
