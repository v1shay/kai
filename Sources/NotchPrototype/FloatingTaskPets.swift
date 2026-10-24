import AppKit

struct FloatingTaskLifecycle {
    private(set) var tasks=[KaiActiveTask]()
    mutating func update(_ incoming:[KaiActiveTask],finished:[KaiFinishedTask],enabled:Bool)->[KaiActiveTask]{
        guard enabled else{tasks=[];return []}
        let incoming=incoming.filter{!$0.id.isEmpty && !$0.id.hasPrefix("dot:") && $0.scene != "off"}
        guard !tasks.isEmpty || incoming.count>1 else{return []}
        let ended=tasks.filter{task in finished.contains{$0.id == task.id && $0.runId == task.runId}}
        let observed=Set(incoming.map(\.id))
        // Each bridge update is a complete snapshot, not a stream of task deltas.
        tasks.removeAll{!observed.contains($0.id) || ended.contains($0)}
        for task in incoming where !finished.contains(where:{$0.id == task.id && $0.runId == task.runId}){
            if let index=tasks.firstIndex(where:{$0.id == task.id}){tasks[index]=task}else{tasks.append(task)}
        }
        return ended
    }
}

enum FloatingTaskCorner:String,CaseIterable {
    case topRight,topLeft,bottomRight,bottomLeft
    var title:String{switch self{case .topRight:return "Top right";case .topLeft:return "Top left";case .bottomRight:return "Bottom right";case .bottomLeft:return "Bottom left"}}
    func frames(screen:CGRect,index:Int,size:CGFloat,text:Bool)->(pet:CGRect,pill:CGRect){
        let width=text ? CGFloat(220):size,height=size+(text ? 34:0),stride=height+14
        let left=self == .topLeft || self == .bottomLeft,top=self == .topLeft || self == .topRight
        let x=left ? screen.minX+18:screen.maxX-18-width,y=top ? screen.maxY-18-height-CGFloat(index)*stride:screen.minY+18+CGFloat(index)*stride
        return (CGRect(x:x+(width-size)/2,y:y+(text ? 34:0),width:size,height:size),CGRect(x:x,y:y,width:width,height:26))
    }
}

