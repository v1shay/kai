import AppKit
import ImageIO
import WebKit

private struct Catalog: Decodable { let pets: [Pet]; let stateAnimations: [String: Motion] }
private struct Pet: Decodable { let id, displayName, spritesheet: String; let grid: Grid; let animationOverrides:[String:Motion]? }
private struct Grid: Decodable { let rows: Int }
private struct Motion: Decodable { let row: Int; let columns, durationsMs: [Int]; let loop: Bool }
private struct FileCitation { let range:NSRange;let url:URL;let purpose:String }
private let citationPattern=try! NSRegularExpression(pattern:#":?codex-file-citation\{[^}\n]*\}"#)
private let citationPathPattern=try! NSRegularExpression(pattern:#"\bpath="([^"]+)""#)
private let citationPurposePattern=try! NSRegularExpression(pattern:#"\bpurpose="([^"]+)""#)
private let webLinkDetector=try! NSDataDetector(types:NSTextCheckingResult.CheckingType.link.rawValue)

private func fileCitations(in text:String)->[FileCitation]{
    let source=text as NSString
    return citationPattern.matches(in:text,range:NSRange(location:0,length:source.length)).compactMap{match in
        let token=source.substring(with:match.range),tokenSource=token as NSString
        guard let pathMatch=citationPathPattern.firstMatch(in:token,range:NSRange(location:0,length:tokenSource.length)) else{return nil}
        let rawPath=tokenSource.substring(with:pathMatch.range(at:1)),path=(rawPath as NSString).expandingTildeInPath
        guard path.hasPrefix("/") else{return nil}
        let purpose=citationPurposePattern.firstMatch(in:token,range:NSRange(location:0,length:tokenSource.length)).map{tokenSource.substring(with:$0.range(at:1))} ?? ""
        return FileCitation(range:match.range,url:URL(fileURLWithPath:path).standardizedFileURL,purpose:purpose)
    }
}

private func displayCitations(in text:String)->(String,[(NSRange,URL)]){
    let source=text as NSString,citations=fileCitations(in:text)
    guard !citations.isEmpty else{return(text,[])}
    var rendered="",cursor=0,links=[(NSRange,URL)]()
    for citation in citations {
        rendered += source.substring(with:NSRange(location:cursor,length:citation.range.location-cursor))
        let label=citation.url.lastPathComponent,location=(rendered as NSString).length
        rendered += label;links.append((NSRange(location:location,length:(label as NSString).length),citation.url))
        cursor=NSMaxRange(citation.range)
    }
    rendered += source.substring(from:cursor)
    return(rendered,links)
}

private func draggedImages(_ pasteboard:NSPasteboard)->[NSImage] {
    let urls=(pasteboard.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL]) ?? []
    let images=urls.compactMap(NSImage.init(contentsOf:));if !images.isEmpty{return images}
    return NSImage(pasteboard:pasteboard).map{[$0]} ?? []
}

private final class NotchPanel:NSPanel { override var canBecomeKey:Bool{true};override var canBecomeMain:Bool{false} }

private class ImageDropView:NSView {
    var dragEntered:(()->Void)?,imageDropped:(([NSImage])->Void)?
    override init(frame:NSRect){super.init(frame:frame);registerForDraggedTypes([.fileURL,.png,.tiff]+NSImage.imageTypes.map{NSPasteboard.PasteboardType($0)})}
    required init?(coder:NSCoder){fatalError()}
    private func images(_ sender:NSDraggingInfo)->[NSImage]{draggedImages(sender.draggingPasteboard)}
    override func draggingEntered(_ sender:NSDraggingInfo)->NSDragOperation{guard !images(sender).isEmpty else{return []};dragEntered?();return .copy}
    override func draggingUpdated(_ sender:NSDraggingInfo)->NSDragOperation{images(sender).isEmpty ? []:.copy}
    override func performDragOperation(_ sender:NSDraggingInfo)->Bool{let images=images(sender);guard !images.isEmpty else{return false};imageDropped?(images);return true}
}

private final class ComposerField:NSTextField {
    var imagesEntered:(()->Void)?,imagesDropped:(([NSImage])->Void)?
    override init(frame:NSRect){super.init(frame:frame);registerForDraggedTypes([.fileURL,.png,.tiff]+NSImage.imageTypes.map{NSPasteboard.PasteboardType($0)})}
    required init?(coder:NSCoder){fatalError()}
    override func performKeyEquivalent(with event:NSEvent)->Bool{let flags=event.modifierFlags.intersection(.deviceIndependentFlagsMask);if flags == .command,event.charactersIgnoringModifiers?.lowercased() == "a"{if let editor=currentEditor(){editor.selectAll(nil)}else{window?.makeFirstResponder(self);selectText(nil)};return true};return super.performKeyEquivalent(with:event)}
    override func draggingEntered(_ sender:NSDraggingInfo)->NSDragOperation{guard !draggedImages(sender.draggingPasteboard).isEmpty else{return []};imagesEntered?();return .copy}
    override func draggingUpdated(_ sender:NSDraggingInfo)->NSDragOperation{draggedImages(sender.draggingPasteboard).isEmpty ? []:.copy}
    override func performDragOperation(_ sender:NSDraggingInfo)->Bool{let images=draggedImages(sender.draggingPasteboard);guard !images.isEmpty else{return false};imagesDropped?(images);return true}
}

@MainActor private final class PetMenuRow:NSView {
    var onSelect:(()->Void)?
    private let label:NSTextField,checkmark:NSTextField
    var selected=false { didSet { checkmark.isHidden = !selected } }

    init(title:String) {
        label=NSTextField(labelWithString:title)
        checkmark=NSTextField(labelWithString:"✓")
        super.init(frame:CGRect(x:0,y:0,width:190,height:20))
        wantsLayer=true
        label.frame=CGRect(x:28,y:1,width:158,height:18)
        checkmark.frame=CGRect(x:8,y:1,width:18,height:18)
        for field in [label,checkmark] {
            field.font=NSFont.menuFont(ofSize:13)
            field.textColor = .labelColor
            field.lineBreakMode = .byTruncatingTail
            addSubview(field)
        }
        checkmark.isHidden=true
    }
    required init?(coder:NSCoder){fatalError()}
    override func hitTest(_ point:NSPoint)->NSView?{
        bounds.contains(convert(point,from:superview)) ? self : nil
    }
    override func updateTrackingAreas(){
        super.updateTrackingAreas()
        trackingAreas.forEach(removeTrackingArea)
        addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self))
    }
    override func mouseEntered(with event:NSEvent){
        layer?.backgroundColor=NSColor.selectedContentBackgroundColor.cgColor
        label.textColor = .selectedMenuItemTextColor;checkmark.textColor = .selectedMenuItemTextColor
    }
    override func mouseExited(with event:NSEvent){
        layer?.backgroundColor=NSColor.clear.cgColor
        label.textColor = .labelColor;checkmark.textColor = .labelColor
    }
    override func mouseDown(with event:NSEvent){}
    override func mouseUp(with event:NSEvent){
        if bounds.contains(convert(event.locationInWindow,from:nil)){onSelect?()}
    }
}

private final class NotchView:ImageDropView {
    var click:((CGPoint,Int)->Void)?
    var scroll:((CGPoint,CGFloat)->Void)?
    var hover:((CGPoint?)->Void)?
    var compactHitRect:CGRect?
    override func hitTest(_ point:NSPoint)->NSView?{if let compactHitRect,!compactHitRect.contains(convert(point,from:superview)){return nil};return super.hitTest(point)}
    override func updateTrackingAreas(){super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea);addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseMoved,.mouseEnteredAndExited,.activeAlways],owner:self))}
    override func mouseDown(with event:NSEvent) { click?(event.locationInWindow,event.clickCount) }
    override func scrollWheel(with event:NSEvent) { scroll?(event.locationInWindow,event.scrollingDeltaY) }
    override func mouseMoved(with event:NSEvent){hover?(event.locationInWindow)}
    override func mouseExited(with event:NSEvent){hover?(nil)}
}

