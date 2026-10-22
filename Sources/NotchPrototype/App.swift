import AppKit
import ImageIO
import WebKit
import UniformTypeIdentifiers

private struct Catalog: Decodable { let pets: [Pet]; let stateAnimations: [String: Motion] }
private struct Pet: Decodable { let id, displayName, spritesheet: String; let grid: Grid; let animationOverrides:[String:Motion]? }
private struct Grid: Decodable { let rows: Int }
private struct Motion: Decodable { let row: Int; let columns, durationsMs: [Int]; let loop: Bool }
struct FileCitation { let range:NSRange;let url:URL;let purpose:String }
private let citationPattern=try! NSRegularExpression(pattern:#":?codex-file-citation\{[^}\n]*\}"#)
private let citationPathPattern=try! NSRegularExpression(pattern:#"\bpath="([^"]+)""#)
private let citationPurposePattern=try! NSRegularExpression(pattern:#"\bpurpose="([^"]+)""#)
private let localFileTagPattern=try! NSRegularExpression(pattern:#"<((?:/|~/|file://)[^<>\n]+)>"#)
let webLinkDetector=try! NSDataDetector(types:NSTextCheckingResult.CheckingType.link.rawValue)

func fileCitations(in text:String)->[FileCitation]{
    let source=text as NSString
    var citations=citationPattern.matches(in:text,range:NSRange(location:0,length:source.length)).compactMap{match -> FileCitation? in
        let token=source.substring(with:match.range),tokenSource=token as NSString
        guard let pathMatch=citationPathPattern.firstMatch(in:token,range:NSRange(location:0,length:tokenSource.length)) else{return nil}
        let rawPath=tokenSource.substring(with:pathMatch.range(at:1)),path=(rawPath as NSString).expandingTildeInPath
        let url:URL
        if let remote=URL(string:rawPath),["https","http"].contains(remote.scheme?.lowercased() ?? ""),remote.host != nil {url=remote}
        else if path.hasPrefix("/"){url=URL(fileURLWithPath:path).standardizedFileURL}
        else{return nil}
        let purpose=citationPurposePattern.firstMatch(in:token,range:NSRange(location:0,length:tokenSource.length)).map{tokenSource.substring(with:$0.range(at:1))} ?? ""
        return FileCitation(range:match.range,url:url,purpose:purpose)
    }
    // Markdown's [label](</path with spaces>) is displayed by the bridge as
    // label (</path with spaces>). Keep the complete path instead of letting
    // the ordinary path detector stop at the first space in Application Support.
    for match in localFileTagPattern.matches(in:text,range:NSRange(location:0,length:source.length)) {
        guard !citations.contains(where:{NSIntersectionRange($0.range,match.range).length>0}) else{continue}
        let raw=source.substring(with:match.range(at:1))
        // A closing markup tag such as </realtime_delegation> is not a
        // filesystem reference. Root-level files need a filename extension.
        if raw.hasPrefix("/"),!raw.dropFirst().contains("/"),(raw as NSString).pathExtension.isEmpty{continue}
        let url:URL
        if raw.hasPrefix("file://") {
            guard let parsed=URL(string:raw),parsed.isFileURL,parsed.host == nil || parsed.host == "" || parsed.host == "localhost" else{continue}
            url=parsed.standardizedFileURL
        } else {url=URL(fileURLWithPath:(raw as NSString).expandingTildeInPath).standardizedFileURL}
        var range=match.range
        if range.location>0,NSMaxRange(range)<source.length,source.substring(with:NSRange(location:range.location-1,length:1)) == "(",source.substring(with:NSRange(location:NSMaxRange(range),length:1)) == ")" {
            range=NSRange(location:range.location-1,length:range.length+2)
        }
        citations.append(FileCitation(range:range,url:url,purpose:""))
    }
    return citations.sorted{$0.range.location<$1.range.location}
}

func displayCitations(in text:String)->(String,[(NSRange,URL)]){
    let source=text as NSString,citations=fileCitations(in:text)
    guard !citations.isEmpty else{return(text,[])}
    var rendered="",cursor=0,links=[(NSRange,URL)]()
    for citation in citations {
        rendered += source.substring(with:NSRange(location:cursor,length:citation.range.location-cursor))
        let url=citation.url
        let label:String
        if url.host == "docs.google.com" {
            let kind=url.pathComponents.contains("presentation") ? "Google Slides":url.pathComponents.contains("spreadsheets") ? "Google Sheets":"Google Docs"
            label="▧  "+kind+"  ↗"
        }else if url.isFileURL {label="▧  "+url.lastPathComponent}
        else{label="▧  "+(url.host ?? "Document")+"  ↗"}
        let location=(rendered as NSString).length
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

/// Drag motion only negotiates types; decoding belongs to the actual drop.
private func hasDraggedImages(_ pasteboard:NSPasteboard)->Bool {
    if pasteboard.availableType(from:[.png,.tiff]+NSImage.imageTypes.map{NSPasteboard.PasteboardType($0)}) != nil{return true}
    let urls=(pasteboard.readObjects(forClasses:[NSURL.self],options:[.urlReadingFileURLsOnly:true]) as? [URL]) ?? []
    return urls.contains{UTType(filenameExtension:$0.pathExtension)?.conforms(to:.image) == true}
}

final class NotchPanel:NSPanel {
    override var canBecomeKey:Bool{true}
    override var canBecomeMain:Bool{false}
    override func performKeyEquivalent(with event:NSEvent)->Bool {
        // This accessory app has no standard Edit menu to dispatch these keys.
        // Send them through the focused editor's responder chain instead.
        let flags=event.modifierFlags.intersection([.command,.shift,.control,.option])
        if flags == .command || flags == [.command,.shift] {
            let key=event.charactersIgnoringModifiers?.lowercased()
            let selector:Selector?
            switch (key,flags.contains(.shift)) {
            case ("a",false):selector=NSSelectorFromString("selectAll:")
            case ("c",false):selector=NSSelectorFromString("copy:")
            case ("v",false):selector=NSSelectorFromString("paste:")
            case ("x",false):selector=NSSelectorFromString("cut:")
            case ("z",false):selector=NSSelectorFromString("undo:")
            case ("z",true):selector=NSSelectorFromString("redo:")
            default:selector=nil
            }
            if let selector,firstResponder?.tryToPerform(selector,with:nil) == true{return true}
        }
        return super.performKeyEquivalent(with:event)
    }
    // This overlay deliberately occupies the menu bar rather than the usual
    // visible frame reserved for application windows.
    override func constrainFrameRect(_ frameRect:NSRect,to screen:NSScreen?)->NSRect {frameRect}
}

private class ImageDropView:NSView {
    var dragEntered:(()->Void)?,imageDropped:(([NSImage])->Void)?
    override init(frame:NSRect){super.init(frame:frame);registerForDraggedTypes([.fileURL,.png,.tiff]+NSImage.imageTypes.map{NSPasteboard.PasteboardType($0)})}
    required init?(coder:NSCoder){fatalError()}
    private func images(_ sender:NSDraggingInfo)->[NSImage]{draggedImages(sender.draggingPasteboard)}
    override func draggingEntered(_ sender:NSDraggingInfo)->NSDragOperation{guard hasDraggedImages(sender.draggingPasteboard) else{return []};dragEntered?();return .copy}
    override func draggingUpdated(_ sender:NSDraggingInfo)->NSDragOperation{hasDraggedImages(sender.draggingPasteboard) ? .copy:[]}
    override func performDragOperation(_ sender:NSDraggingInfo)->Bool{let images=images(sender);guard !images.isEmpty else{return false};imageDropped?(images);return true}
}

private final class ComposerField:NSTextField {
    var imagesEntered:(()->Void)?,imagesDropped:(([NSImage])->Void)?
    override init(frame:NSRect){super.init(frame:frame);registerForDraggedTypes([.fileURL,.png,.tiff]+NSImage.imageTypes.map{NSPasteboard.PasteboardType($0)})}
    required init?(coder:NSCoder){fatalError()}
    override func performKeyEquivalent(with event:NSEvent)->Bool{let flags=event.modifierFlags.intersection(.deviceIndependentFlagsMask);if flags == .command,event.charactersIgnoringModifiers?.lowercased() == "a"{if let editor=currentEditor(){editor.selectAll(nil)}else{window?.makeFirstResponder(self);selectText(nil)};return true};return super.performKeyEquivalent(with:event)}
    override func draggingEntered(_ sender:NSDraggingInfo)->NSDragOperation{guard hasDraggedImages(sender.draggingPasteboard) else{return []};imagesEntered?();return .copy}
    override func draggingUpdated(_ sender:NSDraggingInfo)->NSDragOperation{hasDraggedImages(sender.draggingPasteboard) ? .copy:[]}
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
    override func updateTrackingAreas(){super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea);addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseMoved,.mouseEnteredAndExited,.activeAlways],owner:self))}
    override func mouseDown(with event:NSEvent) { click?(event.locationInWindow,event.clickCount) }
    override func scrollWheel(with event:NSEvent) { scroll?(event.locationInWindow,event.scrollingDeltaY) }
    override func mouseMoved(with event:NSEvent){hover?(event.locationInWindow)}
    override func mouseExited(with event:NSEvent){hover?(nil)}
}

@MainActor final class KaiPetApp: NSObject, NSApplicationDelegate, NSTextFieldDelegate, NSTextViewDelegate {
    private let glassBackground=NotchGlassBackground(frame:.zero)
    private var codeConnectionItem:NSMenuItem?
    private var glassSliders=[String:NSSlider](),glassLabels=[String:NSTextField](),glassChoiceItems=[NSMenuItem]()
    private var glassEnabled=UserDefaults.standard.bool(forKey:"notchBottomGlass"),glassItem:NSMenuItem?
    private let mask = CAShapeLayer(), notchContent = CALayer(), sprite = CALayer(), indicator = NotchIndicatorEngine(), miniUI = MiniAppInterface(), petGallery=PetGallery(), dictation=DictationController(), bridge=KaiBridge(),sounds=KaiSoundEngine()
    private var panel: NSPanel!, dropPanel:NSPanel!, composerField:ComposerField!, status: NSStatusItem!, catalog: Catalog!, gradientCatalog: GradientCatalog!
    private var browserView:WKWebView?, experimentalModeItem:NSMenuItem!, mathModeItem:NSMenuItem!,soundsItem:NSMenuItem!
    private let speech=SpeechPlayback(),media=MediaCompanion()
    private let chatVoice=ChatVoiceController()
    private let realtimeVoiceMenu=RealtimeVoiceMenu()
    private let defaultModelMenu=DefaultModelMenu(),shortcutTimingMenu=ShortcutTimingMenu()
    private let voicePrompt=HoldToVoiceController()
    private var compactVoiceMode=false,compactVoiceError:String?
    private var voiceExpanded=false
    private var voiceApprovalMenuItem:NSMenuItem?
    private var compactVoiceIndicator:CompactVoicePresentation.Activity?
    private var pendingVoiceStart:(action:ShortcutAction,audio:Data,context:ApplicationContext?,source:String?)?
    private var voiceCaptureSource:String?,voiceLaunchTimeout:DispatchWorkItem?
    private var chatVoiceMenuItem:NSMenuItem?
    private var speakResponses=false,compactOnSend=false,youtubeMedia=false,spotifyMedia=false
    private var voiceMenu=NSMenu(),speechItem:NSMenuItem!,compactSendItem:NSMenuItem!,youtubeItem:NSMenuItem!,spotifyItem:NSMenuItem!,featureStatusItem:NSMenuItem!
    private var mediaState:MediaNowPlaying?,mediaImage:NSImage?,mediaProfile:PetGradientProfile?,mediaVisible=false,artworkTask:URLSessionDataTask?
    private let mediaArtwork=CALayer(),visualTransition=NotchVisualTransition()
    private var mediaReleaseWork:DispatchWorkItem?
    private var mediaSize:CGFloat=32,mediaSizeSlider:NSSlider?,loadedMediaKey:String?,presentedMediaKey:String?
    private let mathRenderer=MathRenderer()
    private var renderMath=true,soundsEnabled=true,petMenuRows=[String:PetMenuRow]()
    private var earlierHistoryRequest:String?
    private let historyControl=ConversationHistoryControl()
    private var mathRefreshWork:DispatchWorkItem?
    private lazy var conversationRenderer=ConversationRenderer(mathRenderer:mathRenderer)
    private var conversationScroll:NSScrollView!,conversationText:NSTextView!,latestState:KaiState?,currentScene="",sendingPrompt:String?,attachmentFiles=[URL](),queuedImageCount=0
    private let imageEncodingQueue:OperationQueue = {let queue=OperationQueue();queue.name="kai.attachment-encoding";queue.maxConcurrentOperationCount=1;queue.qualityOfService = .userInitiated;return queue}()
    private var imageEncodingWork:BlockOperation?,attachmentGeneration=UUID(),attachmentEncoding=false,pendingAttachmentSend=false
    private let fileViewer=FileViewerContent(),fileActions=FileActionsMenu()
    private var motionItems = [String: NSMenuItem](), indicatorItems = [String: NSMenuItem](), accentItems = [String: NSMenuItem]()
    private var localMonitor: Any?, globalMonitor: Any?, dragMonitor:Any?, showItem: NSMenuItem!
    private var morphWork = [DispatchWorkItem](),petClickWork:DispatchWorkItem?
    private let petSelection=PetSelectionPreferences(defaults:.standard)
    private var petFilterItem:NSMenuItem?,explicitTaskPets=Set<String>()
    private var petID = "codex", userPetID = "codex", renderedPetID = "", motionID = "idle", indicatorID = "demo", frame = 0, playToken = 0
    private var taskPets=[String:String](),activeTaskIDs=Set<String>(),focusedTaskID:String?,autoCompact=false
    private var loadedSpritePetID:String?,loadedSprite:CGImage?
    private var assistantPalette=[NSColor]()
    private var conversationPetID=""
    private var sendingPromptRequestID:String?
    private var expandedChatRequest:(id:String,target:String,name:String)?
    private let spriteAssets=SpriteAssetCache()
    private var loadingSpritePetID:String?
    private var petScale: CGFloat = 1
    private var accentSide = "right"
    private let autoSendPreferences=DictationSendPreferences(defaults:.standard)
    private var autoSendMenu=NSMenu()
    private let shortcutPreferences=ShortcutPreferences(defaults:.standard)
    private var shortcutMenu=NSMenu()
    private var expandedDictationItem:NSMenuItem?
    private var shortcutWork:DispatchWorkItem?
    private var shortcutGeneration=UUID()
    private var shortcutGesture=ShortcutGesture()
    private var pendingAutoSend=false
    private var dictationGeneration=UUID()
    private var panelRevision=0
    private var open = false, miniOpen = false, pinned = false, commandDown = false, optionDown = false, controlDown = false, functionDown = false,petGalleryVisible=false
    private let shortcutPreparation=ShortcutPreparation()
    private var dictationDestination:String?
    private var lastOpenedChatID=UserDefaults.standard.string(forKey:"lastOpenedChatID")
    private var contextLightweight = false
    private var dictationTrigger:DictationTrigger?
    private var experimentalWebMode=false
    private var modeBeforeExperimental=(open:false,mini:false,pinned:false)
    private var compactWhileWorking=false,presentedCompact=false,compactThreadID:String?,compactStartHistoryCount=0
    private var pendingFilesRequestID:String?
    private var miniListening = false,dictating=false,finalizingDictation=false,dictationPrefix=""
    private var approachImages=[NSImage](),approachActive=false,lastImageDrop=0.0
    private var imageExpandWork:DispatchWorkItem?
    private var lastCommandTap = 0.0, lastControlTap = 0.0
    private var holdWork: DispatchWorkItem?
    private let scalableUI=NSView(),resizeHandles=NotchResizeHandles()
    private var expandedSize:CGSize?,resizeInitial=CGSize.zero,resizeMode=NotchResizeMode.both
    private var defaultWidthSlider:NSSlider?,defaultHeightSlider:NSSlider?
    private var sizeLabels=[String:NSTextField]()
    private var expandedWidth:CGFloat { (miniOpen || experimentalWebMode) ? (expandedSize?.width ?? notchWidth+160) : notchWidth+160 }
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
        NSApp.setActivationPolicy(.regular)
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
        do {
            catalog = try JSONDecoder().decode(Catalog.self,from:Data(contentsOf:assetRoot.appendingPathComponent("animation_catalog.json")))
            gradientCatalog = try JSONDecoder().decode(GradientCatalog.self,from:Data(contentsOf:assetRoot.appendingPathComponent("gradient_profiles.json")))
            guard !catalog.pets.isEmpty else{throw NSError(domain:"KaiAssets",code:1,userInfo:[NSLocalizedDescriptionKey:"Pet catalog is empty"])}
        } catch {
            let alert=NSAlert();alert.messageText="Kai could not load its assets";alert.informativeText=error.localizedDescription;alert.runModal();NSApp.terminate(nil);return
        }
        if let initial=petSelection.initialID(catalog.pets.map(\.id)){petID=initial;userPetID=initial}
        petGallery.configure(catalog.pets.map{pet in
            var motions=catalog.stateAnimations;pet.animationOverrides?.forEach{motions[$0.key]=$0.value}
            let galleryMotions=motions.mapValues{PetGalleryMotion(row:$0.row,columns:$0.columns,durationsMs:$0.durationsMs,loop:$0.loop)}
            let stops=gradientCatalog.profileByPetID[pet.id]?.gradients.ambient.stops ?? []
            let colors=stops.map{stop -> CGColor in let value=UInt64(stop.color.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255,alpha:0.9).cgColor}
            return PetGallerySpec(id:pet.id,spritesheet:pet.spritesheet,rows:pet.grid.rows,motions:galleryMotions,colors:colors)
        },at:assetRoot)
        dictation.onText={ [weak self] text in guard let self,self.dictating else{return};self.setPrompt(self.dictationPrefix+text,dictation:true) };dictation.onLevels={ [weak self] levels in self?.indicator.setExternalLevels(levels) }
        dictation.onError={ [weak self] error in guard let self else{return};self.cancelShortcutInput();self.miniListening=false;self.indicator.hide();self.miniUI.setCodexMessage(error);self.syncMotion(force:true) }
        buildMenu(); applyAccentSide(); placePanel(); play(); indicator.hide()
        miniUI.onProject={ [weak self] path in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("project",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onChat={ [weak self] id in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("chat",["id":id,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNew={ [weak self] in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("new",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNewInProject={ [weak self] in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("new_project",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onFolder={ [weak self] path in guard let self else{return};self.bridge.send("folder",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onSettings={ [weak self] model, effort in self?.bridge.send("settings",["model":model,"effort":effort]) }
        miniUI.onNavigate={ [weak self] direction in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("navigate",["direction":direction,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onContextMode={ [weak self] lightweight in self?.contextLightweight=lightweight }
        miniUI.onSend={ [weak self] in self?.sendPrompt() }
        miniUI.onVoice={ [weak self] in self?.toggleChatVoice() }
        miniUI.onFocusComposer={ [weak self] in guard let self else{return};self.panel.makeKey();self.panel.makeFirstResponder(self.composerField) }
        miniUI.onFileActions={ [weak self] url in self?.showFileActions(url) }
        miniUI.onFileAction={ [weak self] action,url in self?.fileActions.perform(action,url:url) }
        fileActions.onPerformed={ [weak self] action,url in
            let stamp=FileActionSymbols.confirm(action,url:url)
            self?.animateFileAction(action,url:url,stamp:stamp)
        }
        fileViewer.onFileActions={ [weak self] url in self?.showFileActions(url) }
        shortcutPreparation.onCommand={ [weak self] action,fields in self?.bridge.send(action,fields) }
        shortcutPreparation.capture={ lightweight,completion in
            DispatchQueue.global(qos:.userInitiated).async {
                let context=ApplicationContext.capture(lightweight:lightweight)
                DispatchQueue.main.async {completion(context)}
            }
        }
        shortcutPreparation.onRetainContext={ [weak self] context in
            if let path=context.imagePath {self?.attachmentFiles.append(URL(fileURLWithPath:path))}
        }
        shortcutPreparation.onDictation={ [weak self] trigger,clearDraft in
            guard let self else{return}
            if clearDraft, !(self.composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || self.queuedImageCount > 0 {
                self.cancelShortcutInput();self.miniUI.setCodexMessage("Send or clear your draft before starting a new chat");return
            }
            self.dictationDestination=nil
            if clearDraft {self.setPrompt("")}
            self.pinned=true
            if self.miniOpen {self.enterMiniListening(trigger:trigger)}
            else {self.setOpen(true);self.beginDictation(trigger:trigger)}
        }
        shortcutPreparation.onReady={ [weak self] action,target,context in
            guard let self,self.latestState?.threadId == target else{return}
            if action.isVoice {
                let prompt=self.pendingVoiceStart?.audio
                self.pendingVoiceStart=nil;self.voiceLaunchTimeout?.cancel();self.voiceLaunchTimeout=nil
                self.startChatVoice(appshot:context,initialPrompt:prompt)
            } else {
                self.dictationDestination=target
                self.sendPendingDictation()
            }
        }
        shortcutPreparation.onFailure={ [weak self] error in
            guard let self else{return}
            if self.compactVoiceMode {self.failCompactVoice(error)}
            else {self.cancelShortcutInput();self.miniUI.setCodexMessage(error)}
        }
        voicePrompt.capture=shortcutPreparation.capture
        voicePrompt.onLevels={ [weak self] levels in
            guard let self,self.compactVoiceMode,self.voicePrompt.phase == .recording else{return}
            self.indicator.setExternalLevels(levels)
        }
        voicePrompt.onChange={ [weak self] in self?.renderCompactVoice() }
        voicePrompt.onDiscard={ [weak self] in guard let self else{return};self.closeNotch(keepExpanded:self.miniOpen) }
        voicePrompt.onError={ [weak self] in self?.failCompactVoice($0) }
        voicePrompt.onReady={ [weak self] action,audio,context in
            guard let self,self.compactVoiceMode else {
                if let path=context?.imagePath{try? FileManager.default.removeItem(atPath:path)};return
            }
            self.pendingVoiceStart=(action,audio,context,self.voiceCaptureSource)
            let work=DispatchWorkItem{[weak self] in
                guard let self,self.pendingVoiceStart != nil else{return}
                self.failCompactVoice("Codex voice startup timed out; hold the shortcut to try again")
            }
            self.voiceLaunchTimeout=work;DispatchQueue.main.asyncAfter(deadline:.now()+60,execute:work)
            self.prepareRecordedVoice();self.renderCompactVoice()
        }
        chatVoice.onCommand={ [weak self] action,fields in self?.bridge.send(action,fields) }
        chatVoice.onInitialTextAccepted={ [weak self] accepted in
            guard let self,(self.composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines) == accepted else{return}
            self.setPrompt("")
        }
        chatVoice.onStatus={ [weak self] text in
            guard let self else{return}
            self.miniUI.setCodexMessage(text);self.featureStatusItem?.title=text
            if self.compactVoiceMode,self.chatVoice.mode == .off,text != "Voice off" {self.failCompactVoice(text)}
        }
        chatVoice.onLevels={ [weak self] levels in guard let self,self.chatVoice.listening else{return};self.indicator.setExternalLevels(levels) }
        chatVoice.onChange={ [weak self] in
            guard let self else{return}
            self.miniUI.setVoiceActive(self.chatVoice.active || self.voicePrompt.active || self.pendingVoiceStart != nil)
            self.chatVoiceMenuItem?.title=self.chatVoice.active ? "end voice conversation":"start voice in selected chat"
            if self.compactVoiceMode{self.renderCompactVoice()}
            else if self.chatVoice.listening{self.indicator.startListening(useMicrophone:false)}
            else if let state=self.latestState{self.showActivity(self.miniOpen ? state.viewedActivity:state.activity)}
        }
        chatVoice.attachMedia(to:panel.contentView)
        bridge.onVoiceSDP={ [weak self] in self?.chatVoice.receiveSDP($0,$1) }
        miniUI.onApprove={ [weak self] in self?.answerApproval("approve") }
        miniUI.onReject={ [weak self] in self?.answerApproval("reject") }
        miniUI.onInterrupt={ [weak self] in guard let self else{return};self.chatVoice.stopSpeaking();self.speech.cancelAudio();self.bridge.send("interrupt",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onClose={ [weak self] in self?.dismissNotchView() }
        miniUI.onFilesVisibleChanged={ [weak self] visible in self?.indicator.setFileOpen(visible) }
        miniUI.onImagesChanged={ [weak self] images in self?.queueImages(images) }
        bridge.onState={ [weak self] state in self?.receive(state) }
        bridge.onError={ [weak self] error in guard let self else{return};if self.compactVoiceMode{self.failCompactVoice(error);return};self.chatVoice.stop();self.sounds.playScene("failure");self.earlierHistoryRequest=nil;self.historyControl.update(hasMore:false,loading:false);self.cancelShortcutInput();self.pendingAutoSend=false;self.sendingPrompt=nil;self.pendingFilesRequestID=nil;self.indicator.setFileLoading(false);self.finishExpandedChatLoading();if self.miniOpen{self.pinned=false;self.compactWhileWorking=false;self.setOpen(false)};self.miniUI.cancelPending();self.miniUI.setCodexMessage(error);self.syncMotion(force:true) }
        speech.onModels={ [weak self] models in self?.updateVoiceMenu(models) }
        speech.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onChange={ [weak self] state in self?.receiveMedia(state) }
        media.onLevels={ [weak self] values in guard let self,self.mediaVisible else{return};self.indicator.setExternalLevels(values) }
        speech.discover();media.configure(youtube:youtubeMedia,spotify:spotifyMedia)
        bridge.start()
        localMonitor = NSEvent.addLocalMonitorForEvents(matching:[.flagsChanged,.keyDown]) { [weak self] e in
            if e.type == .flagsChanged {self?.flags(e)}
            else {
                self?.cancelShortcutHold();self?.lastCommandTap=0;self?.lastControlTap=0
                if e.keyCode == 53,self?.open == true {self?.dismissNotchView();return nil}
                if self?.voicePrompt.phase == .recording {self?.endCompactVoice()}
            }
            return e
        }
        globalMonitor = NSEvent.addGlobalMonitorForEvents(matching:[.flagsChanged,.keyDown]) { [weak self] e in
            if e.type == .flagsChanged {self?.flags(e)}
            else {self?.cancelShortcutHold();self?.lastCommandTap=0;self?.lastControlTap=0;if self?.voicePrompt.phase == .recording {self?.endCompactVoice()}}
        }
        dragMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDragged,.leftMouseUp]){[weak self] event in DispatchQueue.main.async{self?.globalDrag(event)}}
        NotificationCenter.default.addObserver(self, selector: #selector(placePanel),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(diagnoseMedia),name:NSNotification.Name("com.kai.media.diagnose"),object:nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(codexPreview(_:)),name:NSNotification.Name("com.kai.codex.preview"),object:nil)
    }

    func applicationWillTerminate(_ notification:Notification){imageEncodingWork?.cancel();voicePrompt.cancel();voiceLaunchTimeout?.cancel();shortcutPreparation.cancel();cancelShortcutHold();mediaReleaseWork?.cancel();chatVoice.stop();dictation.cancel();speech.stop();media.stop();artworkTask?.cancel();bridge.stop();attachmentFiles.forEach{try? FileManager.default.removeItem(at:$0)}}
    @objc private func codexPreview(_ note:Notification){guard let path=note.userInfo?["path"] as? String else{return};pinned=true;setOpen(true,mini:true,userInitiated:true);miniUI.previewFile(URL(fileURLWithPath:(path as NSString).expandingTildeInPath))}

    private func buildMenu() {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        status.button?.title = "海"; status.button?.toolTip = "kai"
        let menu = NSMenu()
        let defaultModelItem=menu.addItem(withTitle:"Default model",action:nil,keyEquivalent:"")
        defaultModelItem.submenu=defaultModelMenu.menu
        showItem = menu.addItem(withTitle: "show notch", action: #selector(toggleFromMenu), keyEquivalent: "")
        showItem.target = self
        chatVoiceMenuItem=menu.addItem(withTitle:"start voice in selected chat",action:#selector(toggleChatVoice),keyEquivalent:"")
        let realtimeVoices=menu.addItem(withTitle:"Realtime voice",action:nil,keyEquivalent:"")
        realtimeVoices.submenu=realtimeVoiceMenu.menu
        realtimeVoiceMenu.onSelection={ [weak self] choice in self?.featureStatusItem?.title="Realtime voice: \(choice.title) · next conversation" }
        voiceApprovalMenuItem=menu.addItem(withTitle:"review voice approval",action:#selector(reviewCompactVoiceApproval),keyEquivalent:"")
        voiceApprovalMenuItem?.isHidden=true
        chatVoiceMenuItem?.target=self
        experimentalModeItem=menu.addItem(withTitle:"experimental web mode",action:#selector(toggleExperimentalWebMode),keyEquivalent:"")
        experimentalModeItem.target=self
        mathModeItem=menu.addItem(withTitle:"render equations",action:#selector(toggleMathRendering),keyEquivalent:"")
        mathModeItem.target=self
        soundsItem=menu.addItem(withTitle:"sounds",action:#selector(toggleSounds),keyEquivalent:"")
        soundsItem.target=self
        codeConnectionItem=menu.addItem(withTitle:"Connect to Code",action:#selector(toggleCodeConnection),keyEquivalent:"")
        glassItem=menu.addItem(withTitle:"Frosted glass at bottom",action:#selector(toggleBottomGlass),keyEquivalent:"")
        let glassControls=menu.addItem(withTitle:"Glass appearance",action:nil,keyEquivalent:"")
        let glassMenu=NSMenu();glassControls.submenu=glassMenu
        let preferences=GlassPreferences(defaults:.standard)
        for control in GlassPreferences.controls {
            let view=NSView(frame:CGRect(x:0,y:0,width:240,height:42)),label=NSTextField(labelWithString:control.title)
            label.frame=CGRect(x:12,y:23,width:214,height:15);view.addSubview(label);glassLabels[control.key]=label
            let raw=UserDefaults.standard.object(forKey:"glass."+control.key) as? Double ?? control.initial
            let slider=NSSlider(value:raw.isFinite ? min(control.range.upperBound,max(control.range.lowerBound,raw)):control.initial,minValue:control.range.lowerBound,maxValue:control.range.upperBound,target:self,action:#selector(changeGlass(_:)))
            slider.identifier=NSUserInterfaceItemIdentifier(control.key);slider.frame=CGRect(x:12,y:2,width:216,height:20);slider.isContinuous=true
            view.addSubview(slider);glassSliders[control.key]=slider
            let item=NSMenuItem();item.view=view;glassMenu.addItem(item)
        }
        for (title,key,values) in [("Blur style","blur",["Light glass","Balanced frost","Deep frost"]),("Fade shape","blend",GlassPreferences.Blend.allCases.map(\.title))] {
            let root=glassMenu.addItem(withTitle:title,action:nil,keyEquivalent:"");root.submenu=NSMenu()
            for (index,value) in values.enumerated(){let item=root.submenu!.addItem(withTitle:value,action:#selector(chooseGlass(_:)),keyEquivalent:"");item.target=self;item.representedObject=key;item.tag=index;item.state=(key == "blur" ? preferences.blur:preferences.blend.rawValue) == index ? .on:.off;glassChoiceItems.append(item)}
        }
        let reset=glassMenu.addItem(withTitle:"Reset glass appearance",action:#selector(resetGlass),keyEquivalent:"");reset.target=self
        refreshGlassLabels()
        let sizingItem=menu.addItem(withTitle:"Default notch size",action:nil,keyEquivalent:"")
        let sizingMenu=NSMenu();sizingItem.submenu=sizingMenu
        for (title,key,minimum,maximum) in [("Width","notchDefaultWidth",370.0,1200.0),("Height","notchDefaultHeight",320.0,900.0)] {
            let view=NSView(frame:CGRect(x:0,y:0,width:240,height:42)),label=NSTextField(labelWithString:title)
            label.frame=CGRect(x:12,y:23,width:210,height:14);view.addSubview(label);sizeLabels[key]=label
            let slider=NSSlider(value:UserDefaults.standard.object(forKey:key) as? Double ?? minimum,minValue:minimum,maxValue:maximum,target:self,action:#selector(changeDefaultNotchSize(_:)))
            slider.identifier=NSUserInterfaceItemIdentifier(key);slider.frame=CGRect(x:12,y:2,width:216,height:20);slider.isContinuous=true;view.addSubview(slider)
            if key == "notchDefaultWidth" {defaultWidthSlider=slider}else{defaultHeightSlider=slider}
            let item=NSMenuItem();item.view=view;sizingMenu.addItem(item)
        }
        for (title,action) in [("Use current size as default",#selector(saveDefaultNotchSize)),("Restore saved default",#selector(restoreDefaultNotchSize)),("Reset to original larger size",#selector(resetDefaultNotchSize))] {let item=sizingMenu.addItem(withTitle:title,action:action,keyEquivalent:"");item.target=self}
        let morphItem = menu.addItem(withTitle:"test size morph",action:#selector(testSizeMorph),keyEquivalent:"")
        morphItem.target = self
        let petRoot = NSMenuItem(title: "pet", action: nil, keyEquivalent: ""), petMenu = NSMenu()
        let randomPetItem=petMenu.addItem(withTitle:"randomize pet",action:nil,keyEquivalent:"")
        let randomRow=PetMenuRow(title:"randomize pet")
        randomRow.onSelect={ [weak self] in self?.randomizePet() }
        randomPetItem.view=randomRow
        petFilterItem=petMenu.addItem(withTitle:"Exclude Codex pets from automatic choices",action:#selector(toggleCodexPetFilter),keyEquivalent:"");petFilterItem?.target=self
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
        let rescanVoicesItem=menu.addItem(withTitle:"rescan local voices",action:#selector(rescanVoices),keyEquivalent:"")
        let stopSpeakingItem=menu.addItem(withTitle:"stop speaking",action:#selector(stopSpeaking),keyEquivalent:"")
        compactSendItem=menu.addItem(withTitle:"compact immediately on send",action:#selector(toggleCompactSend),keyEquivalent:"")
        youtubeItem=menu.addItem(withTitle:"YouTube when idle (Webby, Dia, Chrome)",action:#selector(toggleYouTube),keyEquivalent:"")
        spotifyItem=menu.addItem(withTitle:"Spotify when idle",action:#selector(toggleSpotify),keyEquivalent:"")
        let mediaSizeView=NSView(frame:NSRect(x:0,y:0,width:180,height:40))
        let mediaSizeLabel=NSTextField(labelWithString:"media thumbnail size"),mediaSlider=NSSlider(value:Double(mediaSize),minValue:16,maxValue:38,target:self,action:#selector(changeMediaSize(_:)))
        mediaSizeLabel.frame=NSRect(x:14,y:22,width:150,height:14);mediaSlider.frame=NSRect(x:12,y:2,width:155,height:20);mediaSlider.isContinuous=true;mediaSizeSlider=mediaSlider
        mediaSizeView.addSubview(mediaSizeLabel);mediaSizeView.addSubview(mediaSlider);let mediaSizeItem=NSMenuItem();mediaSizeItem.view=mediaSizeView;menu.addItem(mediaSizeItem)
        let mediaPermissionsItem=menu.addItem(withTitle:"allow media permissions…",action:#selector(mediaPermissions),keyEquivalent:"")
        let contextItem=menu.addItem(withTitle:"Prefer lightweight appshot context",action:#selector(toggleContextMode(_:)),keyEquivalent:"");contextItem.target=self;contextItem.state=contextLightweight ? .on:.off
        let shortcutItem=menu.addItem(withTitle:"Shortcut actions",action:nil,keyEquivalent:"")
        let timingItem=menu.addItem(withTitle:"Hotkey hold timing",action:nil,keyEquivalent:"")
        expandedDictationItem=menu.addItem(withTitle:"Expanded notch: Command dictation only",action:#selector(toggleExpandedDictation),keyEquivalent:"")
        expandedDictationItem?.toolTip="When off, all shortcut assignments work in the expanded notch. When on, Command only dictates into the current draft; Fn and Option input shortcuts are disabled there."
        expandedDictationItem?.state=shortcutPreferences.expandedDictationOnly ? .on:.off
        timingItem.submenu=shortcutTimingMenu.menu
        shortcutTimingMenu.onChange={ [weak self] in self?.cancelShortcutHold() }
        shortcutItem.submenu=shortcutMenu
        for trigger in DictationTrigger.allCases {
            let title=trigger == .function ? "Fn":trigger.title
            let parent=shortcutMenu.addItem(withTitle:"Hold \(title)",action:nil,keyEquivalent:"")
            let choices=NSMenu();parent.submenu=choices
            for action in ShortcutAction.allCases {
                let item=choices.addItem(withTitle:action.title,action:#selector(selectShortcutAction(_:)),keyEquivalent:"")
                item.target=self;item.representedObject=["trigger":trigger.rawValue,"action":action.rawValue]
                item.toolTip=action.isVoice ? "Hold to record your first prompt. Release to connect and talk in the compact notch. Press again to end.":"Hold to dictate. Release sends when Dictation auto-send is enabled for this key."
            }
        }
        refreshShortcutMenu()
        let autoSendItem=menu.addItem(withTitle:"Dictation auto-send",action:nil,keyEquivalent:"")
        autoSendItem.submenu=autoSendMenu
        for trigger in DictationTrigger.allCases {
            let item=autoSendMenu.addItem(withTitle:trigger.title,action:#selector(toggleDictationAutoSend(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=trigger.rawValue
        }
        autoSendMenu.addItem(.separator())
        for (title,value) in [("Enable all","all"),("Disable all","none")] {
            let item=autoSendMenu.addItem(withTitle:title,action:#selector(toggleDictationAutoSend(_:)),keyEquivalent:"");item.target=self;item.representedObject=value
        }
        refreshAutoSendMenu()
        featureStatusItem=menu.addItem(withTitle:"Local speech and media are optional",action:nil,keyEquivalent:"");featureStatusItem.isEnabled=false
        for item in menu.items where item.action != nil && item.target == nil {item.target=self}
        updateFeatureChecks()
        let quit = menu.addItem(withTitle: "quit kai pet", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        quit.target = NSApp
        func items(_ values:[NSMenuItem?])->[NSMenuItem]{values.compactMap{$0}}
        NotchMenuOrganizer.organize(menu,quick:items([showItem,chatVoiceMenuItem,voiceApprovalMenuItem]),sections:[
            .init(title:"Chats & Models",items:[defaultModelItem]),
            .init(title:"Shortcuts & Input",items:items([shortcutItem,timingItem,expandedDictationItem,autoSendItem,contextItem,compactSendItem])),
            .init(title:"Voice & Audio",items:items([realtimeVoices,speechItem,voices,rescanVoicesItem,stopSpeakingItem,soundsItem])),
            .init(title:"Appearance",items:items([petRoot,sizeItem,motionRoot,indicatorRoot,accentRoot,sizingItem,glassItem,glassControls,mathModeItem])),
            .init(title:"Media",items:items([youtubeItem,spotifyItem,mediaSizeItem,mediaPermissionsItem])),
            .init(title:"Advanced & Connections",items:items([codeConnectionItem,experimentalModeItem,morphItem]))
        ],footer:items([featureStatusItem,quit]))
        status.menu = menu; refreshChecks()
    }

    @objc private func placePanel() {
        guard let screen = NSScreen.screens.first(where: { $0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil }),
              let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea else { panel?.orderOut(nil); return }
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        notchHeight = screen.safeAreaInsets.top + 1
        let gapCenter = (left.maxX + right.minX) / 2
        let center = abs(gapCenter - screen.frame.midX) <= 1 ? screen.frame.midX : gapCenter
        notchWidth = 2 * max(center - left.maxX, right.minX - center)
        let baseline=CGSize(width:notchWidth+160,height:320)
        let saved=CGSize(width:UserDefaults.standard.object(forKey:"notchDefaultWidth") as? Double ?? Double(baseline.width),height:UserDefaults.standard.object(forKey:"notchDefaultHeight") as? Double ?? 320)
        expandedSize=NotchSizing.constrained(expandedSize ?? saved,baseline:baseline,screen:screen.frame.size)
        canvasWidth=ceil(max(notchWidth+260,expandedSize!.width+100));canvasHeight=expandedSize!.height
        let defaultSize=NotchSizing.constrained(saved,baseline:baseline,screen:screen.frame.size)
        defaultWidthSlider?.minValue=Double(baseline.width);defaultWidthSlider?.maxValue=Double(max(baseline.width,screen.frame.width-80));defaultWidthSlider?.doubleValue=Double(defaultSize.width)
        defaultHeightSlider?.maxValue=Double(max(320,screen.frame.height-80));defaultHeightSlider?.doubleValue=Double(defaultSize.height)
        sizeLabels["notchDefaultWidth"]?.stringValue="Width · \(Int(defaultSize.width)) pt"
        sizeLabels["notchDefaultHeight"]?.stringValue="Height · \(Int(defaultSize.height)) pt"
        let rect = NSRect(x: center - canvasWidth / 2, y: screen.frame.maxY - canvasHeight,
                          width: canvasWidth, height: canvasHeight)
        if panel == nil {
            panel = NotchPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.backgroundColor = .clear; panel.isOpaque = false; panel.hasShadow = false; panel.ignoresMouseEvents = true
            panel.level = .statusBar; panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
            let host=NotchView(frame:CGRect(origin:.zero,size:rect.size));host.click={ [weak self] point,count in self?.handleClick(point,count:count) };host.scroll={ [weak self] point,delta in self?.handleScroll(point,delta) };host.hover={ [weak self] point in self?.handleHover(point) };host.dragEntered={ [weak self] in guard self?.experimentalWebMode != true else{return};self?.handleImageEntered() };host.imageDropped={ [weak self] images in guard self?.experimentalWebMode != true else{return};self?.handleImageDrop(images) };panel.contentView=host;host.wantsLayer=true;panel.acceptsMouseMovedEvents=true
            host.addSubview(glassBackground)
            scalableUI.wantsLayer=true;host.addSubview(scalableUI)
            composerField=ComposerField();composerField.isBordered=false;composerField.drawsBackground=false;composerField.focusRingType = .none;composerField.font=NSFont.systemFont(ofSize:10);composerField.textColor=NSColor.white.withAlphaComponent(0.88);composerField.delegate=self;composerField.isHidden=true;composerField.imagesEntered={ [weak self] in self?.handleImageEntered() };composerField.imagesDropped={ [weak self] images in self?.handleImageDrop(images) };scalableUI.addSubview(composerField)
            miniUI.composerFrameChanged={ [weak self] frame,animated in self?.layoutComposer(frame,animated:animated) }
            let scroll=ConversationScrollView(),text=FileActionHoverTextView()
            historyControl.onLoad={ [weak self] in self?.loadEarlierHistory() }
            historyControl.frame=CGRect(x:0,y:0,width:scroll.contentSize.width,height:24)
            historyControl.autoresizingMask=[.width];historyControl.isHidden=true;text.addSubview(historyControl)
            scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.scrollerStyle = .overlay;scroll.borderType = .noBorder;scroll.isHidden=true
            text.linkTextAttributes=[.cursor:NSCursor.pointingHand];text.drawsBackground=false;text.isEditable=false;text.isSelectable=true;text.textColor = .white;text.font=NSFont.systemFont(ofSize:9);text.textContainerInset=NSSize(width:3,height:24);text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true;text.autoresizingMask=[.width];text.delegate=self
            let interactionLayout=InteractionLayoutManager();interactionLayout.cardHost=scroll.contentView;scroll.contentView.drawsBackground=false
            text.textContainer?.replaceLayoutManager(interactionLayout)
            scroll.documentView=text;scalableUI.addSubview(scroll);conversationScroll=scroll;conversationText=text
            fileViewer.isHidden=true;scalableUI.addSubview(fileViewer)
            miniUI.previewContentChanged={ [weak self] frame,text,url,visible in
                guard let self else{return}
                let root=self.miniUI.layer.frame
                self.fileViewer.frame=frame.offsetBy(dx:root.minX,dy:root.minY)
                self.fileViewer.isHidden = !visible || !self.miniOpen || text == nil
                if let text,let url {self.fileViewer.show(text,url:url)}
            }
            mathRenderer.onImageReady={ [weak self] latex in guard let self else{return};self.conversationRenderer.invalidateMath(latex);self.scheduleMathRefresh() }
            miniUI.conversationFrameChanged={ [weak self] frame in self?.layoutConversation(frame) }
            let root = panel.contentView!.layer!; root.backgroundColor = NSColor.black.cgColor; root.mask = mask; scalableUI.layer!.addSublayer(miniUI.layer);scalableUI.layer!.addSublayer(petGallery.layer);notchContent.zPosition=20;root.addSublayer(notchContent)
            host.addSubview(resizeHandles)
            resizeHandles.onBegin={ [weak self] mode in guard let self else{return};self.cancelSizeTest();self.resizeMode=mode;self.resizeInitial=self.expandedSize ?? .zero }
            resizeHandles.onDrag={ [weak self] delta in guard let self else{return};self.expandedSize=NotchSizing.dragged(self.resizeInitial,delta:delta,mode:self.resizeMode);self.placePanel() }
            resizeHandles.onEnd={ [weak self] in guard let self else{return};self.panel.invalidateCursorRects(for:self.resizeHandles) }
            sprite.contentsGravity = .resizeAspect; sprite.magnificationFilter = .nearest; notchContent.addSublayer(indicator.leftAccentLayer); notchContent.addSublayer(indicator.accentLayer); notchContent.addSublayer(sprite);mediaArtwork.cornerRadius=7;mediaArtwork.masksToBounds=true;mediaArtwork.contentsGravity = .resizeAspectFill;mediaArtwork.isHidden=true;notchContent.addSublayer(mediaArtwork); notchContent.addSublayer(indicator.layer); notchContent.addSublayer(indicator.fileLayer)
        }
        mask.removeAnimation(forKey:"width")
        panel.setFrame(rect, display: true); mask.frame = CGRect(origin: .zero, size: rect.size); notchContent.frame = mask.frame
        // The full canvas must never intercept the desktop while compact.
        panel.ignoresMouseEvents = !NotchInputRegion.mainAcceptsInput(expanded:miniOpen,web:experimentalWebMode)
        let dropRect=NotchInputRegion.compactFrame(screen:screen.frame,center:center,width:notchWidth+160,height:notchHeight)
        if dropPanel == nil { dropPanel=NotchPanel(contentRect:dropRect,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);dropPanel.backgroundColor = .clear;dropPanel.isOpaque=false;dropPanel.level = .popUpMenu;dropPanel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary,.stationary];let drop=NotchView(frame:NSRect(origin:.zero,size:dropRect.size));drop.autoresizingMask=[.width,.height];drop.click={ [weak self] _,_ in guard let self else{return};if self.compactVoiceMode{self.handleCompactVoiceClick();return};guard self.compactWhileWorking else{return};self.autoCompact=false;self.compactWhileWorking=false;self.compactThreadID=nil;self.pinned=true;self.setOpen(true,mini:true) };drop.dragEntered={ [weak self] in self?.handleImageEntered() };drop.imageDropped={ [weak self] images in self?.handleImageDrop(images) };dropPanel.contentView=drop }
        dropPanel.setFrame(dropRect,display:true)
        let compactInput=NotchInputRegion.compactAcceptsInput(open:open,expanded:miniOpen,web:experimentalWebMode,working:compactWhileWorking,voice:compactVoiceMode)
        dropPanel.ignoresMouseEvents = !compactInput
        compactInput ? dropPanel.orderFrontRegardless():dropPanel.orderOut(nil)
        updateGlassBackground()
        let expanded = expandedWidth, kaiLeft = (canvasWidth - expanded) / 2
        let scale = screen.backingScaleFactor, topBandY = canvasHeight-notchHeight
        mask.contentsScale=scale
        func px(_ value: CGFloat) -> CGFloat { round(value * scale) / scale }
        layoutPet();layoutMediaArtwork(animated:false);mediaSizeSlider?.maxValue=Double(max(16,notchHeight-3))
        let physicalRight = (canvasWidth + notchWidth) / 2, kaiRight = (canvasWidth + expanded) / 2
        let iconCenter=(physicalRight+kaiRight)/2
        indicator.layer.frame = CGRect(x:px(iconCenter-(miniOpen && !miniListening ? 34:14)),y:px(topBandY+(notchHeight-18)/2),width:28,height:18)
        // Assign bounds and position directly: frame is undefined under the reveal scale
        // and would enlarge the bounds, leaving the fixed-size file glyph off-center.
        indicator.fileLayer.bounds=CGRect(x:0,y:0,width:28,height:18)
        indicator.fileLayer.position=CGPoint(x:px(iconCenter+(miniListening ? -14:6))+14,y:indicator.layer.position.y)
        indicator.layout();setFileButtonVisible(miniOpen && !miniListening,duration:0)
        let accentY=miniOpen ? 0:topBandY
        let shoulder:CGFloat=7,glowWidth:CGFloat=115, glowHeight:CGFloat=82
        indicator.compactBandHeight=notchHeight
        indicator.leftAccentLayer.frame = CGRect(x:px(kaiLeft-shoulder),y:px(accentY),width:glowWidth,height:glowHeight)
        indicator.accentLayer.frame = CGRect(x:px(kaiRight+shoulder-glowWidth),y:px(accentY),width:glowWidth,height:glowHeight); indicator.layout(); indicator.setAccentExpanded(miniOpen,duration:0)
        let content=CGSize(width:px(expanded-16),height:px(topBandY-24))
        let sizing=NotchSizing.layout(content:content,baseline:CGSize(width:notchWidth+144,height:320-notchHeight-24))
        scalableUI.frame=CGRect(x:px(kaiLeft+8),y:16,width:content.width,height:content.height)
        scalableUI.bounds=CGRect(origin:.zero,size:sizing.logical)
        miniUI.setRenderingScale(scale*sizing.scale)
        miniUI.layout(in:scalableUI.bounds)
        resizeHandles.frame=CGRect(x:kaiLeft+8,y:0,width:expanded-16,height:14);resizeHandles.isHidden = !miniOpen || experimentalWebMode
        resizeHandles.needsDisplay=true;panel.invalidateCursorRects(for:resizeHandles)
        petGallery.layer.frame=miniUI.layer.frame;petGallery.layout(in:petGallery.layer.bounds,selected:petID);NotchSizing.updateTextResolution(petGallery.layer,scale:scale*sizing.scale)
        if experimentalWebMode{ensureBrowser()};browserView?.frame=CGRect(x:px(kaiLeft),y:0,width:px(expanded),height:canvasHeight)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        mask.path = path(width:(open || experimentalWebMode) ? expanded:notchWidth, height:(open || experimentalWebMode) ? ((miniOpen || experimentalWebMode) ? canvasHeight:notchHeight):notchHeight)
        notchContent.opacity = experimentalWebMode ? 0:1;miniUI.layer.isHidden=experimentalWebMode;scalableUI.isHidden=experimentalWebMode
        browserView?.isHidden = !experimentalWebMode
        CATransaction.commit()
        if open || experimentalWebMode { panel.orderFrontRegardless() }
    }

    private func path(width: CGFloat, height: CGFloat) -> CGPath {
        NotchSizing.path(canvas:CGSize(width:canvasWidth,height:canvasHeight),width:width,height:height,radius:glassEnabled && miniOpen ? CGFloat(GlassPreferences(defaults:.standard).radius):14)
    }

    private func flags(_ event: NSEvent) {
        cancelSizeTest()
        let down=event.modifierFlags.contains(.command),option=event.modifierFlags.contains(.option),control=event.modifierFlags.contains(.control),function=event.modifierFlags.contains(.function)
        let miniChord=option && control,wasMiniChord=optionDown && controlDown
        defer {commandDown=down;optionDown=option;controlDown=control;functionDown=function}
        for (trigger,pressed) in [(DictationTrigger.command,down),(.option,option),(.function,function)] where !pressed {
            voicePrompt.release(trigger)
            shortcutPreparation.release(trigger)
            if dictating,dictationTrigger == trigger {finishDictation()}
        }
        let now=ProcessInfo.processInfo.systemUptime
        if control && !controlDown {
            if open && now-lastControlTap < 0.36 {lastControlTap=0;cancelShortcutHold();holdWork?.cancel();dismissNotchView();return}
            lastControlTap=now
        }
        // Double-Command closes for every assignment, including voice. Consume
        // it before scheduling a hold so the second tap cannot start listening.
        if down && !commandDown {
            if open && now-lastCommandTap < 0.36 {lastCommandTap=0;cancelShortcutHold();holdWork?.cancel();dismissNotchView();return}
            lastCommandTap=now
        }
        let change=shortcutGesture.update(command:down,option:option,control:control,function:function,shift:event.modifierFlags.contains(.shift))
        if change != .unchanged {
            cancelShortcutHold()
            if case .begin(let trigger)=change {
                holdWork?.cancel();holdWork=nil
                let expanded=miniOpen
                guard let action=shortcutPreferences.action(for:trigger,expanded:expanded) else{return}
                let generation=shortcutGeneration
                // Voice used to start as soon as the modifier went down. The
                // configured hold time only delayed microphone arming, which
                // still opened the notch and animated on quick taps. Gate the
                // whole voice startup on the same threshold as other shortcuts.
                let work=DispatchWorkItem { [weak self] in
                    guard let self,self.shortcutGeneration == generation,self.miniOpen == expanded,self.shortcutPreferences.action(for:trigger,expanded:expanded) == action else{return}
                    let flags=NSEvent.modifierFlags
                    guard ShortcutPreferences.heldTrigger(command:flags.contains(.command),option:flags.contains(.option),control:flags.contains(.control),function:flags.contains(.function),shift:flags.contains(.shift)) == trigger else{return}
                    self.shortcutWork=nil
                    if action.isVoice { self.startVoicePromptShortcut(action,trigger:trigger) }
                    else { self.startShortcut(action,trigger:trigger) }
                }
                shortcutWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+shortcutPreferences.holdDuration(for:trigger),execute:work)
            } else {
                if voicePrompt.phase == .recording {endCompactVoice()}
                for trigger in DictationTrigger.allCases {shortcutPreparation.release(trigger)}
            }
        }
        if miniChord != wasMiniChord {
            holdWork?.cancel();holdWork=nil
            if miniChord {
                let work=DispatchWorkItem { [weak self] in
                    guard let self,self.optionDown,self.controlDown else{return}
                    self.expandToRunningChat()
                }
                holdWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+1.0,execute:work)
            }
        }
    }

    private func startVoicePromptShortcut(_ action:ShortcutAction,trigger:DictationTrigger){
        if compactVoiceMode && (chatVoice.active || voicePrompt.active || shortcutPreparation.busy || pendingVoiceStart != nil) {endCompactVoice();return}
        guard !dictating,!finalizingDictation,!shortcutPreparation.busy,sendingPrompt == nil,expandedChatRequest == nil else{miniUI.setCodexMessage("Finish the current input before starting voice");return}
        guard (composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,queuedImageCount == 0 else{miniUI.setCodexMessage("Send or clear your draft before starting voice");return}
        let target=ShortcutPreparation.target(selected:latestState?.threadId,running:latestState.flatMap{RunningChatSelection.target(in:$0)?.id},lastOpened:lastOpenedChatID)
        if !action.createsChat {
            guard let target,!target.hasPrefix("code:") else{miniUI.setCodexMessage("Select a Codex chat or assign voice in a new chat");return}
            if action.capturesContext,latestState?.activeTurnId != nil{miniUI.setCodexMessage("Wait for this turn to finish, or use voice in a new chat with appshot");return}
        }
        dictationDestination=nil;voiceCaptureSource=latestState?.threadId
        if experimentalWebMode{toggleExperimentalWebMode()}
        voiceExpanded=miniOpen;compactVoiceMode=true;compactVoiceError=nil;compactVoiceIndicator=nil
        autoCompact=false;compactWhileWorking=false;pinned=true
        speech.cancelAudio();dismissMedia(closeNotch:false)
        // The global shortcut monitor already waited out the configured hold
        // threshold before entering here. Starting a second timer inside the
        // recorder would require two holds and discard a prompt released at
        // the slider's displayed duration.
        voicePrompt.begin(action,trigger:trigger,lightweight:contextLightweight,minimumHold:0)
        setOpen(true,mini:false);renderCompactVoice()
    }
    private func prepareRecordedVoice(){
        guard compactVoiceMode,let pending=pendingVoiceStart,!shortcutPreparation.busy,let state=latestState else{return}
        if let source=pending.source,state.threadId != source{failCompactVoice("Chat changed before voice started; hold the shortcut to try again");return}
        let target=ShortcutPreparation.target(selected:state.threadId,running:RunningChatSelection.target(in:state)?.id,lastOpened:lastOpenedChatID)
        shortcutPreparation.start(action:pending.action,trigger:.command,selected:state.threadId,target:target,lightweight:contextLightweight,context:pending.context)
    }
    private func renderCompactVoice(){
        guard compactVoiceMode else{return}
        let scene=latestState?.approval == true ? "permission":latestState?.viewedActivity.scene ?? "off"
        let activity:CompactVoicePresentation.Activity = compactVoiceError != nil ? .failure:
            CompactVoicePresentation.activity(recording:voicePrompt.phase == .recording,
                connecting:voicePrompt.phase == .finishing || pendingVoiceStart != nil || shortcutPreparation.busy || !chatVoice.listening,
                backendScene:scene,voice:chatVoice.activity)
        let status=compactVoiceError ?? (voicePrompt.phase == .recording ? "Hold to speak; release to start Codex voice":
            activity == .thinking ? "Codex voice · thinking":activity == .speaking ? "Codex voice · speaking":
            activity == .attention ? "Codex voice · approval needed":"Codex voice · listening")
        featureStatusItem?.title=status;dropPanel?.contentView?.toolTip=status+" · click to open chat and transcript"
        chatVoiceMenuItem?.title="end voice conversation"
        voiceApprovalMenuItem?.isHidden=latestState?.approval != true
        guard compactVoiceIndicator != activity else{return}
        compactVoiceIndicator=activity;currentScene="voice:\(activity.rawValue)"
        switch activity {
        case .listening,.speaking:indicator.startListening(useMicrophone:false);setMotion("waiting")
        case .thinking:indicator.showThinking();setMotion("running")
        case .attention:indicator.showScene(scene);setMotion("waiting")
        case .failure:indicator.showScene("failure");setMotion("failed")
        }
    }
    private func failCompactVoice(_ message:String){
        // A failed call is over. Retain its message, not ownership of the
        // notch: otherwise every new chat keeps rendering the old voice X.
        closeNotch(keepExpanded:miniOpen)
        miniUI.setCodexMessage(message);featureStatusItem?.title=message
    }
    private func endCompactVoice(){closeNotch()}
    private func handleCompactVoiceClick(){
        expandVoiceChat()
    }
    private func expandVoiceChat(){
        pinned=true;autoCompact=false;compactWhileWorking=false
        setOpen(true,mini:true,userInitiated:true)
    }
    private func dismissNotchView(){
        if compactVoiceMode {setOpen(true,mini:false,userInitiated:true)}else{closeNotch()}
    }
    @objc private func reviewCompactVoiceApproval(){
        guard compactVoiceMode,let state=latestState,state.approval,let identity=state.approvalRequestId,let thread=state.approvalThreadId,let view=dropPanel?.contentView else{return}
        let menu=NSMenu()
        let title=menu.addItem(withTitle:"Codex approval · review request",action:nil,keyEquivalent:"");title.isEnabled=false
        let scroll=NSScrollView(frame:NSRect(x:0,y:0,width:380,height:170))
        scroll.hasVerticalScroller=true;scroll.borderType = .noBorder
        let text=NSTextView(frame:NSRect(x:0,y:0,width:360,height:170))
        text.isEditable=false;text.isSelectable=true;text.font = .monospacedSystemFont(ofSize:11,weight:.regular)
        text.string=state.approvalSummary ?? state.activity.text;text.textContainerInset=NSSize(width:10,height:8)
        text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true
        text.textContainer?.containerSize=NSSize(width:340,height:CGFloat.greatestFiniteMagnitude)
        text.layoutManager?.ensureLayout(for:text.textContainer!)
        let height=text.layoutManager?.usedRect(for:text.textContainer!).height ?? 170
        text.setFrameSize(NSSize(width:360,height:max(170,height+16)));scroll.documentView=text
        let details=NSMenuItem();details.view=scroll;menu.addItem(details);menu.addItem(.separator())
        for (label,accept) in [("Allow this request",true),("Reject this request",false)] {
            let item=menu.addItem(withTitle:label,action:#selector(decideCompactApproval(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=["id":identity,"thread":thread,"accept":accept ? "yes":"no"]
        }
        menu.addItem(.separator())
        let end=menu.addItem(withTitle:"End voice",action:#selector(toggleChatVoice),keyEquivalent:"");end.target=self
        menu.popUp(positioning:nil,at:NSPoint(x:max(0,view.bounds.width-32),y:0),in:view)
    }
    @objc private func decideCompactApproval(_ item:NSMenuItem){
        guard let choice=item.representedObject as? [String:String],let identity=choice["id"],let thread=choice["thread"],latestState?.approvalRequestId == identity,latestState?.approvalThreadId == thread else{featureStatusItem?.title="Approval changed; open the chat to review again";return}
        bridge.send(choice["accept"] == "yes" ? "approve":"reject",["threadId":thread,"approvalRequestId":identity,"requestId":UUID().uuidString])
    }

    private func startShortcut(_ action:ShortcutAction,trigger:DictationTrigger){
        if action.isVoice {startVoicePromptShortcut(action,trigger:trigger);return}
        if action.isVoice,chatVoice.active {chatVoice.stop();return}
        if action.isVoice,shortcutPreparation.busy {cancelShortcutInput();miniUI.setCodexMessage("Voice startup cancelled");return}
        guard let state=latestState else{miniUI.setCodexMessage("Codex is still connecting");return}
        guard !dictating,!finalizingDictation,!shortcutPreparation.busy,sendingPrompt == nil,expandedChatRequest == nil else{miniUI.setCodexMessage("Finish the current input before starting another shortcut");return}
        if action.isVoice || action.createsChat {
            guard (composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty,queuedImageCount == 0 else{miniUI.setCodexMessage("Send or clear your draft before using this shortcut");return}
        }
        let target=ShortcutPreparation.target(selected:state.threadId,running:RunningChatSelection.target(in:state)?.id,lastOpened:lastOpenedChatID)
        if action.isVoice,action.capturesContext,!action.createsChat,state.activeTurnId != nil {miniUI.setCodexMessage("Wait for this turn to finish before starting voice with appshot");return}
        if action.isVoice {dictationDestination=nil}
        if action.capturesContext {featureStatusItem?.title="Capturing appshot context…"}
        shortcutPreparation.start(action:action,trigger:trigger,selected:state.threadId,target:target,lightweight:contextLightweight)
    }

    private func cancelShortcutInput(){
        if compactVoiceMode {
            compactVoiceMode=false;voiceExpanded=false;compactVoiceError=nil;compactVoiceIndicator=nil
            voiceCaptureSource=nil;voiceApprovalMenuItem?.isHidden=true;currentScene=""
            chatVoice.stop()
        }
        voicePrompt.cancel();voiceLaunchTimeout?.cancel();voiceLaunchTimeout=nil
        if let path=pendingVoiceStart?.context?.imagePath {attachmentFiles.append(URL(fileURLWithPath:path))}
        pendingVoiceStart=nil
        shortcutPreparation.cancel();pendingAutoSend=false;dictationDestination=nil
        dictationGeneration=UUID();finalizingDictation=false
        if dictating {dictation.cancel();dictating=false;dictationTrigger=nil;sounds.stopListening();exitMiniListening()}
    }

    private func expandToRunningChat(){
        if compactVoiceMode {expandVoiceChat();return}
        let target=latestState.flatMap{RunningChatSelection.target(in:$0)}
        pinned=true;setOpen(true,mini:true)
        guard let target,target.id != latestState?.threadId,expandedChatRequest == nil else{return}
        let requestID=miniUI.beginChatLoading(target.name)
        expandedChatRequest=(requestID,target.id,target.name)
        composerField?.isEnabled=false
        bridge.send("chat",["id":target.id,"requestId":requestID])
    }
    private func finishExpandedChatLoading(){
        expandedChatRequest=nil;composerField?.isEnabled=true
    }

    private func setOpen(_ requested: Bool, mini requestedMini: Bool = false,userInitiated:Bool=false) {
        if compactVoiceMode,userInitiated {voiceExpanded=requested && requestedMini}
        let presentation=CompactVoicePresentation.layout(active:compactVoiceMode,open:requested,expanded:requestedMini,manualExpansion:voiceExpanded)
        let desired=presentation.open,mini=presentation.expanded
        if desired && mini {autoCompact=false}
        if mediaVisible && (mini || compactWhileWorking || dictating || experimentalWebMode){dismissMedia()}
        guard panel != nil, desired != open || (desired && mini != miniOpen) || presentedCompact != compactWhileWorking else { refreshChecks();return }
        if petGalleryVisible && (!desired || !mini){hidePetGallery(restore:false)}
        panelRevision += 1;let revision=panelRevision
        let wasOpen=open,wasMini=miniOpen; open = desired; miniOpen = desired && mini;presentedCompact=compactWhileWorking;if desired && !wasOpen{play()}else if !desired{playToken += 1;sprite.removeAnimation(forKey:"spriteFrames")}; panel.ignoresMouseEvents = !miniOpen;composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen;resizeHandles.isHidden = !miniOpen || experimentalWebMode;refreshChecks()
        media.setSuppressed(mediaSuppressed)
        let previousPath=mask.presentation()?.path ?? mask.path
        let oldIndicatorX=indicator.layer.presentation()?.position.x ?? indicator.layer.position.x
        let oldAccentY=[indicator.leftAccentLayer,indicator.accentLayer].map{$0.presentation()?.position.y ?? $0.position.y}
        let oldFileOpacity=indicator.fileLayer.presentation()?.opacity ?? indicator.fileLayer.opacity
        let oldFileTransform=indicator.fileLayer.presentation()?.transform ?? indicator.fileLayer.transform
        let traveling=[sprite,indicator.fileLayer,indicator.leftAccentLayer,indicator.accentLayer]
        let oldPositions=traveling.map{$0.presentation()?.position ?? $0.position}
        placePanel()
        CATransaction.begin();CATransaction.setDisableActions(true)
        indicator.layer.position.x=oldIndicatorX;indicator.fileLayer.opacity=oldFileOpacity;indicator.fileLayer.transform=oldFileTransform
        for (index,accent) in [indicator.leftAccentLayer,indicator.accentLayer].enumerated(){accent.position.y=oldAccentY[index]}
        CATransaction.commit()
        let destination = path(width:desired ? expandedWidth:notchWidth,height:desired ? (mini ? canvasHeight:notchHeight):notchHeight), animation = CABasicAnimation(keyPath:"path")
        animation.fromValue = previousPath; animation.toValue = destination
        animation.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:(desired ? (mini ? 0.28:0.24):0.22); animation.timingFunction = CAMediaTimingFunction(controlPoints: 0.16, 0.85, 0.25, 1)
        for (index,layer) in traveling.enumerated() {
            let move=CABasicAnimation(keyPath:"position.x");move.fromValue=oldPositions[index].x;move.toValue=layer.position.x;move.duration=animation.duration;move.timingFunction=animation.timingFunction;layer.add(move,forKey:"responsiveTravel")
        }
        animateTopAccessories(forMini:miniOpen,duration:animation.duration)
        animateCornerAccents(toBottom:miniOpen,duration:animation.duration)
        indicator.setAccentExpanded(miniOpen,duration:animation.duration)
        miniUI.setVisible(miniOpen,duration:animation.duration)
        syncConversationTheme()
        if miniOpen && !wasMini { if let state=latestState {renderConversation(state.history,forceBottom:true)};miniListening=false;miniUI.resetFiles(); indicator.setFileOpen(false); currentScene="";if let state=latestState{showActivity(miniOpen ? state.viewedActivity:state.activity)};setMotion("waving",replay:true) }
        else if wasMini && !miniOpen { mathRenderer.suspend();mathRefreshWork?.cancel();mathRefreshWork=nil;historyControl.update(hasMore:false,loading:false);miniListening=false;if compactWhileWorking,let state=latestState {currentScene="";showActivity(miniOpen ? state.viewedActivity:state.activity)}else{applyIndicator()};syncMotion(force:true) }
        panel.orderFrontRegardless(); CATransaction.begin(); CATransaction.setDisableActions(true); mask.path = destination; CATransaction.commit(); mask.add(animation, forKey: "width")
        if compactVoiceMode{compactVoiceIndicator=nil;renderCompactVoice()}
        if !desired {indicator.hide();currentScene="";mathRefreshWork?.cancel();mathRefreshWork=nil;DispatchQueue.main.asyncAfter(deadline: .now()+animation.duration) { [weak self] in
            guard let self,!self.open,self.panelRevision == revision else { return }; self.panel.orderOut(nil)
        } }
    }

    private func handleClick(_ point:CGPoint,count:Int) {
        if compactWhileWorking && !miniOpen {autoCompact=false;compactWhileWorking=false;pinned=true;setOpen(true,mini:true);return}
        guard miniOpen else{return}
        let topFrames=[sprite,indicator.fileLayer,indicator.layer].enumerated().map{index,item -> CGRect in
            let visible=item.presentation() ?? item
            guard !item.isHidden,visible.opacity>0.01,!(index == 1 && miniListening) else{return .zero}
            return visible.frame
        }
        let topTarget=NotchInputRegion.nearestTarget(at:point,frames:topFrames)
        if topTarget == 0 {
            if count >= 2 {petClickWork?.cancel();petClickWork=nil;petGalleryVisible ? hidePetGallery():showPetGallery()}
            else if !petGalleryVisible {petClickWork?.cancel();let work=DispatchWorkItem{[weak self] in self?.randomizePet()};petClickWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.22,execute:work)}
            return
        }
        if petGalleryVisible {
            let frame=petGallery.layer.frame,uiPoint=scalableUI.convert(point,from:panel.contentView),local=CGPoint(x:uiPoint.x-frame.minX,y:uiPoint.y-frame.minY)
            if frame.contains(uiPoint),let id=petGallery.pet(at:local){usePet(id);hidePetGallery()}
            return
        }
        if topTarget == 1 { miniUI.toggleFiles();if miniUI.isFilesOpen{let requestID=UUID().uuidString;pendingFilesRequestID=requestID;indicator.setFileLoading(true);bridge.send("files",["requestId":requestID])}else{pendingFilesRequestID=nil;indicator.setFileLoading(false)};return }
        if topTarget == 2 { dismissNotchView();return }
        let frame=miniUI.layer.frame,uiPoint=scalableUI.convert(point,from:panel.contentView), local=CGPoint(x:uiPoint.x-frame.minX,y:uiPoint.y-frame.minY)
        if frame.contains(uiPoint) { _=miniUI.handleClick(local,count:count) }
    }

    private func handleHover(_ point:CGPoint?) {
        guard miniOpen else{return}
        let local=point.map{scalableUI.convert($0,from:panel.contentView)}
        if petGalleryVisible {petGallery.hover(at:local.flatMap{petGallery.layer.frame.contains($0) ? $0:nil});miniUI.handleHover(nil)}
        else {miniUI.handleHover(local.flatMap{miniUI.layer.frame.contains($0) ? $0:nil})}
    }
    private func handleScroll(_ point:CGPoint,_ delta:CGFloat){
        guard miniOpen else{return};let local=scalableUI.convert(point,from:panel.contentView)
        if miniUI.layer.frame.contains(local){miniUI.scroll(delta,at:local)}
    }
    private func showFileActions(_ url:URL){
        fileActions.menu(for:url).popUp(positioning:nil,at:NSEvent.mouseLocation,in:nil)
    }
    private func animateFileAction(_ action:String,url:URL,stamp:TimeInterval){
        guard FileActionSymbols.confirmation(action,url:url) == stamp else{return}
        let finished=ProcessInfo.processInfo.systemUptime-stamp>=1.3
        if finished{FileActionSymbols.clear(action,url:url,stamp:stamp)}
        miniUI.refreshFileAction(action,url:url)
        if let view=conversationText{conversationRenderer.refreshFileAction(action,url:url,in:view)}
        if !finished {
            DispatchQueue.main.asyncAfter(deadline:.now()+1.0/30){[weak self] in self?.animateFileAction(action,url:url,stamp:stamp)}
        }
    }

    private func showPetGallery(){guard miniOpen else{return};petGalleryVisible=true;miniUI.setVisible(false,duration:0.3);composerField?.isHidden=true;conversationScroll?.isHidden=true;petGallery.layout(in:petGallery.layer.bounds,selected:petID);petGallery.show(selected:petID,motion:motionID)}
    private func hidePetGallery(restore:Bool=true){guard petGalleryVisible else{return};petGalleryVisible=false;petGallery.hide();if restore{miniUI.setVisible(miniOpen,duration:0.32);composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen}}

    private func layoutComposer(_ frame:CGRect,animated:Bool){guard let field=composerField else{return};let root=miniUI.layer.frame,h:CGFloat=18,target=CGRect(x:root.minX+frame.minX+10,y:root.minY+frame.midY-h/2-2.25,width:max(20,frame.width-73),height:h);if animated{NSAnimationContext.runAnimationGroup{context in context.duration=0.42;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);field.animator().frame=target}}else{field.frame=target}}
    private func layoutConversation(_ frame:CGRect){guard let scroll=conversationScroll else{return};let root=miniUI.layer.frame;scroll.frame=CGRect(x:root.minX+frame.minX,y:root.minY+frame.minY,width:frame.width,height:frame.height)}
    private func setPrompt(_ text:String,dictation:Bool=false){if dictation,let field=composerField{DictationComposerEditing.update(field,text:text)}else{composerField?.stringValue=text};miniUI.setComposerHasText(!text.isEmpty);syncMotion(force:!text.isEmpty)}
    func controlTextDidChange(_ obj:Notification){miniUI.setComposerHasText(!(composerField?.stringValue.isEmpty ?? true));syncMotion(force:true)}
    func control(_ control:NSControl,textView:NSTextView,doCommandBy selector:Selector)->Bool{if selector == #selector(NSResponder.insertNewline(_:)){sendPrompt();return true};return false}

    private func sendPrompt(){if attachmentEncoding{pendingAttachmentSend=true;miniUI.setCodexMessage("Preparing images…");return};guard expandedChatRequest == nil,sendingPrompt == nil, !shortcutPreparation.busy else{return};if latestState?.threadId?.hasPrefix("code:") == true,latestState?.readOnly == true{miniUI.setCodexMessage("Code is busy or loading — your prompt is kept here.");return};let prompt=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines);guard !prompt.isEmpty || queuedImageCount>0 else{return};sendingPrompt=prompt;setMotion("running-left",replay:true);speech.cancelAudio();let requestID=UUID().uuidString;sendingPromptRequestID=requestID;bridge.send("send",["text":prompt,"requestId":requestID]);if compactOnSend{hidePetGallery(restore:false);autoCompact=false;compactWhileWorking=true;compactThreadID=latestState?.threadId;compactStartHistoryCount=latestState?.history.count ?? 0;pinned=true;setOpen(true,mini:false)}}
    private func answerApproval(_ action:String){if action == "interrupt"{speech.cancelAudio()};var fields:[String:Any]=["requestId":miniUI.pendingRequestID ?? ""];if let id=latestState?.approvalThreadId{fields["threadId"]=id};bridge.send(action,fields)}

    private func queueImages(_ images:[NSImage]){
        imageEncodingWork?.cancel();imageEncodingWork=nil
        let generation=UUID();attachmentGeneration=generation
        queuedImageCount=images.count;attachmentEncoding = !images.isEmpty
        guard !images.isEmpty else{
            bridge.send("attachments",["paths":[String]()])
            if pendingAttachmentSend{pendingAttachmentSend=false;sendPrompt()}
            return
        }
        let snapshots=images.map(AttachmentImageEncoding.Input.init)
        let operation=BlockOperation();imageEncodingWork=operation
        operation.addExecutionBlock { [weak self,weak operation] in
            var files=[URL](),failure:String?
            for image in snapshots {
                guard operation?.isCancelled == false else{break}
                do {
                    let file=try autoreleasepool { ()throws->URL in
                        guard let data=image.png() else{throw CocoaError(.fileReadCorruptFile)}
                        let url=FileManager.default.temporaryDirectory.appendingPathComponent("kai-\(UUID().uuidString).png")
                        try data.write(to:url,options:.atomic);return url
                    }
                    files.append(file)
                }catch{failure="Could not attach image: \(error.localizedDescription)"}
            }
            guard operation?.isCancelled == false else{files.forEach{try? FileManager.default.removeItem(at:$0)};return}
            let prepared=files,error=failure
            DispatchQueue.main.async { [weak self] in
                guard let self,self.attachmentGeneration == generation else{
                    DispatchQueue.global(qos:.utility).async{prepared.forEach{try? FileManager.default.removeItem(at:$0)}}
                    return
                }
                self.imageEncodingWork=nil;self.attachmentEncoding=false
                self.attachmentFiles.append(contentsOf:prepared);self.queuedImageCount=prepared.count
                self.bridge.send("attachments",["paths":prepared.map(\.path)])
                if let error{self.pendingAttachmentSend=false;self.miniUI.setCodexMessage(error)}
                else if self.pendingAttachmentSend{self.pendingAttachmentSend=false;self.sendPrompt()}
            }
        }
        imageEncodingQueue.addOperation(operation)
    }

    private func receive(_ state:KaiState){
        defer{refreshMedia()}
        if let connection=state.codeConnection{codeConnectionItem?.toolTip=connection.error;codeConnectionItem?.title=connection.enabled ? "Connect to Code · \(connection.connected ? "connected":"waiting")":"Connect to Code"}
        let previous=latestState,previousNotice=previous?.notice;latestState=state
        if let models=state.models{defaultModelMenu.update(models)}
        let prepending=previous?.threadId == state.threadId && earlierHistoryRequest != nil && state.completedRequestId == earlierHistoryRequest
        if prepending || previous?.threadId != state.threadId{earlierHistoryRequest=nil}
        if let thread=state.threadId,thread != lastOpenedChatID {lastOpenedChatID=thread;UserDefaults.standard.set(thread,forKey:"lastOpenedChatID")}
        if let target=dictationDestination,state.threadId != target {cancelShortcutInput()}
        shortcutPreparation.receive(thread:state.threadId,request:state.completedRequestId,error:state.completedRequestError)
        if mediaVisible && MediaCompanion.codexNeedsNotch(state) {
            dismissMedia(closeNotch:false)
            if !miniOpen {autoCompact=true;compactWhileWorking=true;compactThreadID=nil;setOpen(true,mini:false)}
        }
        if !state.standalone,previous?.project != state.project{UserDefaults.standard.set(state.project,forKey:"kaiProject")};miniUI.setState(state);if chatVoice.active{speech.synchronize(state)}else{speech.receive(state)};syncTaskPet(state);if miniOpen {renderConversation(state.history,forceBottom:previous?.threadId != state.threadId,preserveViewport:prepending)}
        let completed=previous?.threadId == state.threadId && state.threadId != nil && (previous?.activeTurnId != nil || previous?.viewedActivity.scene == "thinking") && state.activeTurnId == nil && state.viewedActivity.scene == "off"
        if sendingPrompt != nil {
            if (state.completedRequestId == sendingPromptRequestID && state.completedRequestError == nil) || (state.threadId?.hasPrefix("code:") == true ? state.codeAcceptedRequestId != nil && state.codeAcceptedRequestId == sendingPromptRequestID : state.notice == "Turn started" || state.notice == "Sent to voice" || state.notice.hasPrefix("Steered active turn")) {setPrompt("");miniUI.clearImages();sendingPrompt=nil;sendingPromptRequestID=nil}
            else if state.notice != previousNotice {sendingPrompt=nil;sendingPromptRequestID=nil;if compactOnSend && compactWhileWorking{compactWhileWorking=false;compactThreadID=nil;setOpen(true,mini:true)}}
        }
        if state.threadId?.hasPrefix("code:") == true,state.notice.hasPrefix("Prompt staged in Code"),state.notice != previousNotice{compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)}
        if !dictating && !miniListening && !chatVoice.listening {showActivity(miniOpen ? state.viewedActivity:state.activity)}
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
                if let output,!compactVoiceMode{miniUI.previewFile(output)}
            }
        }
        if compactWhileWorking && !autoCompact && state.activeTasks.isEmpty && previous?.activeTasks.isEmpty == false && !miniOpen {
            compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true);setMotion("jumping",replay:true)
        }
        if let request=expandedChatRequest {
            if state.completedRequestId == request.id {
                finishExpandedChatLoading()
                if state.threadId != request.target {miniUI.setCodexMessage(state.notice)}
            } else {miniUI.setCodexMessage("Loading \(request.name.isEmpty ? "chat":request.name)…")}
        }
        let wasVoiceActive=chatVoice.active
        chatVoice.receive(state)
        if compactVoiceMode,wasVoiceActive,!chatVoice.active {closeNotch(keepExpanded:miniOpen)}
        if voicePrompt.active,let source=voiceCaptureSource,state.threadId != source {failCompactVoice("Chat changed while recording; hold the shortcut to try again")}
        prepareRecordedVoice();renderCompactVoice()
        if let error=state.voice?.historyError{miniUI.setCodexMessage(error)}
        miniUI.finishPending(requestID:state.completedRequestId)
        if let pendingFilesRequestID,state.completedRequestId == pendingFilesRequestID {self.pendingFilesRequestID=nil;indicator.setFileLoading(false)}
        if !state.activeTasks.isEmpty && !open {autoCompact=true;compactWhileWorking=true;setOpen(true,mini:false)}
        if state.activeTasks.isEmpty && autoCompact && !miniOpen {autoCompact=false;DispatchQueue.main.asyncAfter(deadline:.now()+1.2){[weak self] in guard let self,self.latestState?.activeTasks.isEmpty == true,!self.miniOpen,!self.dictating,!self.finalizingDictation else{return};self.compactWhileWorking=false;self.setOpen(false)}}
    }
    private func updateFeatureChecks(){
        speechItem?.state=speakResponses ? .on:.off;compactSendItem?.state=compactOnSend ? .on:.off
        youtubeItem?.state=youtubeMedia ? .on:.off;spotifyItem?.state=spotifyMedia ? .on:.off
    }
    private func refreshAutoSendMenu(){
        for item in autoSendMenu.items {
            if let value=item.representedObject as? String,let trigger=DictationTrigger(rawValue:value){item.state=autoSendPreferences.enabled(for:trigger) ? .on:.off}
        }
    }
    private func cancelShortcutHold(){
        shortcutWork?.cancel();shortcutWork=nil;shortcutGeneration=UUID()
    }
    private func refreshShortcutMenu(){
        shortcutTimingMenu.refresh()
        for parent in shortcutMenu.items {
            for item in parent.submenu?.items ?? [] {
                guard let choice=item.representedObject as? [String:String],let key=choice["trigger"],let trigger=DictationTrigger(rawValue:key),let action=choice["action"] else{continue}
                item.state=shortcutPreferences.action(for:trigger).rawValue == action ? .on:.off
            }
        }
    }
    @objc private func selectShortcutAction(_ item:NSMenuItem){
        guard let choice=item.representedObject as? [String:String],let key=choice["trigger"],let trigger=DictationTrigger(rawValue:key),let value=choice["action"],let action=ShortcutAction(rawValue:value) else{return}
        cancelShortcutHold();holdWork?.cancel();holdWork=nil
        if voicePrompt.active || pendingVoiceStart != nil {endCompactVoice()}
        else if shortcutPreparation.busy {cancelShortcutInput()}
        shortcutPreferences.set(action,for:trigger);refreshShortcutMenu()
    }
    @objc private func toggleDictationAutoSend(_ item:NSMenuItem){
        guard let value=item.representedObject as? String else{return}
        if let trigger=DictationTrigger(rawValue:value){autoSendPreferences.set(!autoSendPreferences.enabled(for:trigger),for:trigger)}
        else {for trigger in DictationTrigger.allCases {autoSendPreferences.set(value == "all",for:trigger)}}
        refreshAutoSendMenu()
    }
    @objc private func toggleExpandedDictation(){
        cancelShortcutHold()
        shortcutPreferences.expandedDictationOnly.toggle()
        expandedDictationItem?.state=shortcutPreferences.expandedDictationOnly ? .on:.off
    }
    private func sendPendingDictation(){
        guard pendingAutoSend,!shortcutPreparation.busy,!dictating else{return}
        guard let target=dictationDestination,target == latestState?.threadId else{pendingAutoSend=false;return}
        pendingAutoSend=false
        sendPrompt()
    }
    @objc private func toggleContextMode(_ item:NSMenuItem){contextLightweight.toggle();miniUI.setContextMode(contextLightweight);item.state=contextLightweight ? .on:.off}
    @objc private func toggleChatVoice(){
        cancelShortcutHold()
        if compactVoiceMode || chatVoice.active{endCompactVoice();return}
        if shortcutPreparation.busy {cancelShortcutInput();miniUI.setCodexMessage("Voice startup cancelled");return}
        startChatVoice()
    }
    private func startChatVoice(appshot:ApplicationContext?=nil,initialPrompt:Data?=nil){
        guard let state=latestState else{miniUI.setCodexMessage("Codex is still connecting");return}
        guard !dictating,!finalizingDictation,!shortcutPreparation.busy,sendingPrompt == nil,expandedChatRequest == nil else{miniUI.setCodexMessage("Finish the current input before starting voice");return}
        let typed=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines)
        guard queuedImageCount == 0,initialPrompt == nil || typed.isEmpty else{failCompactVoice("Send or clear the attached draft before starting voice");return}
        if experimentalWebMode{toggleExperimentalWebMode()}
        if !compactVoiceMode{voiceExpanded=miniOpen}
        speech.cancelAudio();dismissMedia(closeNotch:false);compactVoiceMode=true;compactVoiceError=nil;compactVoiceIndicator=nil
        autoCompact=false;compactWhileWorking=false;pinned=true;setOpen(true,mini:false)
        chatVoice.start(state,appshot:appshot,initialPrompt:initialPrompt,initialText:typed.isEmpty ? nil:typed,voice:realtimeVoiceMenu.selection)
        renderCompactVoice()
    }

    @objc private func toggleSpeech(){speakResponses.toggle();UserDefaults.standard.set(speakResponses,forKey:"speakResponses");speech.configure(enabled:speakResponses,model:speech.selectedModel);updateFeatureChecks()}
    @objc private func toggleCompactSend(){compactOnSend.toggle();UserDefaults.standard.set(compactOnSend,forKey:"compactOnSend");updateFeatureChecks()}
    @objc private func stopSpeaking(){speech.cancelAudio();chatVoice.stopSpeaking()}
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
        guard let url=URL(string:state.artwork),url.scheme == "https" else{mediaImage=nil;mediaProfile=nil;loadedMediaKey=nil;dismissMedia();return}
        artworkTask=URLSession.shared.dataTask(with:URLRequest(url:url,timeoutInterval:12)){[weak self] data,response,_ in
            let image:CGImage?=autoreleasepool {
                guard let data,let response=response as? HTTPURLResponse,(200..<300).contains(response.statusCode) else{return nil}
                return MediaNowPlaying.decodeArtwork(data)
            }
            let profile=image.flatMap{MediaNowPlaying.profile($0)}
            DispatchQueue.main.async{ [weak self] in guard let self,self.mediaState == state else{return};self.artworkTask=nil;guard let image else{self.mediaImage=nil;self.mediaProfile=nil;self.loadedMediaKey=nil;self.dismissMedia();return};self.mediaImage=NSImage(cgImage:image,size:CGSize(width:image.width,height:image.height));self.mediaProfile=profile;self.loadedMediaKey=state.application+":"+state.identity+":"+state.artwork;self.refreshMedia()}
        };artworkTask?.resume();refreshMedia()
    }
    private var mediaSuppressed:Bool {compactVoiceMode || voicePrompt.active || chatVoice.active || miniOpen || dictating || finalizingDictation || pendingAutoSend || miniListening || compactWhileWorking || experimentalWebMode || sendingPrompt != nil || latestState?.activeTurnId != nil || latestState?.approval == true || !(latestState?.activeTasks.isEmpty ?? true)}
    private func refreshMedia(){
        let busy=mediaSuppressed
        media.setSuppressed(busy)
        guard !busy,mediaState != nil,let image=mediaImage else{if mediaVisible{dismissMedia()};return}
        let entering = !mediaVisible
        if entering {mediaVisible=true;setOpen(true,mini:false);playToken += 1;sprite.removeAnimation(forKey:"spriteFrames");visualTransition.reveal(sprite,visible:false);layoutMediaArtwork(animated:false);indicator.setExternalLevels([0,0,0,0]);indicator.startListening(useMicrophone:false)}
        media.capture(true)
        if entering || presentedMediaKey != loadedMediaKey {
            presentedMediaKey=loadedMediaKey
            if let cgImage=image.cgImage(forProposedRect:nil,context:nil,hints:nil){visualTransition.artwork(cgImage,on:mediaArtwork)}
            if let profile=mediaProfile{indicator.setProfile(profile,wave:true)}
        }
    }
    private func dismissMedia(closeNotch:Bool=true){
        guard mediaVisible else{return};mediaVisible=false;currentScene="";media.capture(false);presentedMediaKey=nil;visualTransition.hideArtwork(mediaArtwork);sprite.isHidden=false;visualTransition.reveal(sprite,visible:true);play();if let state=latestState{showActivity(miniOpen ? state.viewedActivity:state.activity)};if closeNotch && !miniOpen && !compactWhileWorking && !dictating && !finalizingDictation{setOpen(false)}
    }
    private func syncConversationTheme(){
        let id=ConversationTheme.petID(expanded:miniOpen || compactVoiceMode,viewedThread:latestState?.threadId,taskPets:taskPets,indicatorPet:petID,fallback:userPetID)
        guard conversationPetID != id,let profile=gradientCatalog.profileByPetID[id] else{return}
        conversationPetID=id;indicator.setProfile(profile,wave:true);miniUI.setProfile(profile,animated:true)
        assistantPalette=assistantGradientColors(for:id)
        if let storage=conversationText?.textStorage{ConversationTheme.recolor(storage,palette:assistantPalette)}
        updateGlassBackground()
    }
    private func syncTaskPet(_ state:KaiState){
        let ids=Set(state.activeTasks.map(\.id)),pets=petSelection.automaticIDs(catalog.pets.map(\.id))
        var occupied=Set(activeTaskIDs.intersection(ids).compactMap{taskPets[$0]})
        for task in state.activeTasks.reversed(){
            if taskPets[task.id] == nil {
                if occupied.isEmpty {
                    taskPets[task.id]=pets.contains(petID) ? petID : (pets.first ?? userPetID)
                } else {
                    let available=pets.filter{!occupied.contains($0)}
                    taskPets[task.id]=(available.isEmpty ? pets:available).randomElement() ?? userPetID
                }
            }
            if let assigned=taskPets[task.id]{occupied.insert(assigned)}
        }
        if let viewed=state.threadId,taskPets[viewed] == nil {taskPets[viewed]=pets.contains(userPetID) ? userPetID : (pets.first ?? userPetID)}
        activeTaskIDs=ids
        if compactVoiceMode,let viewed=state.threadId,let assigned=taskPets[viewed] {
            if petID != assigned {petID=assigned;currentScene="";play()}
        } else if let focus=state.focusThreadId {
            let assigned=taskPets[focus] ?? petID
            if focusedTaskID != focus || petID != assigned {focusedTaskID=focus;petID=assigned;currentScene="";play()}
        } else {
            focusedTaskID=nil
        }
        syncConversationTheme()
    }
    private func closeNotch(keepExpanded:Bool=false){
        voiceExpanded=false;voiceApprovalMenuItem?.isHidden=true
        compactVoiceMode=false;compactVoiceError=nil;compactVoiceIndicator=nil;voiceCaptureSource=nil;currentScene=""
        if let path=pendingVoiceStart?.context?.imagePath{attachmentFiles.append(URL(fileURLWithPath:path))}
        voicePrompt.cancel();voiceLaunchTimeout?.cancel();voiceLaunchTimeout=nil;pendingVoiceStart=nil
        cancelShortcutHold();shortcutPreparation.cancel()
        chatVoice.stop()
        hidePetGallery(restore:false)
        pendingAutoSend=false;finalizingDictation=false;dictationGeneration=UUID()
        if dictating{finishDictation(allowAutoSend:false)}
        pinned=false
        if keepExpanded {
            compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)
        } else if let state=latestState, state.activeTurnId != nil || !state.activeTasks.isEmpty || state.activity.scene == "thinking" {
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
        if compactVoiceMode{renderCompactVoice();return}
        guard open,!dictating,!finalizingDictation,!miniListening else{return}
        if mediaVisible && activity.scene == "off"{return}
        let scene=IntegrationIcons.id(for:activity.text) ?? activity.scene
        guard currentScene != scene else{return};currentScene=scene
        sounds.playScene(activity.scene)
        switch activity.scene {case "off":indicator.hide();case "thinking":indicator.showThinking();default:indicator.showScene(scene)}
    }

    private func setMotion(_ id:String,replay:Bool=false){
        guard catalog.stateAnimations[id] != nil,(motionID != id || replay) else{return}
        motionID=id;play();refreshChecks()
    }
    private func syncMotion(force:Bool=false){
        if compactVoiceMode{renderCompactVoice();return}
        if !force && ["waving","running-left","jumping","failed"].contains(motionID){return}
        let motion:String
        let activity=latestState.map{miniOpen ? $0.viewedActivity:$0.activity}
        if dictating || finalizingDictation || miniListening || (sendingPrompt == nil && !(composerField?.stringValue.isEmpty ?? true)) {motion="waiting"}
        else if activity?.scene == "permission" || activity?.scene == "question" {motion="waiting"}
        else if sendingPrompt != nil || latestState?.activeTurnId != nil || (activity?.scene != "off" && activity?.scene != "failure" && activity != nil) {motion="running"}
        else {motion="idle"}
        setMotion(motion)
    }

    private func loadEarlierHistory(){
        guard miniOpen,earlierHistoryRequest == nil,let state=latestState,state.historyHasMore == true,let id=state.threadId else{return}
        let request=UUID().uuidString;earlierHistoryRequest=request
        historyControl.update(hasMore:true,loading:true)
        bridge.send("history_more",["threadId":id,"requestId":request])
    }
    private func renderConversation(_ history:[KaiMessage],forceBottom:Bool=false,preserveViewport:Bool=false){
        guard let view=conversationText,let scroll=conversationScroll else{return}
        // Most conversations need no WebKit process. Start it only for equations.
        if miniOpen,renderMath,!mathRenderer.isInstalled,history.contains(where:{MathExpression.containsRenderableMath($0.lines)}),mathRenderer.install(in:scalableUI,below:scroll) {
            conversationRenderer.invalidateAll()
        }
        let anchor=ConversationViewport.anchor(view,in:scroll)
        historyControl.update(hasMore:latestState?.historyHasMore == true,loading:earlierHistoryRequest != nil)
        let followBottom=forceBottom || ConversationViewport.isAtBottom(view,in:scroll)
        let changed=conversationRenderer.render(history,threadID:latestState?.threadId,in:view,scroll:scroll,renderMath:renderMath,palette:assistantPalette,anchorCharacter:anchor?.character)
        guard conversationRenderer.didChange || forceBottom else{return}
        ConversationViewport.layout(view,in:scroll)
        if preserveViewport {ConversationViewport.revealOlderText(view,characters:conversationRenderer.prependedCharacterCount)}
        if !forceBottom,let anchor,(preserveViewport || !followBottom) {
            ConversationViewport.restore(anchor,adding:(conversationRenderer.mappedAnchorCharacter ?? anchor.character)-anchor.character,text:view,scroll:scroll)
        } else if changed && followBottom || forceBottom {scrollConversationToBottom(threadID:latestState?.threadId)}
    }
    private func scheduleMathRefresh(){
        guard renderMath,miniOpen,conversationRenderer.hasDirtyMath,mathRefreshWork == nil else{return}
        let work=DispatchWorkItem { [weak self] in
            guard let self else{return}
            self.mathRefreshWork=nil
            guard self.renderMath,let state=self.latestState else{return}
            guard self.miniOpen else{return}
            self.renderConversation(state.history)
        }
        mathRefreshWork=work
        DispatchQueue.main.asyncAfter(deadline:.now()+0.2,execute:work)
    }
    private func assistantGradientColors(for id:String)->[NSColor]{
        let stops=(gradientCatalog.profileByPetID[id]?.gradients.ambient.stops ?? []).sorted{$0.location<$1.location}
        guard !stops.isEmpty else{return [.white]}
        let colors=stops.map{stop -> NSColor in
            let value=UInt64(stop.color.dropFirst(),radix:16) ?? 0
            return NSColor(srgbRed:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255,alpha:1)
        }
        return (0..<64).map{sample in
            let position=Double(sample)/63
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
        guard latestState?.threadId == threadID,let view=conversationText,let scroll=conversationScroll else{return}
        ConversationViewport.bottom(view,in:scroll)
    }
    func textView(_ textView:NSTextView,clickedOnLink link:Any,at charIndex:Int)->Bool{guard let url=(link as? URL) ?? (link as? String).flatMap(URL.init(string:)) else{return false};if url.scheme == "kai-file-action",let action=url.host,let path=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "path"})?.value,path.hasPrefix("/"){fileActions.perform(action,url:URL(fileURLWithPath:path));return true};if url.scheme == "kai-reply",let answer=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "answer"})?.value{NSPasteboard.general.clearContents();NSPasteboard.general.setString(answer,forType:.string);return true};if url.isFileURL{miniUI.previewFile(url);return true};guard ["http","https"].contains(url.scheme?.lowercased() ?? "") else{return false};NSWorkspace.shared.open(url);return true}

    private func beginDictation(trigger:DictationTrigger){
        // Plain dictation owns the input presentation, even in a chat that
        // previously had a realtime call. Do not leave voice status rendering
        // active while Apple's dictation supplies text to the draft.
        compactVoiceMode=false;voiceExpanded=false;compactVoiceIndicator=nil;compactVoiceError=nil
        voiceApprovalMenuItem?.isHidden=true
        chatVoice.stop()
        guard !dictating else{return}
        pendingAutoSend=false;finalizingDictation=false;dictationGeneration=UUID();speech.cancelAudio()
        // Take ownership before retiring media, so cleanup cannot close the notch
        // or overwrite listening with an idle activity scene.
        dictating=true;dictationTrigger=trigger
        dismissMedia(closeNotch:false)
        sounds.startListening()
        dictationPrefix=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines)
        if !dictationPrefix.isEmpty{dictationPrefix += " "}
        setMotion("waiting");indicator.setExternalLevels([0,0,0,0]);indicator.startListening(useMicrophone:false);dictation.start()
    }

    private func finishDictation(allowAutoSend:Bool=true){
        guard dictating else{return}
        let autoSend=allowAutoSend && !(miniOpen && shortcutPreferences.expandedDictationOnly) && dictationTrigger.map{autoSendPreferences.enabled(for:$0)} == true
        let prefix=dictationPrefix, generation=dictationGeneration
        let keepCompact=autoSend && compactOnSend && !miniOpen
        finalizingDictation=true;dictating=false;dictationTrigger=nil;sounds.stopListening()
        dictation.stop{[weak self] final in
            guard let self,self.dictationGeneration == generation else{return}
            self.finalizingDictation=false
            self.setPrompt(prefix+final,dictation:true);self.syncMotion(force:true)
            if autoSend,!final.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty {
                self.pendingAutoSend=true;self.sendPendingDictation()
            } else if keepCompact {
                // Empty recognition still leaves a composer available for retry.
                self.setOpen(true,mini:true)
            }
            if !self.pendingAutoSend,self.sendingPrompt == nil,let state=self.latestState{self.showActivity(self.miniOpen ? state.viewedActivity:state.activity)}
            self.refreshMedia()
        }
        if miniOpen{exitMiniListening()}else if !keepCompact{setOpen(true,mini:true)}
    }

    private func enterMiniListening(trigger:DictationTrigger) {
        guard miniOpen,!miniListening else{return};hidePetGallery();miniListening=true;if miniUI.isFilesOpen{miniUI.toggleFiles()};indicator.setFileOpen(false);beginDictation(trigger:trigger)
        let center=(canvasWidth+notchWidth)/2+(expandedWidth-notchWidth)/4,duration=0.62
        for layer in [indicator.layer,indicator.fileLayer] { let move=CABasicAnimation(keyPath:"position.x");move.fromValue=layer.presentation()?.position.x ?? layer.position.x;move.toValue=center;move.duration=duration;move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.position.x=center;CATransaction.commit();layer.add(move,forKey:"listenMerge") }
        setFileButtonVisible(false,duration:duration)
    }

    private func exitMiniListening(){guard miniListening else{return};miniListening=false;let center=(canvasWidth+notchWidth)/2+(expandedWidth-notchWidth)/4,duration=0.62;indicator.showScene("settings");for(layer,target) in [(indicator.layer,center-20),(indicator.fileLayer,center+20)]{let move=CABasicAnimation(keyPath:"position.x");move.fromValue=layer.presentation()?.position.x ?? layer.position.x;move.toValue=target;move.duration=duration;move.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);CATransaction.begin();CATransaction.setDisableActions(true);layer.position.x=target;CATransaction.commit();layer.add(move,forKey:"listenSeparate")};setFileButtonVisible(true,duration:duration)}

    private func handleImageEntered(){guard !miniOpen else{return};sounds.playScene("image");pinned=true;if !open{setOpen(true)};indicator.showScene("image")}
    private func handleImageDrop(_ images:[NSImage]){guard !images.isEmpty,ProcessInfo.processInfo.systemUptime-lastImageDrop>0.35 else{return};lastImageDrop=ProcessInfo.processInfo.systemUptime;approachImages.removeAll();approachActive=false;miniUI.addImages(images);if miniOpen{return};sounds.playScene("success");indicator.showScene("success");imageExpandWork?.cancel();let work=DispatchWorkItem{[weak self] in guard let self,self.open,!self.miniOpen else{return};self.setOpen(true,mini:true)};imageExpandWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.72,execute:work)}
    private func globalDrag(_ event:NSEvent){guard !experimentalWebMode,let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil}) else{return};let p=NSEvent.mouseLocation,nearNotch=abs(p.x-screen.frame.midX)<notchWidth/2+45 && p.y>screen.frame.maxY-notchHeight-70;if event.type == .leftMouseDragged,nearNotch{let images=draggedImages(NSPasteboard(name:.drag));guard !images.isEmpty else{return};approachImages=images;if !approachActive{approachActive=true;handleImageEntered()}}else if event.type == .leftMouseUp{defer{approachActive=false;approachImages.removeAll()};if approachActive,nearNotch,!approachImages.isEmpty{handleImageDrop(approachImages)}}}

    @objc private func changeMediaSize(_ slider:NSSlider){mediaSize=CGFloat(slider.doubleValue);UserDefaults.standard.set(Double(mediaSize),forKey:"mediaSize");layoutMediaArtwork(animated:true)}
    private func layoutMediaArtwork(animated:Bool){
        guard let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil}) else{return}
        let expanded=expandedWidth,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2
        let center=CGPoint(x:(physicalLeft+kaiLeft)/2,y:canvasHeight-notchHeight/2)
        let frame=NotchVisualTransition.artworkFrame(size:mediaSize,notchHeight:notchHeight,center:center,scale:screen.backingScaleFactor)
        visualTransition.resize(mediaArtwork,to:frame,animated:animated)
    }
    private func layoutPet() {
        guard let screen=NSScreen.screens.first(where:{$0.auxiliaryTopLeftArea != nil}) else{return};let expanded=expandedWidth,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2,scale=screen.backingScaleFactor,topBandY=canvasHeight-notchHeight,h=min(36,notchHeight-3)*petScale,w=h*192/208
        func px(_ v:CGFloat)->CGFloat{round(v*scale)/scale};CATransaction.begin();CATransaction.setDisableActions(true);sprite.frame=CGRect(x:px((physicalLeft+kaiLeft)/2-w/2),y:px(topBandY+(notchHeight-h)/2),width:px(w),height:px(h));CATransaction.commit()
    }

    private func animateTopAccessories(forMini mini:Bool,duration:Double) {
        let shift:CGFloat=mini ? -20:0, currentX=indicator.layer.presentation()?.position.x ?? indicator.layer.position.x, targetX=(canvasWidth+notchWidth)/2+(expandedWidth-notchWidth)/4+shift
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
            let url=assetRoot.appendingPathComponent(pet.spritesheet)
            if let decoded=spriteAssets.image(at:url) {
                loadedSpritePetID=petID;loadedSprite=decoded;image=decoded
            } else {
                let requestedPetID=petID
                guard loadingSpritePetID != requestedPetID else{return}
                loadingSpritePetID=requestedPetID
                spriteAssets.load(url) { [weak self] decoded in
                    guard let self else{return}
                    if self.loadingSpritePetID == requestedPetID {self.loadingSpritePetID=nil}
                    guard self.petID == requestedPetID else{return}
                    guard let decoded else {
                        if let previous=self.loadedSpritePetID {self.usePet(previous)}
                        self.miniUI.setCodexMessage("Could not load pet; keeping the previous pet")
                        return
                    }
                    self.loadedSpritePetID=requestedPetID;self.loadedSprite=decoded;self.play()
                }
                return
            }
        }
        if renderedPetID != petID && !renderedPetID.isEmpty {let blend=CATransition();blend.type = .fade;blend.duration=0.36;blend.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);sprite.add(blend,forKey:"petBlend")}
        CATransaction.begin(); CATransaction.setDisableActions(true); sprite.contents = image;sprite.isHidden=false;sprite.opacity=1; CATransaction.commit()
        renderedPetID=petID
        syncConversationTheme()
        sprite.contentsScale = NSScreen.main?.backingScaleFactor ?? 2; frame = 0; playToken += 1
        showFrame(pet, motion, playToken);if petGalleryVisible{petGallery.play(motionID)}
    }

    private func showFrame(_ pet: Pet, _ motion: Motion, _ token: Int) {
        guard token == playToken,open || compactWhileWorking || experimentalWebMode,
              let timeline=SpriteTimeline(row:motion.row,rows:pet.grid.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop) else{return}
        sprite.removeAnimation(forKey:"spriteFrames")
        let reduceMotion=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        CATransaction.begin();CATransaction.setDisableActions(true)
        sprite.contentsRect=motion.loop || reduceMotion ? timeline.frames[0]:timeline.frames[timeline.frames.count-1]
        CATransaction.commit()
        if !reduceMotion,timeline.frames.count>1 {sprite.add(timeline.animation(),forKey:"spriteFrames")}
        if !motion.loop {
            DispatchQueue.main.asyncAfter(deadline:.now()+timeline.duration) { [weak self] in
                guard let self,token == self.playToken else{return}
                self.syncMotion(force:true)
            }
        }
    }

    @objc private func changeDefaultNotchSize(_ slider:NSSlider){
        guard let key=slider.identifier?.rawValue else{return}
        UserDefaults.standard.set(slider.doubleValue,forKey:key);restoreDefaultNotchSize()
    }
    @objc private func saveDefaultNotchSize(){
        guard let size=expandedSize else{return}
        UserDefaults.standard.set(Double(size.width),forKey:"notchDefaultWidth");UserDefaults.standard.set(Double(size.height),forKey:"notchDefaultHeight")
        defaultWidthSlider?.doubleValue=Double(size.width);defaultHeightSlider?.doubleValue=Double(size.height)
        sizeLabels["notchDefaultWidth"]?.stringValue="Width · \(Int(size.width)) pt";sizeLabels["notchDefaultHeight"]?.stringValue="Height · \(Int(size.height)) pt"
    }
    @objc private func restoreDefaultNotchSize(){expandedSize=nil;resizeWithMorph()}
    @objc private func resetDefaultNotchSize(){
        UserDefaults.standard.removeObject(forKey:"notchDefaultWidth");UserDefaults.standard.removeObject(forKey:"notchDefaultHeight")
        restoreDefaultNotchSize();defaultWidthSlider?.doubleValue=Double(notchWidth+160);defaultHeightSlider?.doubleValue=320
    }
    private func resizeWithMorph(){
        let previous=mask.presentation()?.path ?? mask.path,oldHeight=canvasHeight,oldWidth=canvasWidth
        placePanel()
        guard let previous else{return};var translation=CGAffineTransform(translationX:(canvasWidth-oldWidth)/2,y:canvasHeight-oldHeight)
        let animation=CABasicAnimation(keyPath:"path");animation.fromValue=previous.copy(using:&translation);animation.toValue=mask.path;animation.duration=0.42;animation.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.65,0.25,1);mask.add(animation,forKey:"width")
    }
    @objc private func toggleFromMenu() { cancelSizeTest();if compactVoiceMode{setOpen(true,mini:!miniOpen,userInitiated:true);return};if compactWhileWorking {compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)}else if open{closeNotch()}else{pinned=true;setOpen(true,mini:true)} }
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
    private func usePet(_ id:String,explicit:Bool=true){if explicit{petSelection.selectedID=id};userPetID=id;petID=id;if let chatID=miniOpen ? latestState?.threadId : (focusedTaskID ?? latestState?.threadId){taskPets[chatID]=id;if explicit{explicitTaskPets.insert(chatID)}};syncConversationTheme();petGallery.layout(in:petGallery.layer.bounds,selected:id);refreshChecks();play()}
    @objc private func closePetPicker(){status.menu?.cancelTracking()}
    @objc private func toggleMathRendering(){
        renderMath.toggle()
        UserDefaults.standard.set(renderMath,forKey:"renderMath")
        if !renderMath { mathRenderer.suspend() }
        conversationRenderer.invalidateAll()
        if let state=latestState { renderConversation(state.history) }
        refreshChecks()
    }
    @objc private func toggleSounds(){soundsEnabled.toggle();sounds.enabled=soundsEnabled;UserDefaults.standard.set(soundsEnabled,forKey:"soundsEnabled");refreshChecks()}
    @objc private func toggleCodexPetFilter(){
        petSelection.excludeCodexPets.toggle()
        if petSelection.excludeCodexPets {
            for id in Array(taskPets.keys) where !explicitTaskPets.contains(id) && PetSelectionPreferences.excludedIDs.contains(taskPets[id] ?? ""){taskPets.removeValue(forKey:id)}
            if PetSelectionPreferences.excludedIDs.contains(petID),petSelection.selectedID != petID,let fallback=petSelection.automaticIDs(catalog.pets.map(\.id)).first{usePet(fallback,explicit:false)}
            if let state=latestState{syncTaskPet(state)}
        }
        refreshChecks()
    }
    @objc private func randomizePet(){
        let occupied=Set(latestState?.activeTasks.compactMap{task in task.id == focusedTaskID ? nil:taskPets[task.id]} ?? [])
        let choices=petSelection.automaticIDs(catalog.pets.map(\.id)).filter{$0 != petID && !occupied.contains($0)}
        let fallback=petSelection.automaticIDs(catalog.pets.map(\.id)).filter{$0 != petID}
        if let id=(choices.isEmpty ? fallback:choices).randomElement(){petSelection.selectedID=nil;usePet(id,explicit:false)}
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
    @objc private func toggleCodeConnection(){let enabled = !(UserDefaults.standard.object(forKey:"connectCode") as? Bool ?? true);UserDefaults.standard.set(enabled,forKey:"connectCode");bridge.send("code_connection",["enabled":enabled]);refreshChecks()}
    private func refreshGlassLabels(){for control in GlassPreferences.controls{let value=glassSliders[control.key]?.doubleValue ?? control.initial;glassLabels[control.key]?.stringValue=control.title+" · "+(control.key == "radius" ? "\(Int(value)) pt":"\(Int(value*100))%")}}
    @objc private func changeGlass(_ slider:NSSlider){guard let key=slider.identifier?.rawValue else{return};UserDefaults.standard.set(slider.doubleValue,forKey:"glass."+key);refreshGlassLabels();updateGlassBackground();if key == "radius"{placePanel()}}
    @objc private func chooseGlass(_ item:NSMenuItem){guard let key=item.representedObject as? String else{return};UserDefaults.standard.set(item.tag,forKey:"glass."+key);for choice in glassChoiceItems where choice.representedObject as? String == key{choice.state=choice.tag == item.tag ? .on:.off};updateGlassBackground()}
    @objc private func resetGlass(){GlassPreferences.reset(.standard);for control in GlassPreferences.controls{glassSliders[control.key]?.doubleValue=control.initial};for item in glassChoiceItems{item.state=item.tag == ((item.representedObject as? String) == "blur" ? 1:0) ? .on:.off};refreshGlassLabels();placePanel()}
    @objc private func toggleBottomGlass(){glassEnabled.toggle();UserDefaults.standard.set(glassEnabled,forKey:"notchBottomGlass");placePanel();refreshChecks()}
    private func updateGlassBackground(){
        guard let root=panel?.contentView?.layer else{return}
        let enabled=glassEnabled && open && miniOpen && !experimentalWebMode
        root.backgroundColor=enabled ? NSColor.clear.cgColor:NSColor.black.cgColor
        glassBackground.update(canvas:CGSize(width:canvasWidth,height:canvasHeight),width:expandedWidth,enabled:enabled,color:assistantPalette.first ?? .systemBlue,preferences:GlassPreferences(defaults:.standard))
    }
    private func refreshChecks() {
        codeConnectionItem?.state=(UserDefaults.standard.object(forKey:"connectCode") as? Bool ?? true) ? .on:.off
        petFilterItem?.state=petSelection.excludeCodexPets ? .on:.off
        glassItem?.state=glassEnabled ? .on:.off
        experimentalModeItem?.state=experimentalWebMode ? .on:.off
        mathModeItem?.state=renderMath ? .on:.off
        soundsItem?.state=soundsEnabled ? .on:.off
        petMenuRows.forEach { $0.value.selected = $0.key == petID }; motionItems.forEach { $0.value.state = $0.key == motionID ? .on:.off }
        indicatorItems.forEach { $0.value.state = $0.key == indicatorID ? .on:.off }
        accentItems.forEach { $0.value.state = $0.key == accentSide ? .on:.off }
        showItem?.title = compactVoiceMode ? (miniOpen ? "compact notch":"expand notch"):(open ? "hide notch":"show notch")
    }
}

@main struct Main {
    @MainActor static func main() {
        if CommandLine.arguments.contains("--diagnose-codex-voice-media") {
            CodexVoiceMediaDiagnostic.run(heldPrompt:CommandLine.arguments.contains("--held-prompt"));return
        }
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