@MainActor final class FloatingTaskPets:NSObject {
    var onCycle:((KaiActiveTask)->Void)?,onOpen:((KaiActiveTask,Bool)->Void)?,onFinish:((KaiActiveTask,Bool)->Void)?,onChange:(()->Void)?
    let menu=NSMenu()
    private let defaults:UserDefaults,images=SpriteAssetCache(maxPixelSize:960)
    private var model=FloatingTaskLifecycle(),entries=[String:Entry](),specs=[PetGallerySpec](),profiles=[String:PetGradientProfile](),pets=[String:String](),root:URL?
    private var preview=false,offset=0
    private let enabledItem=NSMenuItem(),textItem=NSMenuItem(),previewItem=NSMenuItem(),finishItem=NSMenuItem(),nextItem=NSMenuItem()
    var enabled:Bool{defaults.bool(forKey:"floatingTasks.enabled")}
    var active:Bool{!model.tasks.isEmpty}
    var tracking:Bool{active && !preview}
    var trackedTasks:[KaiActiveTask]{model.tasks}
    private var showText:Bool{defaults.object(forKey:"floatingTasks.text") as? Bool ?? true}
    private var size:CGFloat{let value=defaults.double(forKey:"floatingTasks.size");return value.isFinite && value>0 ? min(112,max(40,value)):64}
    private var corner:FloatingTaskCorner{FloatingTaskCorner(rawValue:defaults.string(forKey:"floatingTasks.corner") ?? "") ?? .topRight}
    @MainActor private final class Entry {
        let row=FloatingTaskPetView(frame:.zero),pill=FloatingTaskStatusPill()
        var label:NSTextField{pill.label};var icon:NotchIndicatorEngine{pill.icon}
        let petWindow=FloatingPetPanel(),pillWindow=FloatingPetPanel(),transition=NotchVisualTransition()
        var pet="",motion="",scene="",loading:UUID?
        var petFrame=CGRect.null,pillFrame=CGRect.null
        init(){
            petWindow.contentView=row;row.layer?.opacity=0;pillWindow.contentView=pill;pillWindow.ignoresMouseEvents=true
        }
        func hide(){loading=nil;row.cancelClick();row.sprite.removeAllAnimations();icon.hide();petWindow.orderOut(nil);pillWindow.orderOut(nil)}
    }
    init(defaults:UserDefaults = .standard){
        self.defaults=defaults;super.init()
        for (item,title,selector) in [(enabledItem,"Floating task pets",#selector(toggle)),(textItem,"Show status pills",#selector(toggleText))]{item.title=title;item.target=self;item.action=selector;menu.addItem(item)}
        let corners=NSMenu(),cornerItem=NSMenuItem(title:"Screen corner",action:nil,keyEquivalent:"");cornerItem.submenu=corners;menu.addItem(cornerItem)
        for corner in FloatingTaskCorner.allCases{let item=corners.addItem(withTitle:corner.title,action:#selector(selectCorner(_:)),keyEquivalent:"");item.target=self;item.representedObject=corner.rawValue}
        let sizing=NSView(frame:CGRect(x:0,y:0,width:210,height:44)),label=NSTextField(labelWithString:"Pet size"),slider=NSSlider(value:Double(size),minValue:40,maxValue:112,target:self,action:#selector(resize(_:)))
        label.font = .menuFont(ofSize:12);label.frame=CGRect(x:14,y:25,width:180,height:16);slider.frame=CGRect(x:12,y:3,width:184,height:20);slider.isContinuous=true;slider.setAccessibilityLabel("Floating pet size");sizing.addSubview(label);sizing.addSubview(slider);let sizeItem=NSMenuItem();sizeItem.view=sizing;menu.addItem(sizeItem)
        for (item,title,selector) in [(nextItem,"Next tasks",#selector(nextTasks)),(previewItem,"Preview floating pets",#selector(previewPets)),(finishItem,"Finish a preview task",#selector(finishPreview))]{item.title=title;item.target=self;item.action=selector;menu.addItem(item)}
        refreshMenu()
    }
    func configure(_ specs:[PetGallerySpec],profiles:[String:PetGradientProfile],root:URL){self.specs=specs;self.profiles=profiles;self.root=root}
    func update(_ tasks:[KaiActiveTask],finished:[KaiFinishedTask],pets:[String:String]){
        guard enabled || active || preview else{return}
        guard !preview else{self.pets.merge(pets){_,new in new};refresh();return};self.pets=pets
        let ended=model.update(tasks,finished:finished,enabled:enabled)
        refresh();ended.forEach{onFinish?($0,false)}
    }
    func refresh(pets:[String:String]?=nil){
        if let pets{if preview{self.pets.merge(pets){_,new in new}}else{self.pets=pets}}
        guard let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil}) ?? NSScreen.main else{return}
        let area=screen.visibleFrame,stride=size+(showText ? 34:0)+14,capacity=max(1,Int((area.height-36)/stride))
        offset=min(offset,max(0,model.tasks.count-capacity))
        let visible=Array(model.tasks.dropFirst(offset).prefix(capacity)),ids=Set(visible.map(\.id))
        for id in Array(entries.keys) where !ids.contains(id){
            guard let entry=entries.removeValue(forKey:id) else{continue}
            entry.loading=nil;entry.row.cancelClick();entry.row.sprite.removeAnimation(forKey:"spriteFrames");entry.icon.hide()
            if let layer=entry.row.layer{entry.transition.reveal(layer,visible:false)}
            if let layer=entry.pill.layer{entry.transition.reveal(layer,visible:false)}
            DispatchQueue.main.asyncAfter(deadline:.now()+0.28){entry.hide()}
        }
        for (index,task) in visible.enumerated(){
            let isNew=entries[task.id] == nil,entry=entries[task.id] ?? Entry();entries[task.id]=entry
            entry.row.onSingle={ [weak self] in self?.onCycle?(task) };entry.row.onDouble={ [weak self] in guard let self else{return};self.onOpen?(task,self.preview) }
            entry.row.onScroll={ [weak self] delta in self?.page(delta) }
            entry.row.setAccessibilityLabel(task.name);entry.row.toolTip=task.name+"\n"+task.text+(model.tasks.count>capacity ? "\nScroll for more tasks":"")
            let frames=corner.frames(screen:area,index:index,size:size,text:showText)
            if entry.petFrame != frames.pet || entry.pillFrame != frames.pill{
                entry.petFrame=frames.pet;entry.pillFrame=frames.pill
                if !isNew,!NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{
                    NSAnimationContext.runAnimationGroup{context in context.duration=0.28;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.85,0.25,1);entry.petWindow.animator().setFrame(frames.pet,display:true);entry.pillWindow.animator().setFrame(frames.pill,display:true)}
                }else{entry.petWindow.setFrame(frames.pet,display:true);entry.pillWindow.setFrame(frames.pill,display:true)}
            }
            entry.row.layoutSubtreeIfNeeded();entry.pill.layoutSubtreeIfNeeded()
            let text=task.text.split(separator:"\n").last.map(String.init) ?? task.name
            if entry.label.stringValue != text{entry.label.stringValue=text}
            let pet=self.pets[task.id] ?? specs.first?.id ?? "",motion=["thinking","waiting","permission","question"].contains(task.scene) ? "waiting":"running"
            if entry.pet != pet || entry.motion != motion{
                entry.pet=pet;entry.motion=motion
                if let spec=specs.first(where:{$0.id == pet}),let motion=spec.motions[motion] ?? spec.motions["idle"],let timeline=SpriteTimeline(row:motion.row,rows:spec.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop),let root{
                    let token=UUID();entry.loading=token
                    images.load(root.appendingPathComponent(spec.spritesheet)){[weak entry] image in
                        guard let entry,entry.loading == token,let image else{return}
                        CATransaction.begin();CATransaction.setDisableActions(true);entry.row.sprite.contents=image;entry.row.sprite.contentsRect=timeline.frames[0];CATransaction.commit()
                        entry.row.sprite.removeAnimation(forKey:"spriteFrames")
                        if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{entry.row.sprite.add(timeline.animation(),forKey:"spriteFrames")}
                        if isNew,let layer=entry.row.layer{
                            entry.transition.reveal(layer,visible:true,fromOpacity:0)
                            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{let pop=CABasicAnimation(keyPath:"transform.scale");pop.fromValue=0.72;pop.toValue=1;pop.duration=0.36;pop.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.72,0.24,1);layer.add(pop,forKey:"statusScale")}
                        }
                        if !isNew{let fade=CATransition();fade.type = .fade;fade.duration=0.22;entry.row.sprite.add(fade,forKey:"petChange")}
                    }
                }
                if let profile=profiles[pet]{entry.icon.setProfile(profile,wave:false)}
            }
            let scene=IntegrationIcons.id(for:task.text) ?? task.scene
            if showText,entry.scene != scene{entry.scene=scene;entry.icon.showScene(scene,looping:false)}
            if !entry.petWindow.isVisible{entry.petWindow.orderFrontRegardless()}
            if showText{if !entry.pillWindow.isVisible{entry.pillWindow.orderFrontRegardless()}}else{if entry.pillWindow.isVisible{entry.pillWindow.orderOut(nil)};if !entry.scene.isEmpty{entry.icon.hide();entry.scene=""}}
            if isNew,let layer=entry.pill.layer{entry.transition.reveal(layer,visible:true,fromOpacity:0)}
        }
        refreshMenu()
    }
    func stop(){entries.values.forEach{$0.hide()};entries=[:]}
    private func refreshMenu(){enabledItem.state=enabled ? .on:.off;textItem.state=showText ? .on:.off;finishItem.isHidden = !preview;previewItem.title=preview ? "End preview":"Preview floating pets";previewItem.isEnabled=preview || !active;nextItem.isEnabled=model.tasks.count>entries.count;for item in menu.items.flatMap({$0.submenu?.items ?? []}){item.state=item.representedObject as? String == corner.rawValue ? .on:.off}}
    private func page(_ delta:Int){let pages=max(1,model.tasks.count-entries.count+1);offset=(offset+delta+pages)%pages;refresh()}
    @objc private func nextTasks(){page(1)}
    @objc private func toggle(){if preview{preview=false};defaults.set(!enabled,forKey:"floatingTasks.enabled");_ = model.update([],finished:[],enabled:false);refresh();onChange?()}
    @objc private func toggleText(){defaults.set(!showText,forKey:"floatingTasks.text");refresh()}
    @objc private func selectCorner(_ item:NSMenuItem){defaults.set(item.representedObject,forKey:"floatingTasks.corner");refresh()}
    @objc private func resize(_ slider:NSSlider){defaults.set(slider.doubleValue,forKey:"floatingTasks.size");refresh()}
    @objc func previewPets(){
        if preview{preview=false;_ = model.update([],finished:[],enabled:false);refresh();onChange?();return}
        guard !active else{return};preview=true
        let tasks=[("Checking layout","terminal","swift test --filter FloatingTaskPetsTests"),("Writing changes","files","Updating the native interface"),("Thinking through a fix","pencil","Keeping the task attached to its pet")].enumerated().map{KaiActiveTask(id:"floating-preview:\($0.offset)",name:$0.element.0,scene:$0.element.1,text:$0.element.2)}
        _ = model.update(tasks,finished:[],enabled:true)
        let allowed=PetSelectionPreferences(defaults:defaults).automaticIDs(specs.map(\.id))
        for (index,task) in tasks.enumerated(){pets[task.id]=allowed.isEmpty ? "":allowed[index%allowed.count]}
        refresh()
    }
    @objc private func finishPreview(){guard preview,let task=model.tasks.first else{return};_ = model.update(Array(model.tasks.dropFirst()),finished:[KaiFinishedTask(id:task.id,runId:task.runId)],enabled:true);refresh();onFinish?(task,true)}
}

@MainActor private final class FloatingPetPanel:NSPanel {
    init(){super.init(contentRect:.zero,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);isOpaque=false;backgroundColor = .clear;hasShadow=false;hidesOnDeactivate=false;level = .statusBar;collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary,.stationary]}
    override var canBecomeKey:Bool{false}
    override var canBecomeMain:Bool{false}
}

@MainActor final class FloatingTaskStatusPill:NSVisualEffectView {
    let label=NSTextField(labelWithString:""),icon=NotchIndicatorEngine()
    init(){
        super.init(frame:.zero);material = .hudWindow;blendingMode = .behindWindow;state = .active;appearance=NSAppearance(named:.darkAqua)
        wantsLayer=true;layer?.cornerRadius=13;layer?.masksToBounds=true;layer?.borderWidth=0.5;layer?.borderColor=NSColor.white.withAlphaComponent(0.14).cgColor
        label.font = .systemFont(ofSize:11);label.textColor=NSColor.white.withAlphaComponent(0.86);label.lineBreakMode = .byTruncatingMiddle
        addSubview(label);layer?.addSublayer(icon.layer)
    }
    required init?(coder:NSCoder){fatalError()}
    override func layout(){super.layout();label.frame=CGRect(x:37,y:5,width:bounds.width-47,height:16);icon.layer.frame=CGRect(x:6,y:4,width:28,height:18);icon.layout()}
}

@MainActor final class FloatingTaskPetView:NSView {
    let sprite=CALayer()
    var onSingle:(()->Void)?,onDouble:(()->Void)?,onScroll:((Int)->Void)?
    private var click:DispatchWorkItem?
    static var clickDelay:TimeInterval{max(0.22,NSEvent.doubleClickInterval)}
    override init(frame:NSRect){super.init(frame:frame);wantsLayer=true;sprite.contentsGravity = .resizeAspect;sprite.magnificationFilter = .nearest;layer?.addSublayer(sprite);setAccessibilityElement(true);setAccessibilityRole(.button)}
    required init?(coder:NSCoder){fatalError()}
    override func layout(){super.layout();sprite.frame=bounds}
    override func mouseDown(with event:NSEvent){}
    override func mouseUp(with event:NSEvent){guard bounds.contains(convert(event.locationInWindow,from:nil)) else{return};handleClick(event.clickCount)}
    func handleClick(_ count:Int){cancelClick();if count>1{onDouble?()}else{let work=DispatchWorkItem{[weak self] in self?.onSingle?()};click=work;DispatchQueue.main.asyncAfter(deadline:.now()+Self.clickDelay,execute:work)}}
    func cancelClick(){click?.cancel();click=nil}
    override func accessibilityPerformPress()->Bool{onDouble?();return true}
    override func scrollWheel(with event:NSEvent){if abs(event.scrollingDeltaY)>1{onScroll?(event.scrollingDeltaY<0 ? 1:-1)}}
}