@MainActor final class KaiPetApp: NSObject, NSApplicationDelegate, NSTextFieldDelegate, NSTextViewDelegate {
    private let mask = CAShapeLayer(), notchContent = CALayer(), sprite = CALayer(), indicator = NotchIndicatorEngine(), miniUI = MiniAppInterface(), petGallery=PetGallery(), dictation=DictationController(), bridge=KaiBridge(),sounds=KaiSoundEngine()
    private var panel: NSPanel!, dropPanel:NSPanel!, composerField:ComposerField!, status: NSStatusItem!, catalog: Catalog!, gradientCatalog: GradientCatalog!
    private var browserView:WKWebView?, experimentalModeItem:NSMenuItem!, mathModeItem:NSMenuItem!,soundsItem:NSMenuItem!
    private let speech=SpeechPlayback(),media=MediaCompanion()
    private var speakResponses=false,compactOnSend=false,youtubeMedia=false,spotifyMedia=false
    private var voiceMenu=NSMenu(),speechItem:NSMenuItem!,compactSendItem:NSMenuItem!,youtubeItem:NSMenuItem!,spotifyItem:NSMenuItem!,featureStatusItem:NSMenuItem!
    private var mediaState:MediaNowPlaying?,mediaImage:NSImage?,mediaProfile:PetGradientProfile?,mediaVisible=false,artworkTask:URLSessionDataTask?
    private let mediaArtwork=CALayer(),visualTransition=NotchVisualTransition()
    private var mediaReleaseWork:DispatchWorkItem?
    private var mediaSize:CGFloat=32,mediaSizeSlider:NSSlider?,loadedMediaKey:String?,presentedMediaKey:String?
    private let mathRenderer=MathRenderer()
    private var renderMath=true,soundsEnabled=true,petMenuRows=[String:PetMenuRow]()
    private var mathRefreshWork:DispatchWorkItem?
    private var conversationScroll:NSScrollView!,conversationText:NSTextView!,latestState:KaiState?,renderedThreadID:String?,renderedBlocks=[String](),renderedRanges=[NSRange](),currentScene="",sendingPrompt:String?,attachmentFiles=[URL](),queuedImageCount=0
    private var motionItems = [String: NSMenuItem](), indicatorItems = [String: NSMenuItem](), accentItems = [String: NSMenuItem]()
    private var localMonitor: Any?, globalMonitor: Any?, dragMonitor:Any?, showItem: NSMenuItem!
    private var morphWork = [DispatchWorkItem](),petClickWork:DispatchWorkItem?
    private var petID = "codex", userPetID = "codex", renderedPetID = "", motionID = "idle", indicatorID = "demo", frame = 0, playToken = 0
    private var taskPets=[String:String](),activeTaskIDs=Set<String>(),focusedTaskID:String?,autoCompact=false
    private var loadedSpritePetID:String?,loadedSprite:CGImage?
    private var assistantPalette=[NSColor]()
    private var petScale: CGFloat = 1
    private var accentSide = "right"
    private enum DictationTrigger {case command,function,option}
    private var open = false, miniOpen = false, pinned = false, commandDown = false, optionDown = false, controlDown = false, functionDown = false,petGalleryVisible=false
    private var optionWork: DispatchWorkItem?
    private var pendingContext: ApplicationContext?
    private var contextRequestID: String?
    private var contextLightweight = false
    private var dictationTrigger:DictationTrigger?
    private var experimentalWebMode=false
    private var modeBeforeExperimental=(open:false,mini:false,pinned:false)
    private var compactWhileWorking=false,presentedCompact=false,compactThreadID:String?,compactStartHistoryCount=0
    private var pendingFilesRequestID:String?
    private var miniListening = false,dictating=false,dictationPrefix=""
    private var approachImages=[NSImage](),approachActive=false,lastImageDrop=0.0
    private var imageExpandWork:DispatchWorkItem?
    private var lastCommandTap = 0.0, lastControlTap = 0.0
    private var holdWork: DispatchWorkItem?
    private var notchWidth: CGFloat = 210, notchHeight: CGFloat = 39, canvasWidth: CGFloat = 470, canvasHeight: CGFloat = 320
    private var assetRoot: URL {
        if let resources=Bundle.main.resourceURL {
            let bundled=resources.appendingPathComponent("kai_pets")
            if FileManager.default.fileExists(atPath:bundled.appendingPathComponent("animation_catalog.json").path){return bundled}
        }
        return URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("kai_pets")
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        NSApp.setActivationPolicy(.accessory)
        petScale = CGFloat(UserDefaults.standard.object(forKey: "petScale") as? Double ?? 1)
        experimentalWebMode=UserDefaults.standard.bool(forKey:"experimentalWebMode")
        renderMath=UserDefaults.standard.object(forKey:"renderMath") as? Bool ?? true
        soundsEnabled=UserDefaults.standard.object(forKey:"soundsEnabled") as? Bool ?? true;sounds.enabled=soundsEnabled
        if let size=UserDefaults.standard.object(forKey:"mediaSize") as? Double,size.isFinite{mediaSize=CGFloat(size)}
        speakResponses=UserDefaults.standard.bool(forKey:"speakResponses")
        compactOnSend=UserDefaults.standard.bool(forKey:"compactOnSend")
        youtubeMedia=UserDefaults.standard.bool(forKey:"youtubeMedia")
        spotifyMedia=UserDefaults.standard.bool(forKey:"spotifyMedia")
        accentSide = UserDefaults.standard.string(forKey: "accentSide") ?? "right"
        catalog = try! JSONDecoder().decode(Catalog.self,
            from: Data(contentsOf: assetRoot.appendingPathComponent("animation_catalog.json")))
        gradientCatalog = try! JSONDecoder().decode(GradientCatalog.self,
            from: Data(contentsOf: assetRoot.appendingPathComponent("gradient_profiles.json")))
        precondition(Set(catalog.pets.map(\.id)) == Set(gradientCatalog.profileByPetID.keys), "Every pet must have exactly one gradient profile")
        petGallery.configure(catalog.pets.map{pet in
            var motions=catalog.stateAnimations;pet.animationOverrides?.forEach{motions[$0.key]=$0.value}
            let galleryMotions=motions.mapValues{PetGalleryMotion(row:$0.row,columns:$0.columns,durationsMs:$0.durationsMs,loop:$0.loop)}
            let stops=gradientCatalog.profileByPetID[pet.id]?.gradients.ambient.stops ?? []
            let colors=stops.map{stop -> CGColor in let value=UInt64(stop.color.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255,alpha:0.9).cgColor}
            return PetGallerySpec(id:pet.id,spritesheet:pet.spritesheet,rows:pet.grid.rows,motions:galleryMotions,colors:colors)
        },at:assetRoot)
        dictation.onText={ [weak self] text in guard let self,self.dictating else{return};self.setPrompt(self.dictationPrefix+text) };dictation.onLevels={ [weak self] levels in self?.indicator.setExternalLevels(levels) }
        buildMenu(); applyAccentSide(); placePanel(); play(); indicator.hide()
        miniUI.onProject={ [weak self] path in guard let self else{return};self.bridge.send("project",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onChat={ [weak self] id in guard let self else{return};self.bridge.send("chat",["id":id,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNew={ [weak self] in guard let self else{return};self.bridge.send("new",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNewInProject={ [weak self] in guard let self else{return};self.bridge.send("new_project",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onFolder={ [weak self] path in guard let self else{return};self.bridge.send("folder",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onSettings={ [weak self] model, effort in self?.bridge.send("settings",["model":model,"effort":effort]) }
        miniUI.onNavigate={ [weak self] direction in guard let self else{return};self.bridge.send("navigate",["direction":direction,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onContextMode={ [weak self] lightweight in self?.contextLightweight=lightweight }
        miniUI.onSend={ [weak self] in self?.sendPrompt() }
        miniUI.onApprove={ [weak self] in self?.answerApproval("approve") }
        miniUI.onReject={ [weak self] in self?.answerApproval("reject") }
        miniUI.onInterrupt={ [weak self] in guard let self else{return};self.bridge.send("interrupt",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onClose={ [weak self] in self?.closeNotch() }
        miniUI.onFilesVisibleChanged={ [weak self] visible in self?.indicator.setFileOpen(visible) }
        miniUI.onImagesChanged={ [weak self] images in self?.queueImages(images) }
        bridge.onState={ [weak self] state in self?.receive(state) }
        bridge.onError={ [weak self] error in guard let self else{return};self.sounds.playScene("failure");self.contextRequestID=nil;self.pendingContext=nil;self.sendingPrompt=nil;self.pendingFilesRequestID=nil;self.indicator.setFileLoading(false);self.miniUI.cancelPending();self.miniUI.setCodexMessage(error);self.syncMotion(force:true) }
        speech.onModels={ [weak self] models in self?.updateVoiceMenu(models) }
        speech.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onChange={ [weak self] state in self?.receiveMedia(state) }
        media.onLevels={ [weak self] values in guard let self,self.mediaVisible else{return};self.indicator.setExternalLevels(values) }
        speech.discover();media.configure(youtube:youtubeMedia,spotify:spotifyMedia)
        bridge.start()
        localMonitor = NSEvent.addLocalMonitorForEvents(matching: .flagsChanged) { [weak self] e in self?.flags(e); return e }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching: .flagsChanged) { [weak self] e in self?.flags(e) }
        dragMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDragged,.leftMouseUp]){[weak self] event in DispatchQueue.main.async{self?.globalDrag(event)}}
        NotificationCenter.default.addObserver(self, selector: #selector(placePanel),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(diagnoseMedia),name:NSNotification.Name("com.kai.media.diagnose"),object:nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(codexPreview(_:)),name:NSNotification.Name("com.kai.codex.preview"),object:nil)
    }

    func applicationWillTerminate(_ notification:Notification){mediaReleaseWork?.cancel();speech.stop();media.stop();artworkTask?.cancel();bridge.stop();attachmentFiles.forEach{try? FileManager.default.removeItem(at:$0)}}
    @objc private func codexPreview(_ note:Notification){guard let path=note.userInfo?["path"] as? String else{return};pinned=true;setOpen(true,mini:true);miniUI.previewFile(URL(fileURLWithPath:(path as NSString).expandingTildeInPath))}

    private func buildMenu() {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        status.button?.title = "海"; status.button?.toolTip = "kai"
        let menu = NSMenu()
        showItem = menu.addItem(withTitle: "show notch", action: #selector(toggleFromMenu), keyEquivalent: "")
        showItem.target = self
        experimentalModeItem=menu.addItem(withTitle:"experimental web mode",action:#selector(toggleExperimentalWebMode),keyEquivalent:"")
        experimentalModeItem.target=self
        mathModeItem=menu.addItem(withTitle:"render equations",action:#selector(toggleMathRendering),keyEquivalent:"")
        mathModeItem.target=self
        soundsItem=menu.addItem(withTitle:"sounds",action:#selector(toggleSounds),keyEquivalent:"")
        soundsItem.target=self
        let morphItem = menu.addItem(withTitle:"test size morph",action:#selector(testSizeMorph),keyEquivalent:"")
        morphItem.target = self
        let petRoot = NSMenuItem(title: "pet", action: nil, keyEquivalent: ""), petMenu = NSMenu()
        let randomPetItem=petMenu.addItem(withTitle:"randomize pet",action:nil,keyEquivalent:"")
        let randomRow=PetMenuRow(title:"randomize pet")
        randomRow.onSelect={ [weak self] in self?.randomizePet() }
        randomPetItem.view=randomRow
        petMenu.addItem(.separator())
        for pet in catalog.pets.sorted(by: { $0.displayName.localizedCaseInsensitiveCompare($1.displayName) == .orderedAscending }) {
            let item = petMenu.addItem(withTitle: pet.displayName, action: nil, keyEquivalent: "")
            let row=PetMenuRow(title:pet.displayName)
            row.onSelect={ [weak self] in self?.usePet(pet.id) }
            item.view=row;petMenuRows[pet.id]=row
        }
        petMenu.addItem(.separator())
        let done=petMenu.addItem(withTitle:"done",action:#selector(closePetPicker),keyEquivalent:"")
        done.target=self
        petRoot.submenu = petMenu; menu.addItem(petRoot)
        let sizeItem = NSMenuItem(), sizeView = NSView(frame: CGRect(x: 0, y: 0, width: 190, height: 34))
        let sizeLabel = NSTextField(labelWithString: "pet size"), sizeSlider = NSSlider(value: Double(petScale), minValue: 0.55, maxValue: 1, target: self, action: #selector(changePetSize))
        sizeLabel.frame = CGRect(x: 14, y: 9, width: 52, height: 17); sizeSlider.frame = CGRect(x: 72, y: 7, width: 104, height: 20); sizeSlider.isContinuous = true
        sizeView.addSubview(sizeLabel); sizeView.addSubview(sizeSlider); sizeItem.view = sizeView; menu.addItem(sizeItem)
        let motionRoot = NSMenuItem(title: "animation", action: nil, keyEquivalent: ""), motionMenu = NSMenu()
        let names = ["idle":"idle", "running-right":"run right", "running-left":"run left", "waving":"wave", "jumping":"jump", "failed":"failed", "waiting":"waiting", "running":"working", "review":"review"]
        for id in ["idle","running-right","running-left","waving","jumping","failed","waiting","running","review"] {
            let item = motionMenu.addItem(withTitle: names[id]!, action: #selector(selectMotion), keyEquivalent: "")
            item.target = self; item.representedObject = id; motionItems[id] = item
        }
        motionRoot.submenu = motionMenu; menu.addItem(motionRoot)
        let indicatorRoot = NSMenuItem(title: "notch indicator", action: nil, keyEquivalent: ""), indicatorMenu = NSMenu()
        for (id,name) in [("listening","listening (microphone)"),("thinking","thinking"),("demo","replay listening → thinking"),("pencil","writing"),("terminal","terminal"),("git","git"),("image","image"),("document","document"),("code","code"),("search","search"),("read","read"),("files","browse files"),("plan","plan"),("review","review"),("permission","permission"),("question","question for user"),("success","success"),("failure","failure"),("declined","declined"),("waiting","waiting"),("tool","external tool"),("agents","multiple agents"),("compact","context compression"),("warning","warning"),("cycle","random morph cycle"),("off","off")] {
            let item = indicatorMenu.addItem(withTitle: name, action: #selector(selectIndicator), keyEquivalent: "")
            item.target = self; item.representedObject = id; indicatorItems[id] = item
        }
        indicatorRoot.submenu = indicatorMenu; menu.addItem(indicatorRoot)
        let accentRoot = NSMenuItem(title: "corner gradient", action: nil, keyEquivalent: ""), accentMenu = NSMenu()
        for (id,name) in [("right","right"),("left","left"),("both","both"),("off","off")] {
            let item = accentMenu.addItem(withTitle: name, action: #selector(selectAccentSide), keyEquivalent: "")
            item.target = self; item.representedObject = id; accentItems[id] = item
        }
        accentRoot.submenu = accentMenu; menu.addItem(accentRoot); menu.addItem(.separator())
        menu.addItem(.separator())
        speechItem=menu.addItem(withTitle:"speak Codex responses",action:#selector(toggleSpeech),keyEquivalent:"")
        let voices=NSMenuItem(title:"local ONNX voice",action:nil,keyEquivalent:"");voices.submenu=voiceMenu;menu.addItem(voices)
        menu.addItem(withTitle:"rescan local voices",action:#selector(rescanVoices),keyEquivalent:"")
        menu.addItem(withTitle:"stop speaking",action:#selector(stopSpeaking),keyEquivalent:"")
        compactSendItem=menu.addItem(withTitle:"compact immediately on send",action:#selector(toggleCompactSend),keyEquivalent:"")
        youtubeItem=menu.addItem(withTitle:"YouTube when idle (Webby, Dia, Chrome)",action:#selector(toggleYouTube),keyEquivalent:"")
        spotifyItem=menu.addItem(withTitle:"Spotify when idle",action:#selector(toggleSpotify),keyEquivalent:"")
        let mediaSizeView=NSView(frame:NSRect(x:0,y:0,width:180,height:40))
        let mediaSizeLabel=NSTextField(labelWithString:"media thumbnail size"),mediaSlider=NSSlider(value:Double(mediaSize),minValue:16,maxValue:38,target:self,action:#selector(changeMediaSize(_:)))
        mediaSizeLabel.frame=NSRect(x:14,y:22,width:150,height:14);mediaSlider.frame=NSRect(x:12,y:2,width:155,height:20);mediaSlider.isContinuous=true;mediaSizeSlider=mediaSlider
        mediaSizeView.addSubview(mediaSizeLabel);mediaSizeView.addSubview(mediaSlider);let mediaSizeItem=NSMenuItem();mediaSizeItem.view=mediaSizeView;menu.addItem(mediaSizeItem)
        menu.addItem(withTitle:"allow media permissions…",action:#selector(mediaPermissions),keyEquivalent:"")
        let contextItem=menu.addItem(withTitle:"Prefer lightweight Option context",action:#selector(toggleContextMode(_:)),keyEquivalent:"");contextItem.target=self;contextItem.state=contextLightweight ? .on:.off
        featureStatusItem=menu.addItem(withTitle:"Local speech and media are optional",action:nil,keyEquivalent:"");featureStatusItem.isEnabled=false
        for item in menu.items where item.action != nil && item.target == nil {item.target=self}
        updateFeatureChecks()
        let quit = menu.addItem(withTitle: "quit kai pet", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        quit.target = NSApp; status.menu = menu; refreshChecks()
    }

    @objc private func placePanel() {
        guard let screen = NSScreen.screens.first(where: { $0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil }),
              let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea else { panel?.orderOut(nil); return }
        notchHeight = screen.safeAreaInsets.top + 1
        let gapCenter = (left.maxX + right.minX) / 2
        let center = abs(gapCenter - screen.frame.midX) <= 1 ? screen.frame.midX : gapCenter
        notchWidth = 2 * max(center - left.maxX, right.minX - center); canvasWidth = notchWidth + 260
        let rect = NSRect(x: center - canvasWidth / 2, y: screen.frame.maxY - canvasHeight,
                          width: canvasWidth, height: canvasHeight)
        if panel == nil {
            panel = NotchPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.backgroundColor = .clear; panel.isOpaque = false; panel.hasShadow = false; panel.ignoresMouseEvents = true
            panel.level = .statusBar; panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
            let host=NotchView(frame:CGRect(origin:.zero,size:rect.size));host.click={ [weak self] point,count in self?.handleClick(point,count:count) };host.scroll={ [weak self] point,delta in self?.handleScroll(point,delta) };host.hover={ [weak self] point in self?.handleHover(point) };host.dragEntered={ [weak self] in guard self?.experimentalWebMode != true else{return};self?.handleImageEntered() };host.imageDropped={ [weak self] images in guard self?.experimentalWebMode != true else{return};self?.handleImageDrop(images) };panel.contentView=host;host.wantsLayer=true;panel.acceptsMouseMovedEvents=true
            composerField=ComposerField();composerField.isBordered=false;composerField.drawsBackground=false;composerField.focusRingType = .none;composerField.font=NSFont.systemFont(ofSize:10);composerField.textColor=NSColor.white.withAlphaComponent(0.88);composerField.delegate=self;composerField.isHidden=true;composerField.imagesEntered={ [weak self] in self?.handleImageEntered() };composerField.imagesDropped={ [weak self] images in self?.handleImageDrop(images) };host.addSubview(composerField)
            miniUI.composerFrameChanged={ [weak self] frame,animated in self?.layoutComposer(frame,animated:animated) }
            let scroll=NSScrollView(),text=NSTextView()
            scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.scrollerStyle = .overlay;scroll.borderType = .noBorder;scroll.isHidden=true
            text.drawsBackground=false;text.isEditable=false;text.isSelectable=true;text.textColor = .white;text.font=NSFont.systemFont(ofSize:9);text.textContainerInset=NSSize(width:3,height:3);text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true;text.autoresizingMask=[.width];text.delegate=self
            scroll.documentView=text;host.addSubview(scroll);conversationScroll=scroll;conversationText=text
            mathRenderer.onImageReady={ [weak self] in self?.scheduleMathRefresh() }
            if renderMath { mathRenderer.install(in:host,below:scroll) }
            miniUI.conversationFrameChanged={ [weak self] frame in self?.layoutConversation(frame) }
            let root = panel.contentView!.layer!; root.backgroundColor = NSColor.black.cgColor; root.mask = mask; root.addSublayer(miniUI.layer);root.addSublayer(petGallery.layer);root.addSublayer(notchContent)
            sprite.contentsGravity = .resizeAspect; sprite.magnificationFilter = .nearest; notchContent.addSublayer(indicator.leftAccentLayer); notchContent.addSublayer(indicator.accentLayer); notchContent.addSublayer(sprite);mediaArtwork.cornerRadius=7;mediaArtwork.masksToBounds=true;mediaArtwork.contentsGravity = .resizeAspectFill;mediaArtwork.isHidden=true;notchContent.addSublayer(mediaArtwork); notchContent.addSublayer(indicator.layer); notchContent.addSublayer(indicator.fileLayer)
        }
        panel.setFrame(rect, display: true); mask.frame = CGRect(origin: .zero, size: rect.size); notchContent.frame = mask.frame
        // The camera cutout itself has no drawable/event surface.  Keep a slim live
        // strip directly under it so a drag aimed at the notch can enter the app.
        let dropReach:CGFloat=36
        let dropRect=NSRect(x:center-notchWidth/2,y:screen.frame.maxY-notchHeight-dropReach,width:notchWidth,height:notchHeight+dropReach)
        if dropPanel == nil { dropPanel=NSPanel(contentRect:dropRect,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);dropPanel.backgroundColor = .clear;dropPanel.isOpaque=false;dropPanel.level = .popUpMenu;dropPanel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary,.stationary];let drop=NotchView(frame:NSRect(origin:.zero,size:dropRect.size));drop.autoresizingMask=[.width,.height];drop.click={ [weak self] _,_ in guard let self,self.compactWhileWorking else{return};self.compactWhileWorking=false;self.compactThreadID=nil;self.pinned=true;self.setOpen(true,mini:true) };drop.dragEntered={ [weak self] in self?.handleImageEntered() };drop.imageDropped={ [weak self] images in self?.handleImageDrop(images) };dropPanel.contentView=drop }
        dropPanel.setFrame(dropRect,display:true);(miniOpen || experimentalWebMode) ? dropPanel.orderOut(nil):dropPanel.orderFrontRegardless()
        let expanded = notchWidth + 160, kaiLeft = (canvasWidth - expanded) / 2
        let scale = screen.backingScaleFactor, topBandY = canvasHeight-notchHeight
        func px(_ value: CGFloat) -> CGFloat { round(value * scale) / scale }
        layoutPet();layoutMediaArtwork(animated:false);mediaSizeSlider?.maxValue=Double(max(16,notchHeight-3))
        let physicalRight = (canvasWidth + notchWidth) / 2, kaiRight = (canvasWidth + expanded) / 2
        let iconCenter=(physicalRight+kaiRight)/2
        indicator.layer.frame = CGRect(x:px(iconCenter-(miniOpen && !miniListening ? 34:14)),y:px(topBandY+(notchHeight-18)/2),width:28,height:18)
        indicator.fileLayer.frame=CGRect(x:px(iconCenter+(miniListening ? -14:6)),y:px(topBandY+(notchHeight-18)/2),width:28,height:18); indicator.layout();setFileButtonVisible(miniOpen && !miniListening,duration:0)
        let accentY=miniOpen ? 0:topBandY
        let glowWidth:CGFloat=108, glowHeight:CGFloat=82
        indicator.leftAccentLayer.frame = CGRect(x:px(kaiLeft),y:px(accentY),width:glowWidth,height:glowHeight)
        indicator.accentLayer.frame = CGRect(x:px(kaiRight-glowWidth),y:px(accentY),width:glowWidth,height:glowHeight); indicator.layout(); indicator.setAccentExpanded(miniOpen,duration:0)
        miniUI.layout(in:CGRect(x:px(kaiLeft+8),y:8,width:px(expanded-16),height:px(topBandY-16)))
        petGallery.layer.frame=miniUI.layer.frame;petGallery.layout(in:petGallery.layer.bounds,selected:petID)
        if experimentalWebMode{ensureBrowser()};browserView?.frame=CGRect(x:px(kaiLeft),y:0,width:px(expanded),height:canvasHeight)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        mask.path = path(width:(open || experimentalWebMode) ? expanded:notchWidth, height:(open || experimentalWebMode) ? ((miniOpen || experimentalWebMode) ? canvasHeight:notchHeight):notchHeight)
        notchContent.opacity = experimentalWebMode ? 0:1;miniUI.layer.isHidden=experimentalWebMode
        browserView?.isHidden = !experimentalWebMode
        CATransaction.commit()
        if open || experimentalWebMode { panel.orderFrontRegardless() }
    }

    private func path(width: CGFloat, height: CGFloat) -> CGPath {
        let x = (canvasWidth-width)/2, r: CGFloat = 14, sx: CGFloat = 7, sy: CGFloat = 9, top = canvasHeight, bottom = top-height, right = x+width, k = 1-0.5523
        let p = CGMutablePath(); p.move(to: CGPoint(x:x-sx,y:top)); p.addLine(to:CGPoint(x:right+sx,y:top))
        p.addCurve(to:CGPoint(x:right,y:top-sy),control1:CGPoint(x:right+sx*k,y:top),control2:CGPoint(x:right,y:top-sy*k))
        p.addLine(to:CGPoint(x:right,y:bottom+r)); p.addCurve(to:CGPoint(x:right-r,y:bottom),control1:CGPoint(x:right,y:bottom+r*k),control2:CGPoint(x:right-r*k,y:bottom))
        p.addLine(to:CGPoint(x:x+r,y:bottom)); p.addCurve(to:CGPoint(x:x,y:bottom+r),control1:CGPoint(x:x+r*k,y:bottom),control2:CGPoint(x:x,y:bottom+r*k))
        p.addLine(to:CGPoint(x:x,y:top-sy)); p.addCurve(to:CGPoint(x:x-sx,y:top),control1:CGPoint(x:x,y:top-sy*k),control2:CGPoint(x:x-sx*k,y:top)); p.closeSubpath(); return p
    }

    private func flags(_ event: NSEvent) {
        cancelSizeTest()
        let down = event.modifierFlags.contains(.command), option = event.modifierFlags.contains(.option), control = event.modifierFlags.contains(.control),function=event.modifierFlags.contains(.function)
        let miniChord = option && control, wasMiniChord = optionDown && controlDown
        defer { commandDown = down; optionDown = option; controlDown = control;functionDown=function }
        if control || down || function || !option {optionWork?.cancel();optionWork=nil}
        if option && !optionDown && !control && !down && !function && !dictating {
            let context=ApplicationContext.capture(lightweight:contextLightweight)
            if let path=context.imagePath {attachmentFiles.append(URL(fileURLWithPath:path))}
            let work=DispatchWorkItem { [weak self] in
                guard let self,self.optionDown,!self.controlDown,!self.commandDown,!self.functionDown else{return}
                self.pendingContext=context
                let id=UUID().uuidString;self.contextRequestID=id
                self.setPrompt("");self.miniUI.clearImages();self.bridge.send("new",["requestId":id])
                self.pinned=true;self.setOpen(true);self.beginDictation(trigger:.option)
            }
            optionWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.2,execute:work)
        } else if !option, dictating, dictationTrigger == .option {finishDictation()}
        let now = ProcessInfo.processInfo.systemUptime
        if control && !controlDown {
            if open && now-lastControlTap < 0.36 { lastControlTap=0; holdWork?.cancel(); closeNotch(); return }
            lastControlTap=now
        }
        if down && !commandDown && open && now-lastCommandTap < 0.36 {
            lastCommandTap=0; holdWork?.cancel(); closeNotch(); return
        }
        if miniChord != wasMiniChord {
            holdWork?.cancel(); holdWork = nil
            if miniChord {
                let work = DispatchWorkItem { [weak self] in
                    guard let self, self.optionDown, self.controlDown else { return }; self.pinned = true; self.setOpen(true, mini:true)
                }
                holdWork = work; DispatchQueue.main.asyncAfter(deadline:.now()+1.0,execute:work); return
            }
        }
        if down != commandDown {
            holdWork?.cancel(); holdWork = nil
            if down && !miniChord {
                lastCommandTap = now
                let work = DispatchWorkItem { [weak self] in
                    guard let self, self.commandDown,
                          CGEventSource.flagsState(.combinedSessionState).contains(.maskCommand) else { return }
                    self.pinned=true;if self.miniOpen{self.enterMiniListening(trigger:.command)}else{self.setOpen(true);self.beginDictation(trigger:.command)}
                }
                holdWork = work; DispatchQueue.main.asyncAfter(deadline: .now()+1.0, execute: work)
            } else if !down,dictating,dictationTrigger == .command { finishDictation() }
            }
        if function != functionDown {
            holdWork?.cancel();holdWork=nil
            if function && !down && !miniChord {
                let work=DispatchWorkItem{[weak self] in guard let self,self.functionDown else{return};self.setPrompt("");self.miniUI.clearImages();self.bridge.send("new",["requestId":UUID().uuidString]);self.pinned=true;if self.miniOpen{self.enterMiniListening(trigger:.function)}else{self.setOpen(true);self.beginDictation(trigger:.function)}}
                holdWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.0,execute:work)
            } else if !function,dictating,dictationTrigger == .function {finishDictation()}
        }
        }

    private func setOpen(_ desired: Bool, mini: Bool = false) {
        if desired && mini {autoCompact=false}
        if mediaVisible && (mini || compactWhileWorking || dictating || experimentalWebMode){dismissMedia()}
        guard panel != nil, desired != open || (desired && mini != miniOpen) || presentedCompact != compactWhileWorking else { return }
        if petGalleryVisible && (!desired || !mini){hidePetGallery(restore:false)}
        let wasOpen=open,wasMini=miniOpen; open = desired; miniOpen = desired && mini;presentedCompact=compactWhileWorking;if desired && !wasOpen{play()}else if !desired{playToken += 1}; panel.ignoresMouseEvents = !miniOpen && !compactWhileWorking;(panel.contentView as? NotchView)?.compactHitRect=compactWhileWorking && !miniOpen ? CGRect(x:(canvasWidth-notchWidth-160)/2,y:canvasHeight-notchHeight,width:notchWidth+160,height:notchHeight):nil;composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen;miniOpen ? dropPanel?.orderOut(nil):dropPanel?.orderFrontRegardless();refreshChecks()
        let destination = path(width:notchWidth+(desired ? 160:0),height:desired ? (mini ? canvasHeight:notchHeight):notchHeight), animation = CABasicAnimation(keyPath:"path")
        animation.fromValue = mask.presentation()?.path ?? mask.path; animation.toValue = destination
        animation.duration = desired ? (mini ? 0.52:0.42):0.55; animation.timingFunction = CAMediaTimingFunction(controlPoints: 0.16, 0.65, 0.25, 1)
        animateTopAccessories(forMini:miniOpen,duration:animation.duration)
        animateCornerAccents(toBottom:miniOpen,duration:animation.duration)
        indicator.setAccentExpanded(miniOpen,duration:animation.duration)
        miniUI.setVisible(miniOpen,duration:animation.duration)
        if miniOpen && !wasMini { miniListening=false;miniUI.resetFiles(); indicator.setFileOpen(false); currentScene="";if let state=latestState{showActivity(state.activity)};setMotion("waving",replay:true) }
        else if wasMini && !miniOpen { miniListening=false;if compactWhileWorking,let state=latestState {currentScene="";showActivity(state.activity)}else{applyIndicator()};syncMotion(force:true) }
        panel.orderFrontRegardless(); CATransaction.begin(); CATransaction.setDisableActions(true); mask.path = destination; CATransaction.commit(); mask.add(animation, forKey: "width")
        if !desired { DispatchQueue.main.asyncAfter(deadline: .now()+animation.duration) { [weak self] in
            guard let self, !self.open else { return }; self.panel.orderOut(nil)
        } }
    }

    private func handleClick(_ point:CGPoint,count:Int) {
        if compactWhileWorking && !miniOpen {autoCompact=false;compactWhileWorking=false;pinned=true;setOpen(true,mini:true);return}
        guard miniOpen else{return}
        if (sprite.presentation()?.frame ?? sprite.frame).insetBy(dx:-4,dy:-3).contains(point) {
            if count >= 2 {petClickWork?.cancel();petClickWork=nil;petGalleryVisible ? hidePetGallery():showPetGallery()}
            else if !petGalleryVisible {petClickWork?.cancel();let work=DispatchWorkItem{[weak self] in self?.randomizePet()};petClickWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.22,execute:work)}
            return
        }
        if petGalleryVisible {
            let frame=petGallery.layer.frame,local=CGPoint(x:point.x-frame.minX,y:point.y-frame.minY)
            if frame.contains(point),let id=petGallery.pet(at:local){usePet(id);hidePetGallery()}
            return
        }
        if !miniListening,(indicator.fileLayer.presentation()?.frame ?? indicator.fileLayer.frame).contains(point) { miniUI.toggleFiles();if miniUI.isFilesOpen{let requestID=UUID().uuidString;pendingFilesRequestID=requestID;indicator.setFileLoading(true);bridge.send("files",["requestId":requestID])}else{pendingFilesRequestID=nil;indicator.setFileLoading(false)};return }
        if (indicator.layer.presentation()?.frame ?? indicator.layer.frame).contains(point) { closeNotch();return }
        let frame=miniUI.layer.frame, local=CGPoint(x:point.x-frame.minX,y:point.y-frame.minY)
        if frame.contains(point) { _=miniUI.handleClick(local) }
    }

    private func handleHover(_ point:CGPoint?) {guard miniOpen else{return};if petGalleryVisible{let frame=petGallery.layer.frame;petGallery.hover(at:point.flatMap{frame.contains($0) ? CGPoint(x:$0.x-frame.minX,y:$0.y-frame.minY):nil});miniUI.handleHover(nil);return};guard let point else{miniUI.handleHover(nil);return};let frame=miniUI.layer.frame;miniUI.handleHover(frame.contains(point) ? CGPoint(x:point.x-frame.minX,y:point.y-frame.minY):nil) }
    private func handleScroll(_ point:CGPoint,_ delta:CGFloat){guard miniOpen else{return};let frame=miniUI.layer.frame;if frame.contains(point){miniUI.scroll(delta,at:CGPoint(x:point.x-frame.minX,y:point.y-frame.minY))}}

    private func showPetGallery(){guard miniOpen else{return};petGalleryVisible=true;miniUI.setVisible(false,duration:0.3);composerField?.isHidden=true;conversationScroll?.isHidden=true;petGallery.layout(in:petGallery.layer.bounds,selected:petID);petGallery.show(selected:petID,motion:motionID)}
    private func hidePetGallery(restore:Bool=true){guard petGalleryVisible else{return};petGalleryVisible=false;petGallery.hide();if restore{miniUI.setVisible(miniOpen,duration:0.32);composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen}}

    private func layoutComposer(_ frame:CGRect,animated:Bool){guard let field=composerField else{return};let root=miniUI.layer.frame,h:CGFloat=18,target=CGRect(x:root.minX+frame.minX+10,y:root.minY+frame.midY-h/2-2.25,width:max(20,frame.width-49),height:h);if animated{NSAnimationContext.runAnimationGroup{context in context.duration=0.42;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);field.animator().frame=target}}else{field.frame=target}}
    private func layoutConversation(_ frame:CGRect){guard let scroll=conversationScroll else{return};let root=miniUI.layer.frame;scroll.frame=CGRect(x:root.minX+frame.minX,y:root.minY+frame.minY,width:frame.width,height:frame.height)}
    private func setPrompt(_ text:String){composerField?.stringValue=text;miniUI.setComposerHasText(!text.isEmpty);syncMotion(force:!text.isEmpty)}
    func controlTextDidChange(_ obj:Notification){miniUI.setComposerHasText(!(composerField?.stringValue.isEmpty ?? true));syncMotion(force:true)}
    func control(_ control:NSControl,textView:NSTextView,doCommandBy selector:Selector)->Bool{if selector == #selector(NSResponder.insertNewline(_:)){sendPrompt();return true};return false}

    private func sendPrompt(){guard sendingPrompt == nil, contextRequestID == nil else{return};let prompt=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines);guard !prompt.isEmpty || queuedImageCount>0 else{return};sendingPrompt=prompt;setMotion("running-left",replay:true);speech.cancelAudio();bridge.send("send",["text":prompt]);if compactOnSend{hidePetGallery(restore:false);autoCompact=false;compactWhileWorking=true;compactThreadID=latestState?.threadId;compactStartHistoryCount=latestState?.history.count ?? 0;pinned=true;setOpen(true,mini:false)}}
    private func answerApproval(_ action:String){if action == "interrupt"{speech.cancelAudio()};var fields:[String:Any]=["requestId":miniUI.pendingRequestID ?? ""];if let id=latestState?.approvalThreadId{fields["threadId"]=id};bridge.send(action,fields)}

    private func queueImages(_ images:[NSImage]){
        var paths=[String]()
        for image in images {guard let tiff=image.tiffRepresentation,let bitmap=NSBitmapImageRep(data:tiff),let data=bitmap.representation(using:.png,properties:[:]) else{continue};let url=FileManager.default.temporaryDirectory.appendingPathComponent("kai-\(UUID().uuidString).png");do{try data.write(to:url,options:.atomic);attachmentFiles.append(url);paths.append(url.path)}catch{miniUI.setCodexMessage("Could not attach image: \(error.localizedDescription)")}}
        queuedImageCount=paths.count;bridge.send("attachments",["paths":paths])
    }

    private func receive(_ state:KaiState){
        defer{refreshMedia()}
        let previous=latestState,previousNotice=previous?.notice;latestState=state
        if let requestID=contextRequestID,state.completedRequestId == requestID {
            contextRequestID=nil
            if let context=pendingContext,let threadID=state.threadId,state.notice.hasPrefix("New task") {
                bridge.send("context",["threadId":threadID,"text":context.text,"paths":context.imagePath.map{[$0]} ?? []])
            }
            pendingContext=nil
        }
        if mediaVisible && MediaCompanion.codexNeedsNotch(state) {
            dismissMedia(closeNotch:false)
            if !miniOpen {autoCompact=true;compactWhileWorking=true;compactThreadID=nil;setOpen(true,mini:false)}
        }
        if !state.standalone{UserDefaults.standard.set(state.project,forKey:"kaiProject")};miniUI.setState(state);speech.receive(state);syncTaskPet(state);renderConversation(state.history)
        let completed=previous?.threadId == state.threadId && state.threadId != nil && (previous?.activeTurnId != nil || previous?.activity.scene == "thinking") && state.activeTurnId == nil && state.activity.scene == "off"
        if sendingPrompt != nil {
            if state.notice == "Turn started" || state.notice.hasPrefix("Steered active turn") {setPrompt("");miniUI.clearImages();sendingPrompt=nil}
            else if state.notice != previousNotice {sendingPrompt=nil;if compactOnSend && compactWhileWorking{compactWhileWorking=false;compactThreadID=nil;setOpen(true,mini:true)}}
        }
        if !dictating && !miniListening {showActivity(state.activity)}
        if state.approval && previous?.approval != true {autoCompact=false;compactWhileWorking=false;pinned=true;setOpen(true,mini:true)}
        let finishedTasks=Set(previous?.activeTasks.map(\.id) ?? []).subtracting(state.activeTasks.map(\.id))
        if completed || !finishedTasks.isEmpty {sounds.playScene("success");setMotion("jumping",replay:true)}
        else if previous?.activity.scene != "failure" && state.activity.scene == "failure" {setMotion("failed",replay:true)}
        else {syncMotion()}
        if compactWhileWorking,state.threadId == compactThreadID {
            if completed {
                let output=latestOutputCitation(in:Array(state.history.dropFirst(max(0,compactStartHistoryCount-1))))
                compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)
                setMotion("jumping",replay:true)
                if let output{miniUI.previewFile(output)}
            }
        }
        if compactWhileWorking && !autoCompact && state.activeTasks.isEmpty && previous?.activeTasks.isEmpty == false && !miniOpen {
            compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true);setMotion("jumping",replay:true)
        }
        miniUI.finishPending(requestID:state.completedRequestId)
        if let pendingFilesRequestID,state.completedRequestId == pendingFilesRequestID {self.pendingFilesRequestID=nil;indicator.setFileLoading(false)}
        if !state.activeTasks.isEmpty && !open {autoCompact=true;compactWhileWorking=true;setOpen(true,mini:false)}
        if state.activeTasks.isEmpty && autoCompact && !miniOpen {autoCompact=false;DispatchQueue.main.asyncAfter(deadline:.now()+1.2){[weak self] in guard let self,self.latestState?.activeTasks.isEmpty == true,!self.miniOpen else{return};self.compactWhileWorking=false;self.setOpen(false)}}
    }
    private func updateFeatureChecks(){
        speechItem?.state=speakResponses ? .on:.off;compactSendItem?.state=compactOnSend ? .on:.off
        youtubeItem?.state=youtubeMedia ? .on:.off;spotifyItem?.state=spotifyMedia ? .on:.off
    }
    @objc private func toggleContextMode(_ item:NSMenuItem){contextLightweight.toggle();miniUI.setContextMode(contextLightweight);item.state=contextLightweight ? .on:.off}
    @objc private func toggleSpeech(){speakResponses.toggle();UserDefaults.standard.set(speakResponses,forKey:"speakResponses");speech.configure(enabled:speakResponses,model:speech.selectedModel);updateFeatureChecks()}
    @objc private func toggleCompactSend(){compactOnSend.toggle();UserDefaults.standard.set(compactOnSend,forKey:"compactOnSend");updateFeatureChecks()}
    @objc private func stopSpeaking(){speech.cancelAudio()}
    @objc private func rescanVoices(){speech.discover()}
    @objc private func selectVoice(_ item:NSMenuItem){guard let path=item.representedObject as? String else{return};UserDefaults.standard.set(path,forKey:"speechModel");speech.configure(enabled:speakResponses,model:path);updateVoiceMenu(speech.models,configure:false)}
    private func updateVoiceMenu(_ models:[String],configure:Bool=true){
        voiceMenu.removeAllItems()
        let saved=UserDefaults.standard.string(forKey:"speechModel") ?? ""
        let selected=models.contains(saved) ? saved:models.first ?? ""
        for model in models {let item=voiceMenu.addItem(withTitle:URL(fileURLWithPath:model).deletingPathExtension().lastPathComponent,action:#selector(selectVoice(_:)),keyEquivalent:"");item.target=self;item.representedObject=model;item.state=model == selected ? .on:.off}
        if models.isEmpty {let item=voiceMenu.addItem(withTitle:"No compatible Piper ONNX voices found",action:nil,keyEquivalent:"");item.isEnabled=false}
        if configure {speech.configure(enabled:speakResponses,model:selected)}
    }
    @objc private func toggleYouTube(){youtubeMedia.toggle();UserDefaults.standard.set(youtubeMedia,forKey:"youtubeMedia");configureMedia()}
    @objc private func toggleSpotify(){spotifyMedia.toggle();UserDefaults.standard.set(spotifyMedia,forKey:"spotifyMedia");configureMedia()}
    private func configureMedia(){updateFeatureChecks();media.configure(youtube:youtubeMedia,spotify:spotifyMedia);if !youtubeMedia && !spotifyMedia{dismissMedia()}}
    private var diagnosticMeter:PlaybackMeter?
    @objc private func diagnoseMedia(){
        media.diagnose()
        guard diagnosticMeter == nil else{return}
        let meter=PlaybackMeter();diagnosticMeter=meter;meter.start(application:"diagnostic")
        DispatchQueue.main.asyncAfter(deadline:.now()+4){[weak self] in meter.diagnose();meter.stop();self?.diagnosticMeter=nil}
    }
    @objc private func mediaPermissions(){
        let options=[kAXTrustedCheckOptionPrompt.takeUnretainedValue() as String:true] as CFDictionary
        _=AXIsProcessTrustedWithOptions(options);_=CGRequestScreenCaptureAccess()
        media.capture(false)
        refreshMedia()
    }
    private func receiveMedia(_ state:MediaNowPlaying?){
        guard let state else{
            guard mediaState != nil,mediaReleaseWork == nil else{return}
            let work=DispatchWorkItem{[weak self] in guard let self else{return};self.mediaReleaseWork=nil;self.artworkTask?.cancel();self.mediaState=nil;self.mediaImage=nil;self.mediaProfile=nil;self.loadedMediaKey=nil;self.dismissMedia()}
            mediaReleaseWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.1,execute:work);return
        }
        mediaReleaseWork?.cancel();mediaReleaseWork=nil
        if mediaState == state{refreshMedia();return}
        artworkTask?.cancel();mediaState=state
        guard let url=URL(string:state.artwork),url.scheme == "https" else{return}
        artworkTask=URLSession.shared.dataTask(with:url){[weak self] data,_,_ in
            DispatchQueue.main.async{ [weak self] in guard let self,self.mediaState == state else{return};guard let data,let image=NSImage(data:data) else{self.mediaImage=nil;self.mediaProfile=nil;self.loadedMediaKey=nil;self.dismissMedia();return};self.mediaImage=image;self.mediaProfile=MediaNowPlaying.profile(image);self.loadedMediaKey=state.application+":"+state.identity+":"+state.artwork;self.refreshMedia()}
        };artworkTask?.resume();refreshMedia()
    }
    private func refreshMedia(){
        let busy=miniOpen || dictating || miniListening || compactWhileWorking || experimentalWebMode || sendingPrompt != nil || latestState?.activeTurnId != nil || latestState?.approval == true || !(latestState?.activeTasks.isEmpty ?? true)
        guard !busy,mediaState != nil,let image=mediaImage else{if mediaVisible{dismissMedia()};return}
        let entering = !mediaVisible
        if entering {mediaVisible=true;setOpen(true,mini:false);playToken += 1;visualTransition.reveal(sprite,visible:false);layoutMediaArtwork(animated:false);indicator.setExternalLevels([0,0,0,0]);indicator.startListening(useMicrophone:false)}
        media.capture(true)
        if entering || presentedMediaKey != loadedMediaKey {
            presentedMediaKey=loadedMediaKey
            if let cgImage=image.cgImage(forProposedRect:nil,context:nil,hints:nil){visualTransition.artwork(cgImage,on:mediaArtwork)}
            if let profile=mediaProfile{indicator.setProfile(profile,wave:true)}
        }
    }
    private func dismissMedia(closeNotch:Bool=true){
        guard mediaVisible else{return};mediaVisible=false;currentScene="";media.capture(false);presentedMediaKey=nil;visualTransition.hideArtwork(mediaArtwork);sprite.isHidden=false;visualTransition.reveal(sprite,visible:true);play();if let state=latestState{showActivity(state.activity)};if closeNotch && !miniOpen && !compactWhileWorking{setOpen(false)}
    }
    private func syncTaskPet(_ state:KaiState){
        let ids=Set(state.activeTasks.map(\.id)),pets=catalog.pets.map(\.id)
        var occupied=Set(activeTaskIDs.intersection(ids).compactMap{taskPets[$0]})
        for task in state.activeTasks.reversed(){
            if taskPets[task.id] == nil {
                if occupied.isEmpty {
                    taskPets[task.id]=petID
                } else {
                    let available=pets.filter{!occupied.contains($0)}
                    taskPets[task.id]=(available.isEmpty ? pets:available).randomElement() ?? userPetID
                }
            }
            if let assigned=taskPets[task.id]{occupied.insert(assigned)}
        }
        activeTaskIDs=ids
        if let focus=state.focusThreadId {
            let assigned=taskPets[focus] ?? petID
            if focusedTaskID != focus || petID != assigned {focusedTaskID=focus;petID=assigned;currentScene="";play()}
        } else {
            focusedTaskID=nil
        }
    }
    private func closeNotch(){
        hidePetGallery(restore:false)
        if dictating{finishDictation()}
        pinned=false
        if let state=latestState, state.activeTurnId != nil || !state.activeTasks.isEmpty || state.activity.scene == "thinking" {
            compactWhileWorking=true;compactThreadID=state.focusThreadId ?? state.threadId;compactStartHistoryCount=state.history.count;setOpen(true,mini:false)
        } else {compactWhileWorking=false;compactThreadID=nil;setOpen(false)}
    }
    private func latestOutputCitation(in history:[KaiMessage])->URL?{
        for block in history.reversed() where block.kind == "assistant" {
            for line in block.lines.reversed() where line.style != "code" {
                for citation in fileCitations(in:line.text).reversed() where citation.purpose == "output" {
                    if FileManager.default.fileExists(atPath:citation.url.path){return citation.url}
                }
            }
        }
        return nil
    }

    private func showActivity(_ activity:KaiActivity){
        if mediaVisible && activity.scene == "off"{return}
        guard currentScene != activity.scene else{return};currentScene=activity.scene
        sounds.playScene(activity.scene)
        switch activity.scene {case "off":indicator.hide();case "thinking":indicator.showThinking();default:indicator.showScene(activity.scene)}
    }

    private func setMotion(_ id:String,replay:Bool=false){
        guard catalog.stateAnimations[id] != nil,(motionID != id || replay) else{return}
        motionID=id;play();refreshChecks()
    }
    private func syncMotion(force:Bool=false){
        if !force && ["waving","running-left","jumping","failed"].contains(motionID){return}
        let motion:String
        if dictating || miniListening || (sendingPrompt == nil && !(composerField?.stringValue.isEmpty ?? true)) {motion="waiting"}
        else if latestState?.activity.scene == "permission" || latestState?.activity.scene == "question" {motion="waiting"}
        else if sendingPrompt != nil || latestState?.activeTurnId != nil || (latestState?.activity.scene != "off" && latestState?.activity.scene != "failure" && latestState != nil) {motion="running"}
        else {motion="idle"}
        setMotion(motion)
    }

    private func renderConversation(_ history:[KaiMessage]){
        guard let view=conversationText,let scroll=conversationScroll else{return}
        let threadID=latestState?.threadId,changedThread=threadID != renderedThreadID
        if changedThread {renderedThreadID=threadID;renderedBlocks.removeAll();renderedRanges.removeAll()}
        let signatures=history.map{ block in block.kind+"\u{1f}"+block.id+"\u{1f}"+block.lines.map{$0.style+"\u{1e}"+$0.text+"\u{1e}"+String($0.mathEnabled ?? true)}.joined(separator:"\u{1f}") }
        var first=0
        while first < min(signatures.count,renderedBlocks.count),signatures[first] == renderedBlocks[first] {first += 1}
        guard changedThread || first < signatures.count || first < renderedBlocks.count else{return}
        let nearBottom=changedThread || (scroll.verticalScroller?.floatValue ?? 1) > 0.93
        let content=NSMutableAttributedString(string:"")
        let pathPattern=#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#
        let pathRegex=try? NSRegularExpression(pattern:pathPattern)
        let palette=assistantPalette.isEmpty ? [NSColor.white] : assistantPalette
        var ranges=Array(renderedRanges.prefix(first))
        let startLocation=changedThread ? 0 : (first < renderedRanges.count ? renderedRanges[first].location : (view.textStorage?.length ?? 0))
        for block in history.dropFirst(first) {
            let blockStart=startLocation+content.length
            let visualLines=renderMath ? MathExpression.collapsedLines(block.lines) : block.lines
            let displayed=visualLines.map{block.kind == "assistant" && $0.style != "code" ? displayCitations(in:$0.text) : ($0.text,[(NSRange,URL)]())}
            let totalCharacters=max(1,displayed.reduce(0){$0+$1.0.count+1})
            var characterOffset=0
            for (line,rendered) in zip(visualLines,displayed) {
            let displayText=rendered.0
            let lineCharacterStart=characterOffset
            let style=line.style,base=NSFont.systemFont(ofSize:9),font:NSFont
            switch style {case "heading":font=NSFont.systemFont(ofSize:9,weight:.semibold);case "code":font=NSFont.monospacedSystemFont(ofSize:8,weight:.regular);default:font=base}
            let paragraph=NSMutableParagraphStyle();paragraph.lineSpacing=2
            let start=content.length
            let attributes:[NSAttributedString.Key:Any]=[.font:font,.paragraphStyle:paragraph]
            if block.kind == "assistant" {
                content.append(NSAttributedString(string:displayText+"\n",attributes:attributes))
                var utf16Offset=0,runStart=0,runColorIndex:Int?
                for character in displayText {
                    let index=min(palette.count-1,characterOffset*(palette.count-1)/max(1,totalCharacters-1))
                    if let runColorIndex,runColorIndex != index {
                        content.addAttribute(.foregroundColor,value:palette[runColorIndex],range:NSRange(location:start+runStart,length:utf16Offset-runStart))
                        runStart=utf16Offset
                    }
                    runColorIndex=index
                    utf16Offset += character.utf16.count
                    characterOffset += 1
                }
                if let runColorIndex {content.addAttribute(.foregroundColor,value:palette[runColorIndex],range:NSRange(location:start+runStart,length:utf16Offset-runStart))}
                content.addAttribute(.foregroundColor,value:palette[min(palette.count-1,characterOffset*(palette.count-1)/max(1,totalCharacters-1))],range:NSRange(location:start+utf16Offset,length:1))
            } else {
                content.append(NSAttributedString(string:displayText+"\n",attributes:attributes.merging([.foregroundColor:NSColor.white.withAlphaComponent(0.86)]){$1}))
            }
            characterOffset += 1
            for (range,url) in rendered.1 {content.addAttributes([.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue],range:NSRange(location:start+range.location,length:range.length))}
            for match in pathRegex?.matches(in:displayText,range:NSRange(displayText.startIndex...,in:displayText)) ?? [] {
                let raw=(displayText as NSString).substring(with:match.range).trimmingCharacters(in:CharacterSet(charactersIn:".,;:"))
                let path=raw.hasPrefix("Users/") ? "/"+raw:raw
                let url=raw.hasPrefix("file://") ? URL(string:raw) : URL(fileURLWithPath:(path as NSString).expandingTildeInPath)
                if let url,FileManager.default.fileExists(atPath:url.path) {
                    var link:[NSAttributedString.Key:Any]=[.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue]
                    if block.kind != "assistant" {link[.foregroundColor]=NSColor.white}
                    content.addAttributes(link,range:NSRange(location:start+match.range.location,length:(raw as NSString).length))
                }
            }
            if style != "code" {
                for match in webLinkDetector.matches(in:displayText,range:NSRange(displayText.startIndex...,in:displayText)) {
                    guard let url=match.url,["http","https"].contains(url.scheme?.lowercased() ?? "") else{continue}
                    content.addAttributes([.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue],range:NSRange(location:start+match.range.location,length:match.range.length))
                }
            }
            if renderMath && style != "code" && line.mathEnabled != false {
                for math in MathExpression.find(in:displayText).reversed() {
                    let source=displayText as NSString
                    let before=source.substring(to:math.range.location).count
                    let extent=source.substring(with:math.range).count
                    func color(_ offset:Int)->NSColor {
                        block.kind == "assistant" ? palette[min(palette.count-1,offset*(palette.count-1)/max(1,totalCharacters-1))] : NSColor.white.withAlphaComponent(0.86)
                    }
                    guard let image=mathRenderer.image(for:math,startColor:color(lineCharacterStart+before),endColor:color(lineCharacterStart+before+extent)) else{continue}
                    let attachment=NSTextAttachment()
                    attachment.image=image
                    let scale=min(1,330/max(1,image.size.width))
                    attachment.bounds=CGRect(x:0,y:math.display ? -3:-2,width:image.size.width*scale,height:image.size.height*scale)
                    content.replaceCharacters(in:NSRange(location:start+math.range.location,length:math.range.length),with:NSAttributedString(attachment:attachment))
                }
            }
            }
            ranges.append(NSRange(location:blockStart,length:startLocation+content.length-blockStart))
        }
        let oldLength=view.textStorage?.length ?? 0
        view.textStorage?.replaceCharacters(in:NSRange(location:startLocation,length:oldLength-startLocation),with:content)
        renderedBlocks=signatures;renderedRanges=ranges
        if nearBottom {scrollConversationToBottom(threadID:threadID)}
    }
    private func scheduleMathRefresh(){
        guard renderMath,mathRefreshWork == nil else{return}
        let work=DispatchWorkItem { [weak self] in
            guard let self else{return}
            self.mathRefreshWork=nil
            guard self.renderMath,let state=self.latestState else{return}
            self.renderedBlocks.removeAll()
            self.renderConversation(state.history)
        }
        mathRefreshWork=work
        DispatchQueue.main.asyncAfter(deadline:.now()+0.2,execute:work)
    }
    private func assistantGradientColors()->[NSColor]{
        let stops=(gradientCatalog.profileByPetID[petID]?.gradients.ambient.stops ?? []).sorted{$0.location<$1.location}
        guard !stops.isEmpty else{return [.white]}
        let colors=stops.map{stop -> NSColor in
            let value=UInt64(stop.color.dropFirst(),radix:16) ?? 0
            return NSColor(srgbRed:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255,alpha:1)
        }
        return (0..<512).map{sample in
            let position=Double(sample)/511
            var upper=1
            while upper<stops.count-1 && position>stops[upper].location {upper += 1}
            let lower=max(0,upper-1),right=min(upper,stops.count-1)
            let span=max(0.0001,stops[right].location-stops[lower].location)
            let fraction=CGFloat(min(1,max(0,(position-stops[lower].location)/span)))
            let mixed=colors[lower].blended(withFraction:fraction,of:colors[right]) ?? colors[lower]
            var hue:CGFloat=0,saturation:CGFloat=0,brightness:CGFloat=0,alpha:CGFloat=0
            mixed.getHue(&hue,saturation:&saturation,brightness:&brightness,alpha:&alpha)
            return NSColor(calibratedHue:hue,saturation:min(saturation,0.82),brightness:max(brightness,0.74),alpha:1)
        }
    }
    private func scrollConversationToBottom(threadID:String?){
        DispatchQueue.main.async { [weak self] in
            guard let self,self.latestState?.threadId == threadID,let view=self.conversationText,let scroll=self.conversationScroll else{return}
            if let container=view.textContainer {view.layoutManager?.ensureLayout(for:container)}
            let bottom=max(0,view.bounds.height-scroll.contentView.bounds.height)
            scroll.contentView.scroll(to:NSPoint(x:0,y:bottom));scroll.reflectScrolledClipView(scroll.contentView)
        }
    }
    func textView(_ textView:NSTextView,clickedOnLink link:Any,at charIndex:Int)->Bool{guard let url=(link as? URL) ?? (link as? String).flatMap(URL.init(string:)) else{return false};if url.isFileURL{miniUI.previewFile(url);return true};guard ["http","https"].contains(url.scheme?.lowercased() ?? "") else{return false};NSWorkspace.shared.open(url);return true}

    private func beginDictation(trigger:DictationTrigger){guard !dictating else{return};speech.cancelAudio();dictating=true;dictationTrigger=trigger;sounds.startListening();dictationPrefix=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines);if !dictationPrefix.isEmpty{dictationPrefix += " "};setMotion("waiting");indicator.startListening(useMicrophone:false);dictation.start()}
    private func finishDictation(){guard dictating else{return};dictating=false;dictationTrigger=nil;sounds.stopListening();dictation.stop{[weak self] final in guard let self else{return};self.setPrompt(self.dictationPrefix+final);self.syncMotion(force:true)};if miniOpen{exitMiniListening()}else{setOpen(true,mini:true)}}

    private func enterMiniListening(trigger:DictationTrigger) {
        guard miniOpen,!miniListening else{return};hidePetGallery();miniListening=true;if miniUI.isFilesOpen{miniUI.toggleFiles()};indicator.setFileOpen(false);beginDictation(trigger:trigger)
        let center=(canvasWidth+notchWidth)/2+40,duration=0.62
        for layer in [indicator.layer,indicator.fileLayer] { let move=CABasicAnimation(keyPath:"position.x");move.fromValue=layer.presentation()?.position.x ?? layer.position.x;move.toValue=center;move.duration=duration;move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.position.x=center;CATransaction.commit();layer.add(move,forKey:"listenMerge") }
        setFileButtonVisible(false,duration:duration)
    }

    private func exitMiniListening(){guard miniListening else{return};miniListening=false;let center=(canvasWidth+notchWidth)/2+40,duration=0.62;indicator.showScene("settings");for(layer,target) in [(indicator.layer,center-20),(indicator.fileLayer,center+20)]{let move=CABasicAnimation(keyPath:"position.x");move.fromValue=layer.presentation()?.position.x ?? layer.position.x;move.toValue=target;move.duration=duration;move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.position.x=target;CATransaction.commit();layer.add(move,forKey:"listenSeparate")};setFileButtonVisible(true,duration:duration)}

    private func handleImageEntered(){guard !miniOpen else{return};sounds.playScene("image");pinned=true;if !open{setOpen(true)};indicator.showScene("image")}
    private func handleImageDrop(_ images:[NSImage]){guard !images.isEmpty,ProcessInfo.processInfo.systemUptime-lastImageDrop>0.35 else{return};lastImageDrop=ProcessInfo.processInfo.systemUptime;approachImages.removeAll();approachActive=false;miniUI.addImages(images);if miniOpen{return};sounds.playScene("success");indicator.showScene("success");imageExpandWork?.cancel();let work=DispatchWorkItem{[weak self] in guard let self,self.open,!self.miniOpen else{return};self.setOpen(true,mini:true)};imageExpandWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.72,execute:work)}
    private func globalDrag(_ event:NSEvent){guard !experimentalWebMode,let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil}) else{return};let p=NSEvent.mouseLocation,nearNotch=abs(p.x-screen.frame.midX)<notchWidth/2+45 && p.y>screen.frame.maxY-notchHeight-70;if event.type == .leftMouseDragged,nearNotch{let images=draggedImages(NSPasteboard(name:.drag));guard !images.isEmpty else{return};approachImages=images;if !approachActive{approachActive=true;handleImageEntered()}}else if event.type == .leftMouseUp{defer{approachActive=false;approachImages.removeAll()};if approachActive,nearNotch,!approachImages.isEmpty{handleImageDrop(approachImages)}}}

    @objc private func changeMediaSize(_ slider:NSSlider){mediaSize=CGFloat(slider.doubleValue);UserDefaults.standard.set(Double(mediaSize),forKey:"mediaSize");layoutMediaArtwork(animated:true)}
    private func layoutMediaArtwork(animated:Bool){
        guard let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil}) else{return}
        let expanded=notchWidth+160,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2
        let center=CGPoint(x:(physicalLeft+kaiLeft)/2,y:canvasHeight-notchHeight/2)
        let frame=NotchVisualTransition.artworkFrame(size:mediaSize,notchHeight:notchHeight,center:center,scale:screen.backingScaleFactor)
        visualTransition.resize(mediaArtwork,to:frame,animated:animated)
    }
    private func layoutPet() {
        guard let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil}) else{return};let expanded=notchWidth+160,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2,scale=screen.backingScaleFactor,topBandY=canvasHeight-notchHeight,h=min(36,notchHeight-3)*petScale,w=h*192/208
        func px(_ v:CGFloat)->CGFloat{round(v*scale)/scale};CATransaction.begin();CATransaction.setDisableActions(true);sprite.frame=CGRect(x:px((physicalLeft+kaiLeft)/2-w/2),y:px(topBandY+(notchHeight-h)/2),width:px(w),height:px(h));CATransaction.commit()
    }

    private func animateTopAccessories(forMini mini:Bool,duration:Double) {
        let shift:CGFloat=mini ? -20:0, currentX=indicator.layer.presentation()?.position.x ?? indicator.layer.position.x, targetX=(canvasWidth+notchWidth)/2+40+shift
        let move=CABasicAnimation(keyPath:"position.x"); move.fromValue=currentX; move.toValue=targetX; move.duration=duration; move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1)
        CATransaction.begin(); CATransaction.setDisableActions(true); indicator.layer.position.x=targetX; CATransaction.commit(); indicator.layer.add(move,forKey:"miniShift")
        setFileButtonVisible(mini && !miniListening,duration:duration)
    }

    private func setFileButtonVisible(_ visible:Bool,duration:Double){
        let button=indicator.fileLayer,target:Float=visible ? 1:0,targetScale:CGFloat=visible ? 1:(miniListening ? 0.45:0.72)
        let oldOpacity=button.presentation()?.opacity ?? button.opacity,oldScale=button.presentation()?.transform.m11 ?? button.transform.m11
        button.removeAnimation(forKey:"miniReveal");button.removeAnimation(forKey:"miniScale");button.removeAnimation(forKey:"listenMergeFade");button.removeAnimation(forKey:"listenFileReveal");button.removeAnimation(forKey:"fileVisibility");button.removeAnimation(forKey:"fileScale")
        CATransaction.begin();CATransaction.setDisableActions(true);button.opacity=target;button.transform=CATransform3DMakeScale(targetScale,targetScale,1);CATransaction.commit()
        guard duration>0 else{return}
        let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=oldOpacity;fade.toValue=target;fade.duration=duration;fade.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1);button.add(fade,forKey:"fileVisibility")
        let scale=CABasicAnimation(keyPath:"transform.scale");scale.fromValue=oldScale;scale.toValue=targetScale;scale.duration=duration;scale.timingFunction=fade.timingFunction;button.add(scale,forKey:"fileScale")
    }

    private func animateCornerAccents(toBottom bottom:Bool,duration:Double) {
        for layer in [indicator.leftAccentLayer,indicator.accentLayer] {
            let targetY=(bottom ? 0:canvasHeight-notchHeight)+layer.bounds.height/2
            let move=CABasicAnimation(keyPath:"position.y"); move.fromValue=layer.presentation()?.position.y ?? layer.position.y; move.toValue=targetY; move.duration=duration; move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1)
            CATransaction.begin(); CATransaction.setDisableActions(true); layer.position.y=targetY; CATransaction.commit(); layer.add(move,forKey:"cornerTravel")
        }
    }

    private func play() {
        if mediaVisible {return}
        guard let pet = catalog.pets.first(where: { $0.id == petID }), let motion = pet.animationOverrides?[motionID] ?? catalog.stateAnimations[motionID] else{return}
        let image:CGImage
        if loadedSpritePetID == petID,let loadedSprite {image=loadedSprite}
        else {
            guard let source=CGImageSourceCreateWithURL(assetRoot.appendingPathComponent(pet.spritesheet) as CFURL,nil),let decoded=CGImageSourceCreateImageAtIndex(source,0,nil) else{return}
            loadedSpritePetID=petID;loadedSprite=decoded;image=decoded
        }
        if renderedPetID != petID && !renderedPetID.isEmpty {let blend=CATransition();blend.type = .fade;blend.duration=0.36;blend.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);sprite.add(blend,forKey:"petBlend")}
        CATransaction.begin(); CATransaction.setDisableActions(true); sprite.contents = image; CATransaction.commit()
        if let profile = gradientCatalog.profileByPetID[petID] { indicator.setProfile(profile, wave: true); miniUI.setProfile(profile,animated:true);if renderedPetID != petID {renderedPetID=petID;assistantPalette=assistantGradientColors();renderedThreadID=nil;if let state=latestState{renderConversation(state.history)}} }
        sprite.contentsScale = NSScreen.main?.backingScaleFactor ?? 2; frame = 0; playToken += 1
        showFrame(pet, motion, playToken);if petGalleryVisible{petGallery.play(motionID)}
    }

    private func showFrame(_ pet: Pet, _ motion: Motion, _ token: Int) {
        guard token == playToken,open || compactWhileWorking || experimentalWebMode else { return }; let column = motion.columns[frame], rows = CGFloat(pet.grid.rows)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        sprite.contentsRect = CGRect(x: CGFloat(column)/8, y: 1-CGFloat(motion.row+1)/rows, width: 1/8, height: 1/rows)
        CATransaction.commit()
        let delay = Double(motion.durationsMs[frame])/1000,isLast=frame+1 == motion.columns.count
        frame = isLast ? 0:frame+1
        DispatchQueue.main.asyncAfter(deadline: .now()+delay) { [weak self] in
            guard let self,token == self.playToken else{return}
            if !motion.loop && isLast {self.syncMotion(force:true)}
            else {self.showFrame(pet,motion,token)}
        }
    }

    @objc private func toggleFromMenu() { cancelSizeTest();if compactWhileWorking {compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)}else if open{closeNotch()}else{pinned=true;setOpen(true,mini:true)} }
    @objc private func toggleExperimentalWebMode(){
        if !experimentalWebMode{modeBeforeExperimental=(open,miniOpen,pinned);experimentalWebMode=true;open=false;miniOpen=false;pinned=true;panel?.ignoresMouseEvents=false;composerField?.isHidden=true;conversationScroll?.isHidden=true}
        else{experimentalWebMode=false;browserView?.stopLoading();browserView?.removeFromSuperview();browserView=nil;open=modeBeforeExperimental.open;miniOpen=modeBeforeExperimental.mini;pinned=modeBeforeExperimental.pinned;panel?.ignoresMouseEvents = !miniOpen;composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen}
        UserDefaults.standard.set(experimentalWebMode,forKey:"experimentalWebMode")
        placePanel();refreshChecks()
    }
    private func ensureBrowser(){guard browserView == nil,let host=panel?.contentView else{return};let configuration=WKWebViewConfiguration();configuration.websiteDataStore = .default();let web=WKWebView(frame:.zero,configuration:configuration);web.autoresizingMask=[.width,.height];web.underPageBackgroundColor = .black;host.addSubview(web);browserView=web;web.load(URLRequest(url:URL(string:"https://chatgpt.com/")!))}
    @objc private func testSizeMorph() {
        cancelSizeTest(); pinned=true; setOpen(true)
        let grow=DispatchWorkItem { [weak self] in self?.setOpen(true,mini:true) }
        let shrink=DispatchWorkItem { [weak self] in self?.setOpen(true) }
        morphWork=[grow,shrink]; DispatchQueue.main.asyncAfter(deadline:.now()+0.85,execute:grow)
        DispatchQueue.main.asyncAfter(deadline:.now()+2.05,execute:shrink)
    }
    private func cancelSizeTest() { morphWork.forEach { $0.cancel() }; morphWork.removeAll() }
    private func usePet(_ id:String){userPetID=id;petID=id;if let chatID=focusedTaskID ?? latestState?.threadId{taskPets[chatID]=id};petGallery.layout(in:petGallery.layer.bounds,selected:id);refreshChecks();play()}
    @objc private func closePetPicker(){status.menu?.cancelTracking()}
    @objc private func toggleMathRendering(){
        renderMath.toggle()
        UserDefaults.standard.set(renderMath,forKey:"renderMath")
        if renderMath,let host=panel?.contentView,let scroll=conversationScroll { mathRenderer.install(in:host,below:scroll) }
        else if !renderMath { mathRenderer.suspend() }
        renderedBlocks.removeAll()
        if let state=latestState { renderConversation(state.history) }
        refreshChecks()
    }
    @objc private func toggleSounds(){soundsEnabled.toggle();sounds.enabled=soundsEnabled;UserDefaults.standard.set(soundsEnabled,forKey:"soundsEnabled");refreshChecks()}
    @objc private func randomizePet(){
        let occupied=Set(latestState?.activeTasks.compactMap{task in task.id == focusedTaskID ? nil:taskPets[task.id]} ?? [])
        let choices=catalog.pets.map(\.id).filter{$0 != petID && !occupied.contains($0)}
        let fallback=catalog.pets.map(\.id).filter{$0 != petID}
        usePet((choices.isEmpty ? fallback:choices).randomElement() ?? petID)
    }
    @objc private func selectMotion(_ item: NSMenuItem) { motionID = item.representedObject as! String; refreshChecks(); play() }
    @objc private func changePetSize(_ slider: NSSlider) { petScale = CGFloat(slider.doubleValue); UserDefaults.standard.set(slider.doubleValue, forKey: "petScale"); layoutPet() }
    @objc private func selectAccentSide(_ item: NSMenuItem) {
        accentSide = item.representedObject as! String; UserDefaults.standard.set(accentSide, forKey:"accentSide")
        applyAccentSide(); refreshChecks()
    }
    private func applyAccentSide() { indicator.setAccentVisibility(left:accentSide == "left" || accentSide == "both", right:accentSide == "right" || accentSide == "both") }
    @objc private func selectIndicator(_ item: NSMenuItem) {
        indicatorID = item.representedObject as! String
        if indicatorID == "cycle" {
            let wasMini=miniOpen
            autoCompact=false;compactWhileWorking=false;compactThreadID=nil;pinned=true
            setOpen(true,mini:false)
            if !wasMini {applyIndicator()}
            refreshChecks();return
        }
        if !miniOpen { applyIndicator() }
        if indicatorID != "off" { pinned = true; if !miniOpen { setOpen(true) } }; refreshChecks()
    }
    private func applyIndicator() {
        sounds.playScene(indicatorID)
        switch indicatorID {
        case "listening": indicator.startListening(useMicrophone: true)
        case "thinking": indicator.showThinking()
        case "demo": indicator.startDemo()
        case "pencil", "terminal", "git", "image", "document", "code", "search", "read", "files", "plan", "review", "permission", "question", "success", "failure", "declined", "waiting", "tool", "agents", "compact", "warning": indicator.showScene(indicatorID)
        case "cycle": indicator.startRandomCycle()
        default: indicator.hide()
        }
    }
    private func refreshChecks() {
        experimentalModeItem?.state=experimentalWebMode ? .on:.off
        mathModeItem?.state=renderMath ? .on:.off
        soundsItem?.state=soundsEnabled ? .on:.off
        petMenuRows.forEach { $0.value.selected = $0.key == petID }; motionItems.forEach { $0.value.state = $0.key == motionID ? .on:.off }
        indicatorItems.forEach { $0.value.state = $0.key == indicatorID ? .on:.off }
        accentItems.forEach { $0.value.state = $0.key == accentSide ? .on:.off }
        showItem?.title = open ? "hide notch":"show notch"
    }
}

@main struct Main {
    @MainActor static func main() {
        if CommandLine.arguments.contains("--diagnose-speaker-audio") {
            let meter=PlaybackMeter();var maximum:CGFloat=0,packets=0
            meter.onError={print($0)}
            meter.onLevels={levels in packets += 1;maximum=max(maximum,levels.max() ?? 0)}
            meter.start(application:"diagnostic")
            RunLoop.main.run(until:Date().addingTimeInterval(4))
            meter.stop();print("Speaker meter packets=\(packets) peak=\(maximum)");return
        }
        let app = NSApplication.shared, delegate = KaiPetApp(); app.delegate = delegate; app.run()
    }
}
