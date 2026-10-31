import AppKit

struct PetGalleryMotion {let row:Int,columns:[Int],durationsMs:[Int],loop:Bool}
struct PetGallerySpec {
    let id:String,spritesheet:String,rows:Int,motions:[String:PetGalleryMotion],colors:[CGColor]
    var sourceURL:URL?=nil
    var category:String="Originals"
    var displayName:String=""
}

@MainActor final class PetGallery:NSObject {
    private final class Entry {
        let spec:PetGallerySpec,glow=CAGradientLayer(),sprite=CALayer(),ring=CAShapeLayer(),label=CATextLayer()
        var cell=CGRect.zero
        init(_ spec:PetGallerySpec){self.spec=spec}
    }
    let layer=CALayer()
    private var root:URL?,entries=[Entry](),shown=[Entry](),motionID="idle",hovered:String?
    private let thumbnails=SpriteAssetCache(maxPixelSize:960),curve=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1)
    private var loadGeneration=UUID(),category="All",page=0,lastSelected=""
    private let pageSize=12,columns=6
    private var tabs=[CATextLayer](),tabFrames=[CGRect](),navigation=[CGRect](),footer=CATextLayer()
    private(set) var visible=false
    override init(){super.init();layer.opacity=0;layer.isHidden=true;for _ in 0...PetCategory.allCases.count{let label=CATextLayer();label.alignmentMode = .center;label.font=AppTypography.ui(ofSize:9,weight:.medium);label.fontSize=9;label.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;layer.addSublayer(label);tabs.append(label)};footer.font=AppTypography.ui(ofSize:9);footer.fontSize=9;footer.alignmentMode = .center;footer.foregroundColor=NSColor.secondaryLabelColor.cgColor;footer.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;layer.addSublayer(footer)}
    private var categories:[String]{["All"]+PetCategory.allCases.map(\.rawValue)}
    private var filtered:[Entry]{entries.filter{category=="All" || $0.spec.category==category}}
    func configure(_ specs:[PetGallerySpec],at root:URL){
        self.root=root;loadGeneration=UUID();thumbnails.invalidate()
        for entry in entries{for item in [entry.glow,entry.sprite,entry.ring,entry.label]{item.removeFromSuperlayer()}}
        entries=specs.sorted{($0.displayName.isEmpty ? $0.id:$0.displayName).localizedStandardCompare($1.displayName.isEmpty ? $1.id:$1.displayName) == .orderedAscending}.map{spec in
            let entry=Entry(spec),colors=spec.colors.isEmpty ? [NSColor.white.cgColor]:spec.colors
            entry.glow.type = .radial;entry.glow.startPoint=CGPoint(x:0.5,y:0.5);entry.glow.endPoint=CGPoint(x:1,y:1);entry.glow.colors=[colors.first!,colors[colors.count/2],colors.last!,NSColor.clear.cgColor];entry.glow.locations=[0,0.3,0.62,1];entry.glow.opacity=0.2
            entry.sprite.name="gallery-pet:"+spec.id;entry.sprite.contentsGravity = .resizeAspect;entry.sprite.magnificationFilter = .nearest
            entry.ring.fillColor=nil;entry.ring.strokeColor=colors.last;entry.ring.lineWidth=1;entry.ring.opacity=0
            entry.label.font=AppTypography.ui(ofSize:9);entry.label.fontSize=9;entry.label.alignmentMode = .center;entry.label.truncationMode = .end;entry.label.foregroundColor=NSColor.white.withAlphaComponent(0.75).cgColor;entry.label.string=spec.displayName.isEmpty ? spec.id:spec.displayName
            for item in [entry.glow,entry.sprite,entry.ring,entry.label]{item.contentsScale=NSScreen.main?.backingScaleFactor ?? 2;layer.addSublayer(item)}
            return entry
        }
        page=min(page,max(0,(filtered.count-1)/pageSize));shown=[]
    }
    func layout(in bounds:CGRect,selected:String){
        layer.bounds=CGRect(origin:.zero,size:bounds.size)
        let candidates=filtered
        if selected != lastSelected {lastSelected=selected;if let index=candidates.firstIndex(where:{$0.spec.id==selected}){page=index/pageSize}}
        page=min(page,max(0,(candidates.count-1)/pageSize));shown=Array(candidates.dropFirst(page*pageSize).prefix(pageSize))
        let ids=Set(shown.map{$0.spec.id}),cellWidth=bounds.width/CGFloat(columns),cellHeight=max(20,(bounds.height-56)/2)
        CATransaction.begin();CATransaction.setDisableActions(true)
        tabFrames=categories.indices.map{CGRect(x:CGFloat($0)*bounds.width/CGFloat(tabs.count),y:bounds.height-26,width:bounds.width/CGFloat(tabs.count),height:24)}
        for (i,title) in categories.enumerated(){tabs[i].frame=tabFrames[i].insetBy(dx:2,dy:4);tabs[i].string=title;tabs[i].foregroundColor=NSColor.white.withAlphaComponent(title==category ? 1:0.45).cgColor}
        for entry in entries where !ids.contains(entry.spec.id){entry.cell = .zero;for item in [entry.glow,entry.sprite,entry.ring,entry.label]{item.isHidden=true};entry.sprite.removeAllAnimations();entry.sprite.contents=nil}
        for (index,entry) in shown.enumerated(){
            let cell=CGRect(x:CGFloat(index%columns)*cellWidth,y:bounds.height-29-CGFloat(index/columns+1)*cellHeight,width:cellWidth,height:cellHeight);entry.cell=cell
            if hovered != entry.spec.id{entry.sprite.setAffineTransform(.identity)}
            let height=max(8,min(cell.width*0.83,cell.height-24)),spriteSize=CGSize(width:height*192/208,height:height)
            for item in [entry.glow,entry.sprite,entry.ring,entry.label]{item.isHidden=false}
            entry.glow.frame=cell.insetBy(dx:3,dy:2);entry.sprite.frame=CGRect(x:cell.midX-spriteSize.width/2,y:cell.midY-spriteSize.height/2+7,width:spriteSize.width,height:spriteSize.height)
            entry.ring.frame=entry.sprite.frame.insetBy(dx:-3,dy:-3);entry.ring.path=CGPath(ellipseIn:entry.ring.bounds,transform:nil);entry.ring.opacity=entry.spec.id==selected ? 0.5:0
            entry.label.frame=CGRect(x:cell.minX+2,y:cell.minY+2,width:cell.width-4,height:14)
        }
        let pages=max(1,(candidates.count+pageSize-1)/pageSize)
        footer.string="‹     \(page+1) / \(pages) · \(candidates.count) pets     ›";footer.frame=CGRect(x:40,y:5,width:max(0,bounds.width-80),height:16)
        navigation=[CGRect(x:0,y:0,width:bounds.width/2,height:24),CGRect(x:bounds.width/2,y:0,width:bounds.width/2,height:24)]
        CATransaction.commit()
    }
    func show(selected:String,motion:String){
        visible=true;layer.isHidden=false;loadGeneration=UUID();let generation=loadGeneration
        if let root{for entry in shown where entry.sprite.contents==nil{thumbnails.load(entry.spec.sourceURL ?? root.appendingPathComponent(entry.spec.spritesheet)){[weak self,weak entry] image in guard let self,let entry,self.visible,self.loadGeneration==generation,let image else{return};CATransaction.begin();CATransaction.setDisableActions(true);entry.sprite.contents=image;CATransaction.commit();if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=0;fade.toValue=1;fade.duration=0.18;entry.sprite.add(fade,forKey:"loaded")}}}}
        play(motion)
        let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? 0;fade.toValue=1;fade.duration=0.25;fade.timingFunction=curve;layer.opacity=1;layer.add(fade,forKey:"galleryReveal")
        guard !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion else{return}
        for (index,entry) in shown.enumerated(){let appear=CAAnimationGroup(),opacity=CABasicAnimation(keyPath:"opacity"),scale=CABasicAnimation(keyPath:"transform.scale");opacity.fromValue=0;opacity.toValue=1;scale.fromValue=0.86;scale.toValue=1;appear.animations=[opacity,scale];appear.duration=0.28;appear.beginTime=CACurrentMediaTime()+Double(index%6)*0.025;appear.fillMode = .backwards;appear.timingFunction=curve;entry.sprite.add(appear,forKey:"galleryPetReveal")}
    }
    func hide(){guard visible else{return};visible=false;hovered=nil;loadGeneration=UUID();for entry in shown{entry.sprite.removeAllAnimations()};let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=layer.presentation()?.opacity ?? 1;fade.toValue=0;fade.duration=0.24;fade.timingFunction=curve;layer.opacity=0;layer.add(fade,forKey:"galleryHide");DispatchQueue.main.asyncAfter(deadline:.now()+0.24){[weak self] in guard let self,!self.visible else{return};self.layer.isHidden=true;for entry in self.entries{entry.sprite.contents=nil};self.thumbnails.invalidate()}}
    func play(_ id:String){motionID=id;guard visible else{return};CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()};for entry in shown{entry.sprite.removeAnimation(forKey:"spriteFrames");guard let motion=entry.spec.motions[id],let timeline=SpriteTimeline(row:motion.row,rows:entry.spec.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop)else{continue};let reduce=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion;entry.sprite.contentsRect=motion.loop || reduce ? timeline.frames[0]:timeline.frames.last!;if !reduce,timeline.frames.count>1{entry.sprite.add(timeline.animation(),forKey:"spriteFrames")}}}
    func handleNavigation(at point:CGPoint,selected:String)->Bool {
        if let index=tabFrames.firstIndex(where:{$0.contains(point)}){category=categories[index];page=0;hovered=nil;layout(in:layer.bounds,selected:selected);show(selected:selected,motion:motionID);return true}
        if let index=navigation.firstIndex(where:{$0.contains(point)}){let pages=max(1,(filtered.count+pageSize-1)/pageSize);page=(page+(index==0 ? pages-1:1))%pages;hovered=nil;layout(in:layer.bounds,selected:selected);show(selected:selected,motion:motionID);return true}
        return false
    }
    func pet(at point:CGPoint)->String?{NotchInputRegion.nearestTarget(at:point,frames:shown.map(\.cell),padding:0).map{shown[$0].spec.id}}
    func hover(at point:CGPoint?){let id=point.flatMap{pet(at:$0)};guard id != hovered else{return};hovered=id;for entry in shown{let active=entry.spec.id==id,scale:CGFloat=active ? 1.10:1;let from=entry.sprite.presentation()?.transform.m11 ?? entry.sprite.transform.m11;CATransaction.begin();CATransaction.setDisableActions(true);entry.sprite.setAffineTransform(CGAffineTransform(scaleX:scale,y:scale));entry.glow.opacity=active ? 0.34:0.2;CATransaction.commit();if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{let animation=CABasicAnimation(keyPath:"transform.scale");animation.fromValue=from;animation.toValue=scale;animation.duration=0.2;animation.timingFunction=curve;entry.sprite.add(animation,forKey:"hover")}}}
}
