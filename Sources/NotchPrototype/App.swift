import AppKit
import AVFoundation
import ImageIO
import WebKit
import UniformTypeIdentifiers

struct Catalog: Decodable { let pets: [Pet]; let stateAnimations: [String: Motion] }
struct Pet: Decodable { let id, displayName, spritesheet: String; let grid: Grid; let animationOverrides:[String:Motion]?;var sourceURL:URL?=nil;var kind:String?=nil;var category:String?=nil }
struct Grid: Decodable { let rows: Int }
struct Motion: Codable { let row: Int; let columns, durationsMs: [Int]; let loop: Bool }
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

/// NSTextField's default field editor hugs the top of a taller editing row.
/// Use the same centered text rect for drawing, clicking and live editing.
final class ComposerTextCell:NSTextFieldCell {
    override func drawingRect(forBounds rect:NSRect)->NSRect{
        let base=super.drawingRect(forBounds:rect),face=font ?? AppTypography.ui(ofSize:10)
        let measured=attributedStringValue.boundingRect(with:CGSize(width:max(1,base.width),height:.greatestFiniteMagnitude),options:[.usesLineFragmentOrigin,.usesFontLeading]).height
        let height=min(base.height,ceil(max(face.ascender-face.descender+face.leading,measured)))
        return CGRect(x:base.minX,y:base.midY-height/2,width:base.width,height:height)
    }
    override func edit(withFrame rect:NSRect,in controlView:NSView,editor:NSText,delegate:Any?,event:NSEvent?){super.edit(withFrame:drawingRect(forBounds:rect),in:controlView,editor:editor,delegate:delegate,event:event)}
    override func select(withFrame rect:NSRect,in controlView:NSView,editor:NSText,delegate:Any?,start:Int,length:Int){super.select(withFrame:drawingRect(forBounds:rect),in:controlView,editor:editor,delegate:delegate,start:start,length:length)}
}

private final class ComposerField:NSTextField {
    override class var cellClass:AnyClass?{get{ComposerTextCell.self}set{}}
    override func acceptsFirstMouse(for event:NSEvent?)->Bool{true}
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
    override func acceptsFirstMouse(for event:NSEvent?)->Bool{true}
    var click:((CGPoint,Int)->Void)?
    var scroll:((CGPoint,CGFloat)->Void)?
    var hover:((CGPoint?)->Void)?
    var beginScrollDrag:((CGPoint)->Bool)?,scrollDrag:((CGPoint)->Void)?,endScrollDrag:(()->Void)?
    private var draggingScroll=false
    override func updateTrackingAreas(){super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea);addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseMoved,.mouseEnteredAndExited,.activeAlways],owner:self))}
    override func mouseDown(with event:NSEvent) {
        draggingScroll=beginScrollDrag?(event.locationInWindow) ?? false
        if !draggingScroll{click?(event.locationInWindow,event.clickCount)}
    }
    override func mouseDragged(with event:NSEvent){if draggingScroll{scrollDrag?(event.locationInWindow)}}
    override func mouseUp(with event:NSEvent){if draggingScroll{endScrollDrag?();draggingScroll=false}}
    override func scrollWheel(with event:NSEvent) { scroll?(event.locationInWindow,event.scrollingDeltaY) }
    override func mouseMoved(with event:NSEvent){hover?(event.locationInWindow)}
    override func mouseExited(with event:NSEvent){hover?(nil)}
}

private final class NotchSurfaceHost:NSView {
    override func acceptsFirstMouse(for event:NSEvent?)->Bool{true}
}

@MainActor final class KaiPetApp: NSObject, NSApplicationDelegate, NSTextFieldDelegate, NSTextViewDelegate {
    private var unifiedMenuItems=[NSMenuItem]()
    private let menuBarIdentity=MenuBarIdentityMenu()
    private var statusContextMenu:NSMenu?,menuQuickAnchor:CGRect?
    private let nativeReview=NativeRequestReview()
    private let inlinePetImages=SpriteAssetCache(maxPixelSize:960)
    private var inlinePetLoading=Set<URL>(),inlinePetVisuals=[String:InlineTaskPet](),lastPetVisualPayload:Data?
    private let slashPicker=NativeSlashPicker()
    private var pendingSlashID:String?
    private var pendingSlashPicker:String?
    private var pendingSlashDraft:String?
    private let typographyMenu=TypographyMenu()
    private var petLibraryRevision=0
    private let petLibraryQueue=DispatchQueue(label:"kai.pet-library",qos:.utility)
    private let petLibraryMonitor=LocalPetMonitor()
    private var petLibraryFingerprint=""

    private let quickMorph=WorkspaceMorph()
    private weak var quickWindow:NSWindow?
    private var quickPetSlot=CGRect(x:12,y:8,width:30,height:30)
    private enum QuickReturn {case workspace,extended,compact,hidden,handoff}
    private var quickReturn:QuickReturn = .hidden,quickRevision=0
    private var quickPrepared:(workspace:Bool,source:CGRect,returnTo:QuickReturn)?
    private var commandChordActive=false
    private let hangDiagnostics=HangDiagnostics()
    private var unifiedDraftBase=""
    private let workspaceMorph=WorkspaceMorph()
    private var quickChatVisible=false,workspaceVoiceRequest:String?
    private var completionIndicatorWork:DispatchWorkItem?
    private var workspaceFocusRefreshPending=false
    private var workspaceNotchOverlay=false
    private var workspaceWasForeground:Bool?
    private var passiveCompact=false,passiveVoice=false
    private var passiveRetire:DispatchWorkItem?
    private var workspaceAutomaticStatus=false
    private let workspaceStatusMorph=WorkspaceStatusMorph()
    private var workspaceStatusRetire:DispatchWorkItem?
    private var workspaceHold=WorkspaceHoldGesture(),workspaceHoldJobs=[DispatchWorkItem]()
    private let workspacePet=WorkspacePetView(frame:.zero)
    private var workspacePetMenu:NSMenu?,workspacePetSlot=CGRect(x:215,y:36,width:48,height:34)
    private var reviewedFinished=[String]()
    private let glassBackground=NotchGlassBackground(frame:.zero)
    private let workspaceBackground=NotchWorkspaceBackground(frame:.zero)
    private let ninjaScene=NinjaAtmosphereView(frame:.zero)
    private var ninjaWorkspaceSlot=CGRect.zero
    private var codeWallpaperEnabled=UserDefaults.standard.bool(forKey:"notchCodeWallpaper") || CommandLine.arguments.contains("--unified-wallpaper-probe"),codeWallpaperItem:NSMenuItem?
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
    private let voicePrompt:HoldToVoiceController
    private let modifierFlags:()->NSEvent.ModifierFlags
    private let keyDownCount:()->UInt32
    private var shortcutKeyCount:UInt32?
    private let contextCapture:((Bool,@escaping(ApplicationContext)->Void)->Void)?
    init(voiceRecorder:VoicePromptRecording?=nil,shortcutDefaults:UserDefaults = .standard,modifierFlags:@escaping()->NSEvent.ModifierFlags={NSEvent.modifierFlags},keyDownCount:@escaping()->UInt32={CGEventSource.counterForEventType(.combinedSessionState,eventType:.keyDown)},capture:((Bool,@escaping(ApplicationContext)->Void)->Void)?=nil){
        shortcutPreferences=ShortcutPreferences(defaults:shortcutDefaults);voicePrompt=HoldToVoiceController(recorder:voiceRecorder);self.modifierFlags=modifierFlags;self.keyDownCount=keyDownCount;contextCapture=capture;super.init()
    }
    private var compactVoiceMode=false,compactVoiceError:String?
    private var voiceShortcutArmed=false
    private var beforeVoiceShortcut:(open:Bool,mini:Bool,pinned:Bool,working:Bool,automatic:Bool)?
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
    private var attachmentRequest:(id:String,count:Int)?
    private let fileViewer=FileViewerContent(),fileActions=FileActionsMenu()
    private var motionItems = [String: NSMenuItem](), indicatorItems = [String: NSMenuItem](), accentItems = [String: NSMenuItem]()
    private var localMonitor: Any?, globalMonitor: Any?, dragMonitor:Any?, showItem: NSMenuItem!
    private var morphWork = [DispatchWorkItem](),petClickWork:DispatchWorkItem?
    private let petSelection=PetSelectionPreferences(defaults:.standard)
    private let textRevealPreferences=TextRevealPreferences(),textReveal=ConversationTextReveal()
    private let dictationTextReveal=ConversationTextReveal()
    private var ownerAttachmentImages=[(path:String,image:NSImage)](),attachmentOwnerPaths=[String]()
    private var morphingText=false,textMorphItem:NSMenuItem?
    private let petAppIcon=PetAppIcon()
    private var petFilterItem:NSMenuItem?,explicitTaskPets=Set<String>()
    private var petID = "codex", userPetID = "codex", renderedPetID = "", motionID = "idle", indicatorID = "demo", frame = 0, playToken = 0
    private var compactPetCycle:DispatchWorkItem?,compactPetCycleID:String?
    private var ninjaSpriteStyle:String?,ninjaFrames=[String:[CGImage]]()
    private var taskPets=[String:String](),focusedTaskID:String?,autoCompact=false
    private let floatingTasks=FloatingTaskPets()
    private var floatingFocus:String?,floatingReviews=[KaiActiveTask]()
    private var loadedSpritePetID:String?,loadedSprite:CGImage?
    private var assistantPalette=[NSColor]()
    private let conversationFold=ConversationFoldTransition()
    private var conversationPetID=""
    private var sendingPromptRequestID:String?
    private var expandedChatRequest:(id:String,target:String,name:String)?
    private let spriteAssets=SpriteAssetCache()
    private var loadingSpritePetID:String?
    private var petScale: CGFloat = 1
    private var accentSide = "right"
    private let autoSendPreferences=DictationSendPreferences(defaults:.standard)
    private var autoSendMenu=NSMenu()
    private let shortcutPreferences:ShortcutPreferences
    private var shortcutMenu=NSMenu()
    private var expandedDictationItem:NSMenuItem?
    private var shortcutWork:DispatchWorkItem?
    private var shortcutGeneration=UUID()
    private var pendingShortcutHold:(trigger:DictationTrigger,action:ShortcutAction,expanded:Bool,deadline:TimeInterval,generation:UUID)?
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
    private let approvalUI=CompactApprovalView()
    private var approvalDismissal=ApprovalDismissal()
    private var approvalPopout=false,presentedApproval=false,lastApproval:CompactApprovalRequest?
    private var presentedApprovalHeight:CGFloat=0
    private var beforeApproval=(open:false,mini:false,pinned:false,automatic:false)
    private var popoutHeight:CGFloat {approvalPopout ? notchHeight+24+approvalUI.preferredHeight:(miniOpen || experimentalWebMode ? canvasHeight:notchHeight)}
    private var pendingFilesRequestID:String?
    private var miniListening = false,dictating=false,finalizingDictation=false,dictationPrefix=""
    private var approachImages=[NSImage](),approachActive=false,lastImageDrop=0.0
    private var imageExpandWork:DispatchWorkItem?
    private var lastControlTap = 0.0
    private var commandDismissGesture=ModifierDoubleTap()
    private var holdWork: DispatchWorkItem?
    private let scalableUI=NotchSurfaceHost(),resizeHandles=NotchResizeHandles()
    private var expandedSize:CGSize?,resizeInitial=CGSize.zero,resizeMode=NotchResizeMode.both
    private var defaultWidthSlider:NSSlider?,defaultHeightSlider:NSSlider?
    private var sizeLabels=[String:NSTextField]()
    private var expandedWidth:CGFloat { (miniOpen || experimentalWebMode) ? (expandedSize?.width ?? notchWidth+160) : notchWidth+160 }
    // Keep every surface anchored to the same display and floating top edge.
    private var surfaceScreen: NSScreen? {
        NSScreen.screens.first(where: { $0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil }) ?? NSScreen.main ?? NSScreen.screens.first
    }
    private var islandMode = false
    private var surfaceTopInset: CGFloat { islandMode ? 12 : 0 }
    private var islandRadius: CGFloat { min(26, notchHeight / 2) }
    private var notchWidth: CGFloat = 210, notchHeight: CGFloat = 39, canvasWidth: CGFloat = 470, canvasHeight: CGFloat = 320
    private lazy var assetRoot: URL = {
        if let resources=Bundle.main.resourceURL {
            let bundled=resources.appendingPathComponent("kai_pets")
            if FileManager.default.fileExists(atPath:bundled.appendingPathComponent("animation_catalog.json").path){return bundled}
        }
        return URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("kai_pets")
    }()

    private func applyTypography(interface:String,code:String){
        guard AppTypography.apply(interface:interface,code:code) else{return}
        composerField?.font=AppTypography.ui(ofSize:10)
        textReveal.finish();AppTypography.update(miniUI.layer);conversationRenderer.invalidateAll()
        if let state=latestState,miniOpen{renderConversation(state.history,forceBottom:false,preserveViewport:true)}
    }
    private func populatePetMenu(_ menu:NSMenu,pets:[Pet]){
        for item in menu.items where item.identifier?.rawValue=="pet-category"{menu.removeItem(item)}
        petMenuRows.removeAll()
        var position=menu.items.firstIndex(where:{$0.title=="done"}).map{max(0,$0-1)} ?? menu.items.count
        for category in PetCategory.allCases {
            let group=pets.filter{PetCategory.of($0)==category}.sorted{$0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending}
            guard !group.isEmpty else{continue}
            let parent=NSMenuItem(title:"\(category.rawValue) (\(group.count))",action:nil,keyEquivalent:"");parent.identifier=NSUserInterfaceItemIdentifier("pet-category");let child=NSMenu(title:category.rawValue);parent.submenu=child
            for pet in group {let item=NSMenuItem(title:pet.displayName,action:nil,keyEquivalent:"");let row=PetMenuRow(title:pet.displayName);row.selected=pet.id==petID;row.onSelect={[weak self] in self?.usePet(pet.id);self?.workspacePetMenu?.cancelTracking();self?.status.menu?.cancelTracking()};item.view=row;child.addItem(item);petMenuRows[pet.id]=row}
            menu.insertItem(parent,at:position);position+=1
        }
    }
    @objc private func importPetPack(){
        let picker=NSOpenPanel();picker.title="Import a pet pack";picker.canChooseDirectories=true;picker.canChooseFiles=true;picker.allowsMultipleSelection=true
        picker.allowedContentTypes=[.zip,.png,UTType(filenameExtension:"webp") ?? .image]
        picker.begin{[weak self] result in guard let self,result == .OK else{return};let urls=picker.urls
            self.petLibraryQueue.async{[weak self] in
                var failures=[String]()
                for url in urls {do{try PetPackImporter.install(url,to:PetPackImporter.storage())}catch{failures.append(url.lastPathComponent+": "+error.localizedDescription)}}
                let messages=failures
                DispatchQueue.main.async{[weak self] in self?.rescanLocalPets();if !messages.isEmpty{let alert=NSAlert();alert.messageText="Some pet packs could not be imported";alert.informativeText=messages.joined(separator:"\n");alert.runModal()}}
            }
        }
    }
    @objc private func choosePetFolder(){
        let picker=NSOpenPanel();picker.title="Choose a local pet library";picker.canChooseDirectories=true;picker.canChooseFiles=false;picker.allowsMultipleSelection=false
        picker.begin{[weak self] result in guard result == .OK,let url=picker.url else{return};UserDefaults.standard.set(url.path,forKey:"localPetFolder");self?.rescanLocalPets()}
    }
    @objc private func rescanLocalPets(){
        guard let catalog else{return};petLibraryRevision+=1;let revision=petLibraryRevision,roots=LocalPetLibrary.roots(),priorPet=petID
        let bundled=assetRoot
        let automatic = !CommandLine.arguments.contains("--unified-smoke-test")
        let downloads=FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Downloads")
        petLibraryQueue.async{[weak self] in
            if automatic{PetPackImporter.importDownloads(downloads,to:PetPackImporter.storage())}
            let local=LocalPetLibrary.discover(in:roots),profiles=LocalPetLibrary.profiles(local,cache:automatic)
            let fingerprint=LocalPetLibrary.fingerprint(local)
            let base=(try? JSONDecoder().decode(Catalog.self,from:Data(contentsOf:bundled.appendingPathComponent("animation_catalog.json")))) ?? catalog
            let updated=LocalPetLibrary.merge(local,into:base)
            DispatchQueue.main.async{[weak self] in
                guard let self,self.petLibraryRevision == revision else{return}
                if automatic{self.petLibraryMonitor.watch(roots)}
                guard self.petLibraryFingerprint != fingerprint else{return};self.petLibraryFingerprint=fingerprint
                let unchanged=self.petID == priorPet
                // Status can change the displayed pet during disk discovery.
                // Migrate the preference and assignments independently of it.
                self.petID=LocalPetLibrary.migratedID(self.petID,in:base,local:local)
                self.userPetID=LocalPetLibrary.migratedID(self.userPetID,in:base,local:local)
                if let selected=self.petSelection.selectedID{self.petSelection.selectedID=LocalPetLibrary.migratedID(selected,in:base,local:local)}
                self.taskPets=self.taskPets.mapValues{LocalPetLibrary.migratedID($0,in:base,local:local)}
                self.catalog=updated;self.gradientCatalog.profileByPetID.merge(profiles){_,new in new}
                let available=Set(updated.pets.map(\.id));self.taskPets=self.taskPets.filter{available.contains($0.value)};self.explicitTaskPets.formIntersection(self.taskPets.keys)
                let specs=updated.pets.map{pet -> PetGallerySpec in
                    var motions=updated.stateAnimations;pet.animationOverrides?.forEach{motions[$0.key]=$0.value}
                    let stops=self.gradientCatalog.profileByPetID[pet.id]?.gradients.ambient.stops ?? []
                    let colors=stops.map{stop -> CGColor in let n=UInt64(stop.color.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((n>>16)&255)/255,green:CGFloat((n>>8)&255)/255,blue:CGFloat(n&255)/255,alpha:0.9).cgColor}
                    return PetGallerySpec(id:pet.id,spritesheet:pet.spritesheet,rows:pet.grid.rows,motions:motions.mapValues{PetGalleryMotion(row:$0.row,columns:$0.columns,durationsMs:$0.durationsMs,loop:$0.loop)},colors:colors,sourceURL:pet.sourceURL,category:PetCategory.of(pet).rawValue,displayName:pet.displayName)
                }
                self.petGallery.configure(specs,at:self.assetRoot);self.floatingTasks.configure(specs,profiles:self.gradientCatalog.profileByPetID,root:self.assetRoot)
                if let menu=self.workspacePetMenu{self.populatePetMenu(menu,pets:updated.pets)}
                if self.petGalleryVisible{self.petGallery.layout(in:self.petGallery.layer.bounds,selected:self.petID);self.petGallery.show(selected:self.petID,motion:self.motionID)}
                self.spriteAssets.invalidate();self.loadingSpritePetID=nil;self.loadedSpritePetID=nil;self.loadedSprite=nil
                if unchanged,let selected=self.petSelection.selectedID,updated.pets.contains(where:{$0.id == selected}){self.petID=selected;self.userPetID=selected}
                if !updated.pets.contains(where:{$0.id == self.petID}),let initial=self.petSelection.initialID(updated.pets.map(\.id)){self.petID=initial;self.userPetID=initial}
                self.inlinePetImages.invalidate();self.inlinePetLoading.removeAll();self.inlinePetVisuals.removeAll();self.lastPetVisualPayload=nil
                self.reconcileTaskPets(priority:self.focusedTaskID ?? self.latestState?.threadId ?? "")
                self.conversationPetID="";self.syncConversationTheme();self.refreshChecks();self.play();self.syncInlinePets();self.floatingTasks.refresh(pets:self.taskPets)
            }
        }
    }

    func applicationDidFinishLaunching(_ note: Notification) {
        hangDiagnostics.start()
        NSApp.setActivationPolicy(.regular)
        // Permission-only repair/diagnostic entry point: no capture starts here.
        if CommandLine.arguments.contains("--request-microphone-permission") {
            DispatchQueue.main.async {
                NSApp.activate(ignoringOtherApps:true)
                let before=AVCaptureDevice.authorizationStatus(for:.audio).rawValue
                print("KAI_MICROPHONE_PERMISSION before=\(before)")
                AVCaptureDevice.requestAccess(for:.audio){allowed in
                    print("KAI_MICROPHONE_PERMISSION allowed=\(allowed) status=\(AVCaptureDevice.authorizationStatus(for:.audio).rawValue)")
                }
            }
        }
        petScale = CGFloat(UserDefaults.standard.object(forKey: "petScale") as? Double ?? 1)
        experimentalWebMode = !EmbeddedCodeRuntime.shared.active && !CommandLine.arguments.contains("--preview-approval") && !CommandLine.arguments.contains("--preview-task-pets") && UserDefaults.standard.bool(forKey:"experimentalWebMode")
        morphingText=textRevealPreferences.enabled
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
        // Prime only remembered local choices from their validated metadata/cache;
        // the full scan and any new archive conversion stay on the worker queue.
        let remembered=[petSelection.lastUsedID,petSelection.selectedID].compactMap{$0}.filter{id in !catalog.pets.contains{$0.id==id}}
        let preferred=LocalPetLibrary.preferred(remembered)
        if !preferred.isEmpty{catalog=LocalPetLibrary.merge(preferred,into:catalog);gradientCatalog.profileByPetID.merge(LocalPetLibrary.cachedProfiles(preferred)){_,new in new}}
        if let initial=petSelection.initialID(catalog.pets.map(\.id)){petID=initial;userPetID=initial}
        let gallerySpecs=catalog.pets.map{pet in
            var motions=catalog.stateAnimations;pet.animationOverrides?.forEach{motions[$0.key]=$0.value}
            let galleryMotions=motions.mapValues{PetGalleryMotion(row:$0.row,columns:$0.columns,durationsMs:$0.durationsMs,loop:$0.loop)}
            let stops=gradientCatalog.profileByPetID[pet.id]?.gradients.ambient.stops ?? []
            let colors=stops.map{stop -> CGColor in let value=UInt64(stop.color.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((value>>16)&255)/255,green:CGFloat((value>>8)&255)/255,blue:CGFloat(value&255)/255,alpha:0.9).cgColor}
            return PetGallerySpec(id:pet.id,spritesheet:pet.spritesheet,rows:pet.grid.rows,motions:galleryMotions,colors:colors,sourceURL:pet.sourceURL,category:PetCategory.of(pet).rawValue,displayName:pet.displayName)
        }
        petGallery.configure(gallerySpecs,at:assetRoot)
        shortcutPreparation.supportsOwnedVoice=EmbeddedCodeRuntime.shared.active
        workspaceMorph.onPresentation={[weak self] phase in self?.hangDiagnostics.record("workspace/"+phase);EmbeddedCodeRuntime.shared.presentation(phase)}
        workspacePet.onClick={ [weak self] in guard let self else{return};self.workspacePetMenu?.popUp(positioning:nil,at:NSEvent.mouseLocation,in:nil) }
        petAppIcon.onCompletion={ [weak self] in self?.syncMotion(force:true) }
        floatingTasks.configure(gallerySpecs,profiles:gradientCatalog.profileByPetID,root:assetRoot)
        typographyMenu.onSelect={[weak self] slot,family in
            guard let self else{return}
            let interface=slot == "interface" ? family:AppTypography.interfaceFamily,code=slot == "code" ? family:AppTypography.codeFamily
            if EmbeddedCodeRuntime.shared.active{self.bridge.send("typography",["interfaceFont":interface,"codeFont":code])}
            else{self.applyTypography(interface:interface,code:code);UserDefaults.standard.set(interface,forKey:"interfaceFont");UserDefaults.standard.set(code,forKey:"codeFont")}
        }
        petLibraryMonitor.changed={[weak self] in self?.rescanLocalPets()}
        DispatchQueue.main.async{[weak self] in self?.rescanLocalPets()}
        floatingTasks.onCycle={ [weak self] in self?.cycleFloatingPet($0) }
        floatingTasks.onOpen={ [weak self] task,preview in self?.openFloatingTask(task,preview:preview) }
        floatingTasks.onFinish={ [weak self] task,preview in
            guard let self else{return}
            if preview{self.openFloatingTask(task,preview:true);return}
            if EmbeddedCodeRuntime.shared.active{return}
            if self.miniOpen,self.latestState?.threadId == task.id{return}
            self.floatingReviews.append(task);DispatchQueue.main.asyncAfter(deadline:.now()+0.18){[weak self] in self?.reviewNextFloatingTask()}
        }
        floatingTasks.onChange={ [weak self] in guard let self else{return};if !self.floatingTasks.enabled{self.floatingReviews=[];self.floatingFocus=nil};if let state=self.latestState{self.floatingTasks.update(state.activeTasks,finished:[],pets:self.taskPets)} }
        dictation.onText={ [weak self] text in guard let self,self.dictating else{return};self.setPrompt(self.dictationPrefix+text,dictation:true) };dictation.onLevels={ [weak self] levels in self?.indicator.setExternalLevels(levels) }
        dictation.onError={ [weak self] error in guard let self else{return};self.cancelShortcutInput();self.miniListening=false;self.pinned=true;self.setOpen(true,mini:true);self.indicator.showScene("failure");self.miniUI.setCodexMessage(error);self.syncMotion(force:true) }
        buildMenu(); applyAccentSide(); placePanel(); play(); indicator.hide()
        miniUI.onProject={ [weak self] path in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("project",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onChat={ [weak self] id in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send(id.hasPrefix("child:") ? "child_open":"chat",["id":id,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNew={ [weak self] in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("new",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNewInProject={ [weak self] in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("new_project",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onFolder={ [weak self] path in guard let self else{return};self.bridge.send("folder",["path":path,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onModel={ [weak self] model in self?.bridge.send("settings",["model":model]) }
        miniUI.onPermissions={ [weak self] access,approvals in var fields:[String:Any]=[:];if let access{fields["access"]=access};if let approvals{fields["approvals"]=approvals};self?.bridge.send("workspace_permissions",fields) }
        miniUI.onUsageReset={ [weak self] id in self?.bridge.send("usage_reset",["creditId":id]) }
        miniUI.onOrchestrator={ [weak self] in self?.bridge.send("orchestrator") }
        miniUI.onFast={ [weak self] in self?.bridge.send("fast_mode") }
        miniUI.onUsageRefresh={ [weak self] in self?.bridge.send("usage_refresh") }
        miniUI.onSettings={ [weak self] model, effort in self?.bridge.send("settings",["model":model,"effort":effort]) }
        miniUI.onNavigate={ [weak self] direction in guard let self else{return};self.cancelShortcutHold();self.cancelShortcutInput();self.bridge.send("navigate",["direction":direction,"requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onContextMode={ [weak self] lightweight in self?.contextLightweight=lightweight }
        miniUI.onSend={ [weak self] in self?.sendPrompt() }
        miniUI.onVoice={ [weak self] in self?.toggleChatVoice() }
        miniUI.onPendingTimeout={ [weak self] id in guard let self,self.expandedChatRequest?.id == id else{return};self.finishExpandedChatLoading() }
        miniUI.onFocusComposer={ [weak self] in guard let self else{return};self.panel.makeKey();self.panel.makeFirstResponder(self.composerField) }
        miniUI.onFileActions={ [weak self] url in self?.showFileActions(url) }
        miniUI.onFileAction={ [weak self] action,url in self?.fileActions.perform(action,url:url) }
        fileActions.onPerformed={ [weak self] action,url in
            let stamp=FileActionSymbols.confirm(action,url:url)
            self?.animateFileAction(action,url:url,stamp:stamp)
        }
        fileViewer.onFileActions={ [weak self] url in self?.showFileActions(url) }
        shortcutPreparation.onCommand={ [weak self] action,fields in self?.bridge.send(action,fields) }
        shortcutPreparation.capture=contextCapture ?? { lightweight,completion in
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
            else {self.failCompactVoice(error)}
        }
        voicePrompt.capture=shortcutPreparation.capture
        voicePrompt.onLevels={ [weak self] levels in
            guard let self,self.compactVoiceMode,self.voicePrompt.phase == .recording else{return}
            self.indicator.setExternalLevels(levels)
        }
        voicePrompt.onChange={ [weak self] in self?.renderCompactVoice() }
        voicePrompt.onArmed={ [weak self] in self?.armVoiceShortcutSurface() }
        voicePrompt.onDiscard={ [weak self] in self?.discardRecordedVoice() }
        voicePrompt.onError={ [weak self] in self?.failCompactVoice($0) }
        voicePrompt.onReady={ [weak self] action,audio,context in
            guard let self,self.compactVoiceMode else {
                if let path=context?.imagePath{try? FileManager.default.removeItem(atPath:path)};return
            }
            self.pendingVoiceStart=(action,audio,context,self.voiceCaptureSource)
            self.armVoiceShortcutSurface()
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
            else if let state=self.latestState{self.showActivity(self.displayActivity(state))}
        }
        if let mediaHost=panel?.contentView{chatVoice.attachMedia(to:mediaHost)}
        bridge.onVoiceSDP={ [weak self] in self?.chatVoice.receiveSDP($0,$1) }
        miniUI.onApprove={ [weak self] in self?.answerApproval("approve") }
        miniUI.onReject={ [weak self] in self?.answerApproval("reject") }
        miniUI.onApprovalStop={ [weak self] in self?.answerApproval("approval_stop") }
        miniUI.onReview={ [weak self] in guard let self,let state=self.latestState,let request=CompactApprovalRequest(state:state) else{return};self.presentCompactApproval(request);self.approvalUI.setReviewExpanded(true,pinned:true) }
        miniUI.onInterrupt={ [weak self] in guard let self else{return};self.chatVoice.stopSpeaking();self.speech.cancelAudio();self.bridge.send("interrupt",["requestId":self.miniUI.pendingRequestID ?? ""]) }
        miniUI.onNinja={ [weak self] field,value in self?.bridge.send("ninja_select",["field":field,"value":value]) }
        miniUI.onAgent={ [weak self] agent,request in self?.bridge.send("agent",["agent":agent,"requestId":request]) }
        miniUI.onClose={ [weak self] in self?.dismissNotchView() }
        miniUI.onFilesVisibleChanged={ [weak self] visible in self?.indicator.setFileOpen(visible) }
        miniUI.onImagesChanged={ [weak self] images in self?.queueImages(images) }
        EmbeddedCodeRuntime.shared.onLevels={ [weak self] levels in
            guard let self,self.latestState?.workspaceVoiceActive == true,["listening","speaking"].contains(self.latestState?.workspaceVoiceScene ?? ""),!self.dictating else{return}
            self.indicator.setExternalLevels(levels)
        }
        approvalUI.onCycle={[weak self] in self?.bridge.send("approval_next",[:])}
        approvalUI.onPreferredHeightChange={ [weak self] in guard let self,self.approvalPopout else{return};self.setOpen(true) }
        approvalUI.onReviewAction={ [weak self] action,fields,request in
            guard let self,let state=self.latestState,request.matches(state),fields["threadId"] as? String==request.review?.owner,fields["approvalRequestId"] as? String==request.review?.token else{return}
            self.bridge.send(action,fields)
        }
        bridge.draftBaseProvider={ [weak self] in self?.unifiedDraftBase ?? "" }
        bridge.draftProvider={ [weak self] in self?.composerField?.stringValue ?? "" }
        bridge.onState={ [weak self] state in self?.receive(state) }
        bridge.onError={ [weak self] error in guard let self else{return};self.pendingSlashID=nil;self.pendingSlashPicker=nil;self.slashPicker.hide();self.miniUI.dismissComposerPickers();if self.compactVoiceMode{self.failCompactVoice(error);return};self.chatVoice.stop();self.sounds.playScene("failure");self.earlierHistoryRequest=nil;self.historyControl.update(hasMore:false,loading:false);self.cancelShortcutInput();self.pendingAutoSend=false;self.pendingAttachmentSend=false;self.attachmentRequest=nil;self.sendingPrompt=nil;self.sendingPromptRequestID=nil;self.pendingFilesRequestID=nil;self.indicator.setFileLoading(false);self.finishExpandedChatLoading();if self.miniOpen{self.pinned=EmbeddedCodeRuntime.shared.active;self.compactWhileWorking=false;self.setOpen(EmbeddedCodeRuntime.shared.active,mini:EmbeddedCodeRuntime.shared.active)};self.miniUI.cancelPending();self.miniUI.setCodexMessage(error);self.syncMotion(force:true) }
        speech.onModels={ [weak self] models in self?.updateVoiceMenu(models) }
        speech.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onStatus={ [weak self] status in self?.featureStatusItem.title=status }
        media.onChange={ [weak self] state in self?.receiveMedia(state) }
        media.onLevels={ [weak self] values in guard let self,self.mediaVisible else{return};self.indicator.setExternalLevels(values) }
        speech.discover();media.configure(youtube:youtubeMedia,spotify:spotifyMedia)
        if CommandLine.arguments.contains("--preview-task-pets"){DispatchQueue.main.async{[weak self] in self?.floatingTasks.previewPets()}}
        else if CommandLine.arguments.contains("--preview-approval"){DispatchQueue.main.async{[weak self] in self?.previewCompactApproval()}}
        else{bridge.start();if EmbeddedCodeRuntime.shared.active{EmbeddedCodeRuntime.shared.onError={ [weak self] in self?.miniUI.setCodexMessage($0)}}}
        // Isolated self-tests drive the real handlers directly. Listening to
        // the user's simultaneous keyboard input can arm extra voice/notch
        // shortcuts in the test process and invalidate its surface assertions.
        if !CommandLine.arguments.contains("--unified-smoke-test"){
            localMonitor = NSEvent.addLocalMonitorForEvents(matching:[.flagsChanged,.keyDown]) { [weak self] e in
                if e.type == .flagsChanged {self?.flags(e)}
                else {
                    self?.cancelModifierShortcut()
                    if self?.performApprovalKey(e) == true{return nil}
                    if self?.performNativeKeyboardCommand(e) == true{return nil}
                    if e.keyCode == 53,self?.panel.isKeyWindow == true,self?.open == true {self?.dismissNotchView();return nil}
                }
                return e
            }
            globalMonitor = NSEvent.addGlobalMonitorForEvents(matching:[.flagsChanged,.keyDown]) { [weak self] e in
                if e.type == .flagsChanged {self?.flags(e)}
                else {self?.cancelModifierShortcut();if self?.performApprovalKey(e) == true{return}}
            }
        }
        for name in [NSWindow.didBecomeKeyNotification,NSWindow.didResignKeyNotification,NSApplication.didBecomeActiveNotification,NSApplication.didResignActiveNotification]{NotificationCenter.default.addObserver(self,selector:#selector(scheduleWorkspaceFocusRefresh),name:name,object:nil)}
        NSWorkspace.shared.notificationCenter.addObserver(self,selector:#selector(scheduleWorkspaceFocusRefresh),name:NSWorkspace.didActivateApplicationNotification,object:nil)
        if !CommandLine.arguments.contains("--unified-smoke-test"){
            dragMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDragged,.leftMouseUp]){[weak self] event in DispatchQueue.main.async{self?.globalDrag(event)}}
        }
        NotificationCenter.default.addObserver(self, selector: #selector(placePanel),
            name: NSApplication.didChangeScreenParametersNotification, object: nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(diagnoseMedia),name:NSNotification.Name("com.kai.media.diagnose"),object:nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(codexPreview(_:)),name:NSNotification.Name("com.kai.codex.preview"),object:nil)
        DistributedNotificationCenter.default().addObserver(self,selector:#selector(previewCompactApproval),name:NSNotification.Name("com.kai.approval.preview"),object:nil)
    }

    private func performApprovalKey(_ event:NSEvent)->Bool{
        guard approvalPopout else{return false};return approvalUI.performDecisionKey(event)
    }
    func performNativeKeyboardCommand(_ event:NSEvent)->Bool {
        guard event.type == .keyDown,EmbeddedCodeRuntime.shared.active,miniOpen,panel.isKeyWindow else{return false}
        let flags=event.modifierFlags.intersection([.control,.shift,.command,.option])
        let prefix:String
        switch flags{case [.control]:prefix="ctrl-";case [.control,.shift]:prefix="ctrl-shift-";case [.command]:prefix="cmd-";case [.command,.shift]:prefix="cmd-shift-";default:return false}
        guard let char=event.charactersIgnoringModifiers?.lowercased(),let command=EmbeddedCodeRuntime.shared.keyboardCommands.first(where:{$0.key==prefix+char}) else{return false}
        if command.id != "close_tab",(composerField.currentEditor() as? NSTextView)?.hasMarkedText() == true{return false}
        if flags.contains(.command){
            // A Command shortcut is not a bare Command tap. Repeated Cmd-W
            // must never also trigger the double-Command surface dismissal.
            cancelModifierShortcut()
        }
        if command.id == "composer"{panel.makeFirstResponder(composerField)}
        else if miniUI.performKeyboardCommand(command.id){}
        else {bridge.send("ui_command",["command":command.id]);if ["terminal","files","changes","children","keyboard","projects","search","open_project"].contains(command.id){revealWorkspace()}}
        return true
    }

    func applicationWillTerminate(_ notification:Notification){petLibraryMonitor.stop();NSWorkspace.shared.notificationCenter.removeObserver(self);NotificationCenter.default.removeObserver(self);hangDiagnostics.stop();compactPetCycle?.cancel();cancelWorkspaceHold();quickMorph.cancel();workspaceMorph.cancel();workspaceStatusMorph.cancel();workspaceStatusRetire?.cancel();passiveRetire?.cancel();textReveal.finish();petAppIcon.stop();floatingTasks.stop();imageEncodingWork?.cancel();voicePrompt.cancel();voiceLaunchTimeout?.cancel();shortcutPreparation.cancel();cancelShortcutHold();mediaReleaseWork?.cancel();chatVoice.stop();dictation.cancel();speech.stop();media.stop();artworkTask?.cancel();bridge.stop();attachmentFiles.forEach{try? FileManager.default.removeItem(at:$0)}}
    @objc private func codexPreview(_ note:Notification){guard let path=note.userInfo?["path"] as? String else{return};pinned=true;setOpen(true,mini:true,userInitiated:true);miniUI.previewFile(URL(fileURLWithPath:(path as NSString).expandingTildeInPath))}

    private func applyMenuBarIdentity(_ mark:MenuBarMark){
        guard let button=status?.button else{return}
        button.image=MenuBarIdentityMenu.image(mark)
        button.title=button.image == nil ? "海":""
        button.imagePosition=button.image == nil ? .noImage:.imageOnly
        button.toolTip=mark.kind == "kai" ? "Kai Unified":mark.name+" · Kai Unified"
        button.setAccessibilityLabel(button.toolTip)
    }

    private func buildMenu() {
        status = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        menuBarIdentity.onChange={ [weak self] in self?.applyMenuBarIdentity($0) }
        applyMenuBarIdentity(menuBarIdentity.mark)
        let menu = NSMenu()
        if EmbeddedCodeRuntime.shared.active {
            let workspace=menu.addItem(withTitle:"Open Code workspace",action:#selector(expandWorkspace),keyEquivalent:"");workspace.target=self
            let compact=menu.addItem(withTitle:"Bring workspace to notch",action:#selector(collapseWorkspace),keyEquivalent:"");compact.target=self
            let settings=menu.addItem(withTitle:"Workspace settings",action:#selector(workspaceSettings),keyEquivalent:"");settings.target=self
            unifiedMenuItems=[workspace,compact,settings]
            menu.addItem(.separator())
        }
        let identity=menu.addItem(withTitle:"Menu bar icon",action:nil,keyEquivalent:"");identity.submenu=menuBarIdentity.menu
        let defaultModelItem=menu.addItem(withTitle:"Default model",action:nil,keyEquivalent:"")
        defaultModelItem.submenu=defaultModelMenu.menu
        showItem = menu.addItem(withTitle: "show notch", action: #selector(toggleFromMenu), keyEquivalent: "")
        showItem.target = self
        chatVoiceMenuItem=menu.addItem(withTitle:"start voice in selected chat",action:#selector(toggleChatVoice),keyEquivalent:"")
        let realtimeVoices=menu.addItem(withTitle:"Realtime voice",action:nil,keyEquivalent:"")
        realtimeVoices.submenu=realtimeVoiceMenu.menu
        realtimeVoiceMenu.onSelection={ [weak self] choice in if EmbeddedCodeRuntime.shared.active{self?.bridge.send("workspace_voice_choice",["voice":choice.rawValue])}; self?.featureStatusItem?.title="Realtime voice: \(choice.title) · next conversation" }
        voiceApprovalMenuItem=menu.addItem(withTitle:"review voice approval",action:#selector(reviewCompactVoiceApproval),keyEquivalent:"")
        voiceApprovalMenuItem?.isHidden=true
        let approvalPreview=menu.addItem(withTitle:"preview approval popout",action:#selector(previewCompactApproval),keyEquivalent:"")
        chatVoiceMenuItem?.target=self
        experimentalModeItem=menu.addItem(withTitle:"experimental web mode",action:#selector(toggleExperimentalWebMode),keyEquivalent:"")
        experimentalModeItem.target=self
        mathModeItem=menu.addItem(withTitle:"render equations",action:#selector(toggleMathRendering),keyEquivalent:"")
        mathModeItem.target=self
        textMorphItem=menu.addItem(withTitle:"Morphing text",action:#selector(toggleTextMorph),keyEquivalent:"")
        textMorphItem?.target=self
        soundsItem=menu.addItem(withTitle:"sounds",action:#selector(toggleSounds),keyEquivalent:"")
        soundsItem.target=self
        codeConnectionItem=menu.addItem(withTitle:EmbeddedCodeRuntime.shared.active ? "Code runtime · in this app":"Connect to Code",action:EmbeddedCodeRuntime.shared.active ? nil:#selector(toggleCodeConnection),keyEquivalent:"")
        glassItem=menu.addItem(withTitle:"Frosted glass at bottom",action:#selector(toggleBottomGlass),keyEquivalent:"")
        if EmbeddedCodeRuntime.shared.active{codeWallpaperItem=menu.addItem(withTitle:"Code wallpaper at bottom",action:#selector(toggleCodeWallpaper),keyEquivalent:"");codeWallpaperItem?.target=self;codeWallpaperItem?.toolTip="Reuse Code’s wallpaper, effect, and visibility. Adjust Fade height under Bottom appearance."}
        let glassControls=menu.addItem(withTitle:"Bottom appearance",action:nil,keyEquivalent:"")
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
        let scanPets=petMenu.addItem(withTitle:"Rescan local pets",action:#selector(rescanLocalPets),keyEquivalent:"");scanPets.target=self
        let localPets=petMenu.addItem(withTitle:"Add pet folder…",action:#selector(choosePetFolder),keyEquivalent:"");localPets.target=self
        let importPets=petMenu.addItem(withTitle:"Import pet pack…",action:#selector(importPetPack),keyEquivalent:"");importPets.target=self
        populatePetMenu(petMenu,pets:catalog.pets)
        petMenu.addItem(.separator())
        let done=petMenu.addItem(withTitle:"done",action:#selector(closePetPicker),keyEquivalent:"")
        done.target=self
        workspacePetMenu=petMenu
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
        expandedDictationItem=menu.addItem(withTitle:"Expanded notch: ⌘ Command dictation only",action:#selector(toggleExpandedDictation),keyEquivalent:"")
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
        NotchMenuOrganizer.organize(menu,quick:unifiedMenuItems+items([showItem,chatVoiceMenuItem,voiceApprovalMenuItem,identity]),sections:[
            .init(title:"Chats & Models",items:[defaultModelItem]),
            .init(title:"Shortcuts & Input",items:items([shortcutItem,timingItem,expandedDictationItem,autoSendItem,contextItem,compactSendItem])),
            .init(title:"Voice & Audio",items:items([realtimeVoices,speechItem,voices,rescanVoicesItem,stopSpeakingItem,soundsItem])),
            .init(title:"Appearance",items:items([typographyMenu.item,petRoot,floatingTasksItem(),sizeItem,motionRoot,indicatorRoot,accentRoot,sizingItem,glassItem,codeWallpaperItem,glassControls,mathModeItem,textMorphItem])),
            .init(title:"Media",items:items([youtubeItem,spotifyItem,mediaSizeItem,mediaPermissionsItem])),
            .init(title:"Advanced & Connections",items:items([codeConnectionItem,experimentalModeItem,morphItem,approvalPreview]))
        ],footer:items([featureStatusItem,quit]))
        statusContextMenu=menu;status.menu=nil
        status.button?.target=self;status.button?.action=#selector(menuBarClicked(_:));status.button?.sendAction(on:[.leftMouseUp,.rightMouseUp])
        refreshChecks()
    }

    @objc private func menuBarClicked(_ sender:Any?){
        guard let button=status?.button else{return}
        if let event=NSApp.currentEvent,event.type == .rightMouseUp || event.modifierFlags.contains(.option){if let menu=statusContextMenu{NSMenu.popUpContextMenu(menu,with:event,for:button)};return}
        toggleMenuBarPanel()
    }
    func toggleMenuBarPanel(){
        guard let button=status?.button else{return}
        cancelModifierShortcut()
        guard EmbeddedCodeRuntime.shared.active else{toggleFromMenu();return}
        if quickChatVisible,menuQuickAnchor != nil{EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");return}
        guard let host=button.window else{return};let anchor=host.convertToScreen(button.convert(button.bounds,to:nil));menuQuickAnchor=anchor
        let fromWorkspace=EmbeddedCodeRuntime.shared.workspaceWindow?.isVisible == true
        quickPrepared=(fromWorkspace,fromWorkspace ? EmbeddedCodeRuntime.shared.workspaceWindow!.frame:anchor,.hidden)
        let mark=menuBarIdentity.mark
        bridge.send("menu_bar_open",["kind":mark.kind,"id":mark.id])
    }

    @objc private func placePanel(){layoutPanel(present:true)}
    private func layoutPanel(present:Bool,approvalDuration:TimeInterval=0) {
        defer{floatingTasks.refresh()}
        let previousMaskSize=mask.bounds.size,previousDestination=mask.path
        let notchedScreen = NSScreen.screens.first(where: { $0.auxiliaryTopLeftArea != nil && $0.auxiliaryTopRightArea != nil })
        guard let screen = notchedScreen ?? NSScreen.main ?? NSScreen.screens.first else { panel?.orderOut(nil); return }
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        islandMode = notchedScreen == nil
        let center: CGFloat
        if let left = screen.auxiliaryTopLeftArea, let right = screen.auxiliaryTopRightArea {
            notchHeight = screen.safeAreaInsets.top + 1
            let gapCenter = (left.maxX + right.minX) / 2
            center = abs(gapCenter - screen.frame.midX) <= 1 ? screen.frame.midX : gapCenter
            notchWidth = 2 * max(center - left.maxX, right.minX - center)
        } else {
            center = screen.frame.midX
            notchHeight = 44
            notchWidth = 80
        }
        let baseline=CGSize(width:max(islandMode ? 340:0,notchWidth+160),height:320)
        let saved=CGSize(width:UserDefaults.standard.object(forKey:"notchDefaultWidth") as? Double ?? Double(baseline.width),height:UserDefaults.standard.object(forKey:"notchDefaultHeight") as? Double ?? 320)
        expandedSize=NotchSizing.constrained(expandedSize ?? saved,baseline:baseline,screen:screen.frame.size)
        canvasWidth=ceil(max(notchWidth+260,expandedSize!.width+100));canvasHeight=expandedSize!.height
        let defaultSize=NotchSizing.constrained(saved,baseline:baseline,screen:screen.frame.size)
        defaultWidthSlider?.minValue=Double(baseline.width);defaultWidthSlider?.maxValue=Double(max(baseline.width,screen.frame.width-80));defaultWidthSlider?.doubleValue=Double(defaultSize.width)
        defaultHeightSlider?.maxValue=Double(max(320,screen.frame.height-80));defaultHeightSlider?.doubleValue=Double(defaultSize.height)
        sizeLabels["notchDefaultWidth"]?.stringValue="Width · \(Int(defaultSize.width)) pt"
        sizeLabels["notchDefaultHeight"]?.stringValue="Height · \(Int(defaultSize.height)) pt"
        let rect = NSRect(x: center - canvasWidth / 2, y: screen.frame.maxY - surfaceTopInset - canvasHeight,
                          width: canvasWidth, height: canvasHeight)
        if panel == nil {
            panel = NotchPanel(contentRect: rect, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.backgroundColor = .clear; panel.isOpaque = false; panel.hasShadow = false; panel.ignoresMouseEvents = true
            panel.level = .statusBar; panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary, .stationary]
            let host=NotchView(frame:CGRect(origin:.zero,size:rect.size));host.click={ [weak self] point,count in self?.handleClick(point,count:count) };host.scroll={ [weak self] point,delta in self?.handleScroll(point,delta) };host.beginScrollDrag={ [weak self] point in guard let self,self.miniOpen,!self.petGalleryVisible else{return false};return self.miniUI.beginSidebarScroll(at:self.miniPoint(point)) };host.scrollDrag={ [weak self] point in guard let self else{return};self.miniUI.dragSidebarScroll(to:self.miniPoint(point)) };host.endScrollDrag={ [weak self] in self?.miniUI.endSidebarScroll() };host.hover={ [weak self] point in self?.handleHover(point) };host.dragEntered={ [weak self] in guard self?.experimentalWebMode != true else{return};self?.handleImageEntered() };host.imageDropped={ [weak self] images in guard self?.experimentalWebMode != true else{return};self?.handleImageDrop(images) };panel.contentView=host;host.wantsLayer=true;panel.acceptsMouseMovedEvents=true
            host.addSubview(glassBackground)
            host.addSubview(workspaceBackground)
            scalableUI.wantsLayer=true;host.addSubview(scalableUI)
            composerField=ComposerField();composerField.isBordered=false;composerField.drawsBackground=false;composerField.focusRingType = .none;composerField.font=AppTypography.ui(ofSize:10);composerField.textColor=NSColor.white.withAlphaComponent(0.88);composerField.delegate=self;composerField.isHidden=true;composerField.imagesEntered={ [weak self] in self?.handleImageEntered() };composerField.imagesDropped={ [weak self] images in self?.handleImageDrop(images) };scalableUI.addSubview(composerField)
            miniUI.composerFrameChanged={ [weak self] frame,animated in self?.layoutComposer(frame,animated:animated) }
            let scroll=ConversationScrollView(),text=ConversationTextView()
            historyControl.onLatest={[weak self] in guard let self else{return};self.earlierHistoryRequest=nil;self.bridge.send("history_latest",[:])}
            historyControl.onLoad={ [weak self] in self?.loadEarlierHistory() }
            historyControl.frame=CGRect(x:0,y:0,width:scroll.contentSize.width,height:24)
            historyControl.autoresizingMask=[.width];historyControl.isHidden=true;text.addSubview(historyControl)
            scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.scrollerStyle = .overlay;scroll.borderType = .noBorder;scroll.isHidden=true
            text.linkTextAttributes=[.cursor:NSCursor.pointingHand];text.drawsBackground=false;text.isEditable=false;text.isSelectable=true;text.textColor = .white;text.font=AppTypography.ui(ofSize:9);text.textContainerInset=NSSize(width:3,height:24);text.isVerticallyResizable=true;text.textContainer?.widthTracksTextView=true;text.autoresizingMask=[.width];text.delegate=self
            conversationRenderer.willChange={ [weak self] in self?.textReveal.finish() }
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
        if panel.frame != rect{panel.setFrame(rect,display:true)}; mask.frame = CGRect(origin: .zero, size: rect.size); notchContent.frame = mask.frame
        // The full canvas must never intercept the desktop while compact.
        panel.ignoresMouseEvents = !NotchInputRegion.mainAcceptsInput(expanded:miniOpen,web:experimentalWebMode)
        approvalUI.maximumReviewHeight=max(80,min(480,screen.frame.maxY-screen.visibleFrame.minY-notchHeight-80))
        let dropRect=NotchInputRegion.compactFrame(screen:screen.frame.offsetBy(dx:0,dy:-surfaceTopInset),center:center,width:expandedWidth,height:approvalPopout ? popoutHeight:notchHeight)
        if dropPanel == nil { dropPanel=NotchPanel(contentRect:dropRect,styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false);dropPanel.backgroundColor = .clear;dropPanel.isOpaque=false;dropPanel.level = .popUpMenu;dropPanel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary,.stationary];let drop=NotchView(frame:NSRect(origin:.zero,size:dropRect.size));drop.autoresizingMask=[.width,.height];drop.click={ [weak self] _,_ in guard let self else{return};if self.compactVoiceMode{self.handleCompactVoiceClick();return};guard self.compactWhileWorking || self.islandMode else{return};self.expandToRunningChat() };drop.dragEntered={ [weak self] in self?.handleImageEntered() };drop.imageDropped={ [weak self] images in self?.handleImageDrop(images) };dropPanel.contentView=drop }
        if approvalUI.superview == nil {dropPanel.contentView?.addSubview(approvalUI);approvalUI.onDecision={ [weak self] action,request in self?.decidePopout(action,request:request) }}
        let approvalFrame=CGRect(x:16,y:12,width:dropRect.width-32,height:approvalUI.preferredHeight)
        if approvalDuration>0,approvalPopout,dropPanel.isVisible{
            NSAnimationContext.runAnimationGroup{context in
                context.duration=approvalDuration;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.85,0.25,1);context.allowsImplicitAnimation=true
                self.dropPanel.animator().setFrame(dropRect,display:true);self.approvalUI.animator().frame=approvalFrame;self.approvalUI.layoutSubtreeIfNeeded()
            }
        }else{dropPanel.setFrame(dropRect,display:true);approvalUI.frame=approvalFrame}
        approvalUI.isHidden = !approvalPopout
        let compactInput=(islandMode && open && !miniOpen && !experimentalWebMode) || approvalPopout || NotchInputRegion.compactAcceptsInput(open:open,expanded:miniOpen,web:experimentalWebMode,working:compactWhileWorking,voice:compactVoiceMode)
        dropPanel.ignoresMouseEvents = !compactInput
        if present{compactInput && showsNotchSurface ? dropPanel.orderFrontRegardless():dropPanel.orderOut(nil)}
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
        let accentY=approvalPopout ? canvasHeight-popoutHeight:(miniOpen ? 0:topBandY)
        let shoulder:CGFloat=7,glowWidth:CGFloat=115, glowHeight:CGFloat=82
        indicator.compactBandHeight=notchHeight
        indicator.leftAccentLayer.frame = CGRect(x:px(kaiLeft-shoulder),y:px(accentY),width:glowWidth,height:glowHeight)
        indicator.accentLayer.frame = CGRect(x:px(kaiRight+shoulder-glowWidth),y:px(accentY),width:glowWidth,height:glowHeight); indicator.layout()
        let content=CGSize(width:px(expanded-16),height:px(topBandY-24))
        let sizing=NotchSizing.layout(content:content,baseline:CGSize(width:notchWidth+144,height:320-notchHeight-24))
        scalableUI.frame=CGRect(x:px(kaiLeft+8),y:16,width:content.width,height:content.height)
        scalableUI.bounds=CGRect(origin:.zero,size:sizing.logical)
        miniUI.setRenderingScale(scale*sizing.scale)
        miniUI.layout(in:scalableUI.bounds,bottomInset:16/max(0.1,sizing.scale))
        resizeHandles.frame=CGRect(x:kaiLeft+8,y:0,width:expanded-16,height:14);resizeHandles.isHidden = !miniOpen || experimentalWebMode
        resizeHandles.needsDisplay=true;panel.invalidateCursorRects(for:resizeHandles)
        petGallery.layer.frame=miniUI.layer.frame;petGallery.layout(in:petGallery.layer.bounds,selected:petID);NotchSizing.updateTextResolution(petGallery.layer,scale:scale*sizing.scale)
        if experimentalWebMode{ensureBrowser()};browserView?.frame=CGRect(x:px(kaiLeft),y:0,width:px(expanded),height:canvasHeight)
        CATransaction.begin(); CATransaction.setDisableActions(true)
        let destination=path(width:(open || experimentalWebMode) ? expanded:notchWidth,height:(open || experimentalWebMode) ? popoutHeight:notchHeight)
        if previousMaskSize != mask.bounds.size || previousDestination != destination{mask.removeAnimation(forKey:"width")}
        mask.path=destination
        notchContent.opacity = experimentalWebMode ? 0:1;miniUI.layer.isHidden=experimentalWebMode;scalableUI.isHidden=experimentalWebMode
        browserView?.isHidden = !experimentalWebMode
        CATransaction.commit()
        if present{
            if (open || experimentalWebMode) && showsNotchSurface{panel.orderFrontRegardless()}else if !showsNotchSurface{panel.orderOut(nil);dropPanel?.orderOut(nil);syncWorkspacePet()}
        }
        updateNinjaWallpaper()
    }

    private func path(width: CGFloat, height: CGFloat) -> CGPath {
        NotchSizing.path(canvas:CGSize(width:canvasWidth,height:canvasHeight),width:width,height:height,radius:islandMode ? islandRadius:(glassEnabled && miniOpen ? CGFloat(GlassPreferences(defaults:.standard).radius):14),floating:islandMode)
    }

    func flags(_ event: NSEvent) {
        cancelSizeTest()
        let down=event.modifierFlags.contains(.command),option=event.modifierFlags.contains(.option),control=event.modifierFlags.contains(.control),function=event.modifierFlags.contains(.function)
        let miniChord=option && control && !down && !function && !event.modifierFlags.contains(.shift),wasMiniChord=optionDown && controlDown
        defer {commandDown=down;optionDown=option;controlDown=control;functionDown=function}
        let uptime=ProcessInfo.processInfo.systemUptime
        let now=event.timestamp>0 && event.timestamp<=uptime ? event.timestamp:uptime
        // Key equivalents in another app may bypass our AppKit key monitor.
        // The system counter still invalidates that entire modifier hold.
        if let count=shortcutKeyCount,count != keyDownCount(){cancelModifierShortcut()}
        // A release can reach AppKit before an overdue main-queue timer. Honor
        // the actual key duration once, before release cleanup cancels the hold.
        if event.modifierFlags.intersection([.command,.option,.control,.function,.shift]).isEmpty {
            completeShortcutHold(at:now)
        }
        for (trigger,pressed) in [(DictationTrigger.command,down),(.option,option),(.function,function)] where !pressed {
            voicePrompt.release(trigger)
            shortcutPreparation.release(trigger)
            if dictating,dictationTrigger == trigger {finishDictation()}
        }
        if commandChordActive{commandDismissGesture.cancel();if !down{commandChordActive=false};return}
        if control && !controlDown && !option {
            if open && now-lastControlTap < 0.36 {lastControlTap=0;cancelShortcutHold();holdWork?.cancel();dismissNotchView();return}
            lastControlTap=now
        }
        // One dismissal path for mini chat, Quick Chat, expansion in flight,
        // and the large workspace. Bare taps must never arm a voice hold.
        if handleCommandDismiss(down:down,bare:!option && !control && !function && !event.modifierFlags.contains(.shift),at:now){return}
        let change=shortcutGesture.update(command:down,option:option,control:control,function:function,shift:event.modifierFlags.contains(.shift))
        if change != .unchanged {
            cancelShortcutHold()
            if case .begin(let trigger)=change {
                shortcutKeyCount=keyDownCount()
                holdWork?.cancel();holdWork=nil
                let expanded=miniOpen
                guard let action=shortcutPreferences.action(for:trigger,expanded:expanded) else{return}
                if action.isVoice,!compactVoiceMode,latestState?.workspaceVoiceActive != true{
                    startVoicePromptShortcut(action,trigger:trigger);return
                }
                let generation=shortcutGeneration
                pendingShortcutHold=(trigger,action,expanded,now+shortcutPreferences.holdDuration(for:trigger),generation)
                // Dictation and ending an existing voice call still use the
                // configured hold threshold. A new held voice prompt records
                // immediately, retaining words spoken during that threshold.
                let work=DispatchWorkItem { [weak self] in
                    guard let self,self.shortcutGeneration == generation else{return}
                    let flags=self.modifierFlags()
                    guard ShortcutPreferences.heldTrigger(command:flags.contains(.command),option:flags.contains(.option),control:flags.contains(.control),function:flags.contains(.function),shift:flags.contains(.shift)) == trigger else{return}
                    self.completeShortcutHold(at:ProcessInfo.processInfo.systemUptime)
                }
                shortcutWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+shortcutPreferences.holdDuration(for:trigger),execute:work)
            } else {
                cancelVoicePromptForKeypress()
                for trigger in DictationTrigger.allCases {shortcutPreparation.release(trigger)}
            }
        }
        if miniChord != wasMiniChord {
            holdWork?.cancel();holdWork=nil;cancelWorkspaceHold()
            if miniChord {
                if EmbeddedCodeRuntime.shared.active {
                    workspaceHold.begin(at:now,workspace:workspaceMorph.expanded && workspaceIsForeground,overlay:workspaceNotchOverlay && miniOpen)
                    for delay in [WorkspaceHoldGesture.miniDelay,WorkspaceHoldGesture.workspaceDelay]{
                        let job=DispatchWorkItem{[weak self] in
                            guard let self,self.optionDown,self.controlDown else{return}
                            guard let action=self.workspaceHold.advance(at:ProcessInfo.processInfo.systemUptime) else{return}
                            switch action{case .mini:if !self.miniOpen{self.expandToRunningChat()};case .workspace:self.expandWorkspace();case .fold:self.collapseWorkspace()}
                        };workspaceHoldJobs.append(job);DispatchQueue.main.asyncAfter(deadline:.now()+delay,execute:job)
                    }
                }else{
                    let work=DispatchWorkItem{[weak self] in guard let self,self.optionDown,self.controlDown else{return};self.expandToRunningChat()}
                    holdWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+1,execute:work)
                }
            }
        }
    }
    private func handleCommandDismiss(down:Bool,bare:Bool,at time:TimeInterval)->Bool{
        guard commandDismissGesture.update(down:down,eligible:bare && (open || (workspaceMorph.expanded && workspaceIsForeground) || workspaceMorph.transitioning || quickChatVisible),at:time) else{return false}
        cancelShortcutHold();cancelWorkspaceHold();holdWork?.cancel();dismissNotchView();return true
    }
    func probeMenuBarClick(){guard CommandLine.arguments.contains("--unified-smoke-test") else{return};menuBarClicked(nil)}
    func probeDoubleCommand(){
        guard CommandLine.arguments.contains("--unified-smoke-test") else{return}
        if workspaceMorph.expanded,let window=EmbeddedCodeRuntime.shared.workspaceWindow{
            let f=window.frame
            window.setFrame(CGRect(x:f.minX+80,y:f.minY+70,width:max(600,f.width-160),height:max(420,f.height-140)),display:true)
        }
        commandDismissGesture.cancel();let time=ProcessInfo.processInfo.systemUptime
        _=handleCommandDismiss(down:true,bare:true,at:time)
        _=handleCommandDismiss(down:false,bare:true,at:time+0.05)
        precondition(handleCommandDismiss(down:true,bare:true,at:time+0.1),"Double Command did not reach surface dismissal")
        _=handleCommandDismiss(down:false,bare:true,at:time+0.15)
    }
    private func cancelWorkspaceHold(){workspaceHold.cancel();workspaceHoldJobs.forEach{$0.cancel()};workspaceHoldJobs=[]}

    private func startVoicePromptShortcut(_ action:ShortcutAction,trigger:DictationTrigger){
        if EmbeddedCodeRuntime.shared.active,latestState?.workspaceVoiceActive == true{endCompactVoice();return}
        if compactVoiceMode && (chatVoice.active || voicePrompt.active || shortcutPreparation.busy || pendingVoiceStart != nil) {endCompactVoice();return}
        guard !dictating,!finalizingDictation,!shortcutPreparation.busy,sendingPrompt == nil,expandedChatRequest == nil else{miniUI.setCodexMessage("Finish the current input before starting voice");return}
        if action.createsChat,!(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty || queuedImageCount>0{miniUI.setCodexMessage("Send or clear your draft before starting voice in a new chat");return}
        let target=ShortcutPreparation.target(selected:latestState?.threadId,running:latestState.flatMap{RunningChatSelection.target(in:$0)?.id},lastOpened:lastOpenedChatID)
        if !action.createsChat {
            guard let target,EmbeddedCodeRuntime.shared.active || !target.hasPrefix("code:") else{miniUI.setCodexMessage("Select a Codex chat or assign voice in a new chat");return}
            if action.capturesContext,latestState?.activeTurnId != nil{miniUI.setCodexMessage("Wait for this turn to finish, or use voice in a new chat with appshot");return}
        }
        dictationDestination=nil;voiceCaptureSource=latestState?.threadId
        if experimentalWebMode{toggleExperimentalWebMode()}
        beforeVoiceShortcut=(open,miniOpen,pinned,compactWhileWorking,autoCompact);voiceShortcutArmed=false
        voiceExpanded=false;compactVoiceMode=true;compactVoiceError=nil;compactVoiceIndicator=nil
        autoCompact=false;compactWhileWorking=false;pinned=true
        speech.cancelAudio();dismissMedia(closeNotch:false)
        voicePrompt.begin(action,trigger:trigger,lightweight:contextLightweight,minimumHold:shortcutPreferences.holdDuration(for:trigger))
        guard voicePrompt.active else{return}
        renderCompactVoice()
    }
    func cancelVoicePromptForKeypress(){
        guard voicePrompt.phase == .recording else{return}
        voicePrompt.cancel();discardRecordedVoice()
    }
    private func discardRecordedVoice(){
        if !voiceShortcutArmed,let prior=beforeVoiceShortcut{
            compactVoiceMode=false;compactVoiceIndicator=nil;voiceExpanded=false
            pinned=prior.pinned;compactWhileWorking=prior.working;autoCompact=prior.automatic;setOpen(prior.open,mini:prior.mini)
            currentScene="";if let state=latestState{showActivity(displayActivity(state))}
            beforeVoiceShortcut=nil
        }else{closeNotch()}
    }
    private func armVoiceShortcutSurface(){
        if let count=shortcutKeyCount,count != keyDownCount(){cancelModifierShortcut();return}
        guard compactVoiceMode,!voiceShortcutArmed else{return};voiceShortcutArmed=true
        revealNotchForInput();setOpen(true,mini:false);renderCompactVoice()
    }
    private func prepareRecordedVoice(){
        guard compactVoiceMode,let pending=pendingVoiceStart,!shortcutPreparation.busy,let state=latestState else{return}
        if let source=pending.source,state.threadId != source{failCompactVoice("Chat changed before voice started; hold the shortcut to try again");return}
        let target=ShortcutPreparation.target(selected:state.threadId,running:RunningChatSelection.target(in:state)?.id,lastOpened:lastOpenedChatID)
        shortcutPreparation.start(action:pending.action,trigger:.command,selected:state.threadId,target:target,lightweight:contextLightweight,context:pending.context)
    }
    private func renderCompactVoice(){
        guard compactVoiceMode else{return}
        if EmbeddedCodeRuntime.shared.active,voicePrompt.phase == .off,pendingVoiceStart == nil,!shortcutPreparation.busy {
            if workspaceVoiceRequest != nil{if currentScene != "workspace-voice:connecting"{currentScene="workspace-voice:connecting";indicator.showThinking()};return}
            guard latestState?.workspaceVoiceActive == true else{compactVoiceMode=false;currentScene="";return}
            let scene=latestState?.workspaceVoiceScene ?? "thinking"
            if let levels=latestState?.workspaceVoiceLevels{indicator.setExternalLevels(levels)}
            if currentScene != "workspace-voice:"+scene {
                currentScene="workspace-voice:"+scene
                if ["listening","speaking"].contains(scene){indicator.startListening(useMicrophone:false)}else{indicator.showThinking()}
            }
            return
        }
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
        workspaceVoiceRequest=nil;cancelShortcutInput();pinned=true;compactWhileWorking=false
        setOpen(true,mini:true);miniUI.setCodexMessage(message);featureStatusItem?.title=message
        currentScene="";indicator.showScene("failure")
    }
    private func endCompactVoice(){
        // Ending a call is explicit. Surface dismissal never owns its lifetime.
        compactVoiceMode=false
        if EmbeddedCodeRuntime.shared.active,latestState?.workspaceVoiceActive == true{bridge.send("voice_stop")}
        chatVoice.stop();closeNotch()
    }
    private func handleCompactVoiceClick(){
        expandVoiceChat()
    }
    private func expandVoiceChat(){
        pinned=true;autoCompact=false;compactWhileWorking=false
        setOpen(true,mini:true,userInitiated:true)
    }
    func dismissUnifiedSurface(){dismissNotchView()}
    private func dismissNotchView(){
        if let request=approvalUI.request ?? latestState.flatMap(CompactApprovalRequest.init(state:)){approvalDismissal.dismiss(request)}
        if approvalPopout{nativeReview.dismiss();approvalPopout=false;approvalUI.reset();closeNotch();return}
        cancelWorkspaceHold()
        if workspaceNotchOverlay{
            closeNotch();return
        }
        if quickChatVisible{quickReturn = workspaceNeedsStatus ? .compact:.hidden;EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");return}
        if workspaceMorph.expanded || workspaceMorph.transitioning{dismissWorkspace();return}
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
        revealNotchForInput();pinned=true;setOpen(true,mini:miniOpen);indicator.showThinking()
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
        workspaceVoiceRequest=nil;voicePrompt.cancel();voiceLaunchTimeout?.cancel();voiceLaunchTimeout=nil
        if let path=pendingVoiceStart?.context?.imagePath {attachmentFiles.append(URL(fileURLWithPath:path))}
        pendingVoiceStart=nil
        shortcutPreparation.cancel();pendingAutoSend=false;dictationDestination=nil
        dictationGeneration=UUID();finalizingDictation=false
        if dictating {dictation.cancel();dictating=false;dictationTrigger=nil;sounds.stopListening();exitMiniListening()}
    }

    func expandToRunningChat(){
        prepareNotchOverlay()
        if compactVoiceMode {expandVoiceChat();return}
        // Unified surfaces share Code’s selected session. A status focus on
        // another running agent must never become implicit navigation.
        if EmbeddedCodeRuntime.shared.active{pinned=true;compactWhileWorking=false;setOpen(true,mini:true,userInitiated:true);return}
        let preferred=latestState?.activeTasks.first(where:{$0.id == floatingFocus})
        let target=preferred.map{(id:$0.id,name:$0.name)} ?? latestState.flatMap{RunningChatSelection.target(in:$0)}
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

    private func floatingTasksItem()->NSMenuItem{let item=NSMenuItem(title:"Floating task pets",action:nil,keyEquivalent:"");item.submenu=floatingTasks.menu;return item}
    private func displayActivity(_ state:KaiState)->KaiActivity{
        if !miniOpen,!state.approval,let task=state.activeTasks.first(where:{$0.id == (floatingFocus ?? compactPetCycleID)}){return KaiActivity(scene:task.scene,text:task.text)}
        return miniOpen ? state.viewedActivity:state.activity
    }
    private func cycleFloatingPet(_ task:KaiActiveTask){
        let options=TaskPetAssignments.cycleChoices(for:task.id,assignments:taskPets,taskIDs:petAssignmentTaskIDs(for:task.id),available:petSelection.automaticIDs(catalog.pets.map(\.id)))
        guard let id=options.randomElement() else{return}
        taskPets[task.id]=id;explicitTaskPets.insert(task.id);floatingFocus=task.id
        reconcileTaskPets(priority:task.id)
        if !miniOpen || latestState?.threadId == task.id{petID=id;focusedTaskID=task.id;currentScene="";play();if let state=latestState{showActivity(displayActivity(state))}}
        floatingTasks.refresh(pets:taskPets)
        syncInlinePets()
    }
    private func openFloatingTask(_ task:KaiActiveTask,preview:Bool){
        if task.id.hasPrefix("child:"){bridge.send("child_open",["id":task.id]);return}
        if preview{autoCompact=false;compactWhileWorking=false;pinned=true;setOpen(true,mini:true);miniUI.setCodexMessage(task.text);return}
        guard expandedChatRequest == nil,!dictating,!finalizingDictation,!shortcutPreparation.busy,!chatVoice.active,
              task.id == latestState?.threadId || ((composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty && queuedImageCount == 0) else{pinned=true;setOpen(true,mini:true);miniUI.setCodexMessage("Finish the current input before switching tasks");return}
        floatingFocus=task.id;autoCompact=false;compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true,userInitiated:true)
        guard task.id != latestState?.threadId else{if let state=latestState{syncTaskPet(state)};return}
        let id=miniUI.beginChatLoading(task.name);expandedChatRequest=(id,task.id,task.name);composerField?.isEnabled=false
        bridge.send("chat",["id":task.id,"requestId":id])
    }
    private func reviewNextFloatingTask(){
        guard !quickChatVisible,latestState?.workspaceVoiceActive != true,!workspaceMorph.expanded,!workspaceMorph.transitioning,!floatingReviews.isEmpty,!miniOpen,!approvalPopout,!dictating,!finalizingDictation,!chatVoice.active,!voicePrompt.active,!shortcutPreparation.busy,
              expandedChatRequest == nil,sendingPrompt == nil,queuedImageCount == 0,(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines).isEmpty else{return}
        openFloatingTask(floatingReviews.removeFirst(),preview:false)
    }

    private func setOpen(_ requested: Bool, mini requestedMini: Bool = false,userInitiated:Bool=false,forceReveal:Bool=false) {
        if !requested || !requestedMini{slashPicker.hide();miniUI.dismissComposerPickers()}
        if approvalPopout && (!requested || requestedMini){approvalPopout=false;approvalUI.reset()}
        if compactVoiceMode,userInitiated {voiceExpanded=requested && requestedMini}
        let presentation=CompactVoicePresentation.layout(active:compactVoiceMode,open:requested,expanded:requestedMini,manualExpansion:voiceExpanded)
        let desired=approvalPopout || presentation.open,mini=approvalPopout ? false:presentation.expanded
        if desired && mini {autoCompact=false}
        if !desired || mini{passiveCompact=false;passiveVoice=false;passiveRetire?.cancel();passiveRetire=nil}
        if mediaVisible && (mini || compactWhileWorking || dictating || experimentalWebMode){dismissMedia()}
        guard panel != nil, forceReveal || desired != open || (desired && mini != miniOpen) || presentedCompact != compactWhileWorking || presentedApproval != approvalPopout || (approvalPopout && abs(presentedApprovalHeight-popoutHeight)>0.5) else { refreshChecks();return }
        CATransaction.begin();CATransaction.setDisableActions(true);defer{CATransaction.commit()}
        let wasApproval=presentedApproval
        presentedApproval=approvalPopout;presentedApprovalHeight=approvalPopout ? popoutHeight:0
        if petGalleryVisible && (!desired || !mini){hidePetGallery(restore:false)}
        panelRevision += 1;let revision=panelRevision
        let wasOpen=open,wasMini=miniOpen; open = desired; miniOpen = desired && mini;presentedCompact=compactWhileWorking;if desired && !wasOpen{play()}else if !desired{playToken += 1;sprite.removeAnimation(forKey:"spriteFrames")}; panel.ignoresMouseEvents = !miniOpen;composerField?.isHidden = !miniOpen;conversationScroll?.isHidden = !miniOpen;resizeHandles.isHidden = !miniOpen || experimentalWebMode;refreshChecks()
        if desired,!mini,!approvalPopout,let state=latestState,let request=CompactApprovalRequest(state:state){
            DispatchQueue.main.async{[weak self] in guard let self,self.panelRevision==revision,self.open,!self.miniOpen,!self.approvalPopout,let state=self.latestState,request.matches(state),!self.workspaceIsForeground else{return};self.presentCompactApproval(request,automatic:true)}
        }
        media.setSuppressed(mediaSuppressed)
        let previousPath=forceReveal ? path(width:notchWidth,height:notchHeight):(mask.presentation()?.path ?? mask.path)
        let oldIndicatorX=indicator.layer.presentation()?.position.x ?? indicator.layer.position.x
        let oldAccentY=[indicator.leftAccentLayer,indicator.accentLayer].map{$0.presentation()?.position.y ?? $0.position.y}
        let oldFileOpacity=indicator.fileLayer.presentation()?.opacity ?? indicator.fileLayer.opacity
        let oldFileTransform=indicator.fileLayer.presentation()?.transform ?? indicator.fileLayer.transform
        let traveling=[sprite,indicator.fileLayer,indicator.leftAccentLayer,indicator.accentLayer]
        let oldPositions=traveling.map{$0.presentation()?.position ?? $0.position}
        let approvalResizeDuration:TimeInterval=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:0.28
        layoutPanel(present:false,approvalDuration:approvalPopout && wasApproval ? approvalResizeDuration:0)
        CATransaction.begin();CATransaction.setDisableActions(true)
        indicator.layer.position.x=oldIndicatorX;indicator.fileLayer.opacity=oldFileOpacity;indicator.fileLayer.transform=oldFileTransform
        for (index,accent) in [indicator.leftAccentLayer,indicator.accentLayer].enumerated(){accent.position.y=oldAccentY[index]}
        CATransaction.commit()
        let destination = path(width:desired ? expandedWidth:notchWidth,height:desired ? popoutHeight:notchHeight), animation = CABasicAnimation(keyPath:"path")
        animation.fromValue = previousPath; animation.toValue = destination
        animation.duration = NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:(desired ? (mini || approvalPopout ? 0.28:0.24):0.22); animation.timingFunction = CAMediaTimingFunction(controlPoints: 0.16, 0.85, 0.25, 1)
        for (index,layer) in traveling.enumerated() {
            let move=CABasicAnimation(keyPath:layer === sprite ? "position":"position.x")
            if layer === sprite{move.fromValue=oldPositions[index];move.toValue=layer.position}else{move.fromValue=oldPositions[index].x;move.toValue=layer.position.x}
            move.duration=animation.duration;move.timingFunction=animation.timingFunction;layer.add(move,forKey:"responsiveTravel")
        }
        animateTopAccessories(forMini:miniOpen,duration:animation.duration)
        animateCornerAccents(toBottom:miniOpen || approvalPopout,duration:animation.duration)
        if approvalPopout,(!wasApproval || !wasOpen),let layer=approvalUI.layer{visualTransition.reveal(layer,visible:true,fromOpacity:0)}
        miniUI.setVisible(miniOpen,duration:animation.duration)
        syncConversationTheme()
        if miniOpen && !wasMini { if let state=latestState {renderConversation(state.history,forceBottom:true,deferReveal:true)};miniListening=false;miniUI.resetFiles(); indicator.setFileOpen(false); currentScene="";if let state=latestState{showActivity(displayActivity(state))};setMotion("waving",replay:true) }
        else if wasMini && !miniOpen { textReveal.finish();mathRenderer.suspend();mathRefreshWork?.cancel();mathRefreshWork=nil;historyControl.update(hasMore:false,loading:false);miniListening=false;if compactWhileWorking,let state=latestState {currentScene="";showActivity(displayActivity(state))}else{applyIndicator()};syncMotion(force:true) }
        mask.path=destination
        animation.beginTime=mask.convertTime(CACurrentMediaTime(),from:nil);animation.fillMode = .backwards
        mask.add(animation,forKey:"width")
        if showsNotchSurface{panel.orderFrontRegardless();if !dropPanel.ignoresMouseEvents{dropPanel.orderFrontRegardless()}else{dropPanel.orderOut(nil)}}else{panel.orderOut(nil);dropPanel?.orderOut(nil)}
        if miniOpen && !wasMini && showsNotchSurface && morphingText,let view=conversationText{
            textReveal.reveal([NSRange(location:0,length:view.textStorage?.length ?? 0)],in:view,loading:true)
        }
        if compactVoiceMode{compactVoiceIndicator=nil;renderCompactVoice()}
        if !desired {indicator.hide();currentScene="";mathRefreshWork?.cancel();mathRefreshWork=nil;DispatchQueue.main.asyncAfter(deadline: .now()+animation.duration) { [weak self] in
            guard let self,!self.open,self.panelRevision == revision else { return }; self.panel.orderOut(nil)
            self.reviewNextFloatingTask()
        } }
    }


    func showUnifiedNotchOnLaunch(){pinned=true;setOpen(true,mini:false,userInitiated:true)}
    func toggleUnifiedSurface(){
        if quickChatVisible{quickReturn = .extended;EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");return}
        if workspaceNotchOverlay && miniOpen{expandWorkspace()}
        else if workspaceMorph.expanded{collapseWorkspace()}
        else{pinned=true;compactWhileWorking=false;setOpen(true,mini:true,userInitiated:true)}
    }
    @objc func expandWorkspace(){
        guard EmbeddedCodeRuntime.shared.active,let window=EmbeddedCodeRuntime.shared.workspaceWindow,panel != nil else{return}
        miniUI.cancelPending();finishExpandedChatLoading()
        if workspaceMorph.expanded && workspaceIsForeground && !workspaceNotchOverlay && !workspaceMorph.transitioning{window.makeKeyAndOrderFront(nil);return}
        revealWorkspace()
    }
    func launchUnifiedWorkspace(){guard !quickChatVisible,quickPrepared == nil else{return};revealWorkspace()}
    func revealWorkspace(from explicitSource:CGRect?=nil,sourceIsNotch:Bool=true){
        guard let window=EmbeddedCodeRuntime.shared.workspaceWindow,let panel else{return}
        passiveCompact=false;passiveVoice=false;passiveRetire?.cancel();passiveRetire=nil
        workspaceAutomaticStatus=false;workspaceStatusRetire?.cancel();workspaceStatusRetire=nil;workspaceStatusMorph.cancel()
        let replay=workspaceMorph.expanded && (workspaceNotchOverlay || !workspaceIsForeground)
        if workspaceMorph.expanded && !replay && !workspaceMorph.transitioning{window.makeKeyAndOrderFront(nil);return}
        let quickSource=quickChatVisible ? quickWindow?.frame:nil
        if quickSource != nil || !sourceIsNotch{quickMorph.retireBackdrop()}
        if quickChatVisible{quickReturn = .handoff;EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");quickChatVisible=false}
        let width=open ? expandedWidth:notchWidth,height=open ? popoutHeight:notchHeight
        let settledSource=CGRect(x:panel.frame.midX-width/2,y:panel.frame.maxY-height,width:width,height:height)
        let source=explicitSource ?? quickSource ?? (panel.isVisible ? (mask.presentation()?.path ?? mask.path).map{panel.convertToScreen($0.boundingBoxOfPath)} ?? settledSource:settledSource)
        miniUI.cancelPending();compactWhileWorking=true;compactThreadID=latestState?.threadId;autoCompact=false;pinned=true
        workspaceNotchOverlay=false;voiceExpanded=false
        workspaceMorph.openingOnlyOnce=latestState?.workspaceOpeningOnce ?? true
        workspaceMorph.expand(window,from:source,replay:replay,sourceNotch:quickSource == nil && sourceIsNotch && !islandMode,crossfade:quickSource != nil || !sourceIsNotch,sourceRadius:islandMode ? islandRadius:(quickSource != nil || !sourceIsNotch ? 20:10)){[weak self] in self?.syncWorkspacePet();self?.workspaceFocusChanged()}
        setOpen(true,mini:false);if !approvalPopout{panel.orderOut(nil);dropPanel?.orderOut(nil)};floatingTasks.setSuppressed(true);syncWorkspacePet()
    }
    @objc func collapseWorkspace(){
        if workspaceMorph.expanded && !workspaceIsForeground{expandToRunningChat();return}
        foldWorkspace()
    }
    // An explicit request from Code already identifies the surface to fold.
    // WindowServer focus may still be settling after a reveal; only the global
    // shortcut uses focus to choose a background overlay instead of a fold.
    func foldWorkspace(){
        workspaceNotchOverlay=false
        guard let window=EmbeddedCodeRuntime.shared.workspaceWindow,let panel else{return}
        pinned=true;compactWhileWorking=false;autoCompact=false
        let width=expandedSize?.width ?? notchWidth+160
        let target=CGRect(x:panel.frame.midX-width/2,y:panel.frame.maxY-canvasHeight,width:width,height:canvasHeight)
        workspaceMorph.collapse(window,to:target,into:panel,revealNotch:{[weak self] in
            guard let self else{return}
            self.restoreWorkspacePet()
            self.setOpen(true,mini:true,userInitiated:true)
        },targetNotch:!islandMode,targetRadius:islandMode ? islandRadius:10){[weak self] in
            guard let self else{return}
            self.floatingTasks.setSuppressed(self.suppressFloatingTasks)
            if !self.quickChatVisible,self.showsNotchSurface{self.panel.makeKey();self.reviewNextFloatingTask()}
        }
    }

    private func dismissWorkspace(){
        guard let window=EmbeddedCodeRuntime.shared.workspaceWindow,let panel else{return}
        let busy=latestState.map{!$0.activeTasks.isEmpty || $0.activeTurnId != nil} ?? false
        let width=busy ? notchWidth+160:notchWidth
        let target=CGRect(x:panel.frame.midX-width/2,y:panel.frame.maxY-notchHeight,width:width,height:notchHeight)
        closeNotch()
        workspaceMorph.collapse(window,to:target,targetNotch:!islandMode,targetRadius:islandMode ? islandRadius:10){[weak self] in
            guard let self else{return};self.restoreWorkspacePet();self.floatingTasks.setSuppressed(self.suppressFloatingTasks);if self.open{self.placePanel()}
        }
    }
    private var workspaceIsForeground:Bool{EmbeddedCodeRuntime.shared.workspaceIsForeground && !quickChatVisible}
    private var showsNotchSurface:Bool{approvalPopout || (!quickChatVisible && (workspaceMorph.presentingNotch || (!workspaceMorph.transitioning && (!workspaceMorph.expanded || workspaceNotchOverlay))))}
    private func prepareNotchOverlay(){
        workspaceAutomaticStatus=false;workspaceStatusRetire?.cancel();workspaceStatusRetire=nil;workspaceStatusMorph.cancel()
        guard workspaceMorph.expanded || workspaceMorph.transitioning else{return}
        workspaceNotchOverlay=true
        if workspaceMorph.transitioning,let window=EmbeddedCodeRuntime.shared.workspaceWindow{workspaceMorph.retainOpen(window)}
        restoreWorkspacePet()
    }
    @objc private func scheduleWorkspaceFocusRefresh(){
        guard !workspaceFocusRefreshPending else{return};workspaceFocusRefreshPending=true
        DispatchQueue.main.async{[weak self] in guard let self else{return};self.workspaceFocusRefreshPending=false;self.workspaceFocusChanged()}
    }
    private var workspaceNeedsStatus:Bool{
        guard let state=latestState else{return dictating || finalizingDictation || voicePrompt.active}
        return chatVoice.active || workspaceVoiceRequest != nil || pendingVoiceStart != nil || shortcutPreparation.busy || state.approval || ["permission","question"].contains(state.activity.scene) || state.activeTurnId != nil || !state.activeTasks.isEmpty || state.workspaceVoiceActive == true || dictating || finalizingDictation || voicePrompt.active
    }
    @objc func workspaceFocusChanged(){
        guard EmbeddedCodeRuntime.shared.active else{return}
        if menuQuickAnchor != nil,quickChatVisible,!quickMorph.transitioning,!EmbeddedCodeRuntime.shared.isForeground(quickWindow){EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");return}
        let foreground=workspaceIsForeground,returned=workspaceWasForeground == false && foreground
        workspaceWasForeground=foreground
        if workspaceMorph.expanded,!workspaceMorph.transitioning,!quickChatVisible{
            if EmbeddedCodeRuntime.shared.workspaceIsForeground{
                if let state=latestState{updateCompactApproval(state)}
                if workspaceNotchOverlay && returned{
                    if open || panel?.isVisible == true{revealWorkspace()}else{workspaceNotchOverlay=false;workspaceAutomaticStatus=false;workspaceStatusMorph.cancel();syncWorkspacePet()}
                }
            }else if workspaceNeedsStatus{
                workspaceStatusRetire?.cancel();workspaceStatusRetire=nil
                if !workspaceNotchOverlay || (workspaceAutomaticStatus && !open),let panel{
                    let arrivingFromWorkspace = !workspaceNotchOverlay
                    workspaceNotchOverlay=true;workspaceAutomaticStatus=true;restoreWorkspacePet()
                    autoCompact=false;pinned=true;compactWhileWorking=true
                    let target=CGRect(x:panel.frame.midX-(notchWidth+160)/2,y:panel.frame.maxY-notchHeight,width:notchWidth+160,height:notchHeight)
                    if arrivingFromWorkspace,let window=EmbeddedCodeRuntime.shared.workspaceWindow{workspaceStatusMorph.show(from:window.frame,to:target,motion:workspaceMorph.motionEnabled)}
                    setOpen(true,mini:false,forceReveal:!panel.isVisible)
                    currentScene="";if let state=latestState{showActivity(displayActivity(state))}
                }
            }else if workspaceAutomaticStatus,workspaceStatusRetire == nil{
                // A brief result beat settles into the physical notch. A new
                // task or a return to Code cancels this exact pending dismissal.
                let finish=DispatchWorkItem{[weak self] in
                    guard let self else{return};self.workspaceStatusRetire=nil
                    guard self.workspaceAutomaticStatus,!self.workspaceNeedsStatus,!self.workspaceIsForeground else{return}
                    self.compactWhileWorking=false;self.autoCompact=false;self.setOpen(false)
                    DispatchQueue.main.asyncAfter(deadline:.now()+0.25){[weak self] in
                        guard let self,self.workspaceAutomaticStatus,!self.open,!self.workspaceNeedsStatus else{return}
                        self.workspaceNotchOverlay=false;self.workspaceAutomaticStatus=false;self.workspaceStatusMorph.cancel()
                    }
                };workspaceStatusRetire=finish;DispatchQueue.main.asyncAfter(deadline:.now()+1.1,execute:finish)
            }
        }
        if let state=latestState{updateCompactApproval(state)}
    }
    private var suppressFloatingTasks:Bool{quickChatVisible || workspaceMorph.expanded || workspaceMorph.transitioning || approvalPopout}
    func cancelModifierShortcut(){
        commandChordActive=commandChordActive || commandDown || modifierFlags().contains(.command)
        commandDismissGesture.cancel();shortcutGesture=ShortcutGesture();cancelShortcutHold();cancelWorkspaceHold();holdWork?.cancel();lastControlTap=0;cancelVoicePromptForKeypress()
    }
    func prepareMiniOpening(){
        cancelModifierShortcut()
        guard menuQuickAnchor != nil else{prepareQuickOpening();return}
        let source=quickWindow?.frame ?? menuQuickAnchor!
        menuQuickAnchor=nil
        if let window=quickWindow,let screen=window.screen ?? NSScreen.main{
            let size=CGSize(width:min(680,screen.visibleFrame.width-24),height:min(520,screen.visibleFrame.height-24))
            window.setFrame(CGRect(x:screen.visibleFrame.midX-size.width/2,y:screen.visibleFrame.midY-size.height/2,width:size.width,height:size.height),display:false)
        }
        quickPrepared=(false,source,.hidden)
    }
    func prepareQuickOpening(){
        cancelModifierShortcut()
        guard quickPrepared == nil,!(quickChatVisible && quickWindow?.isVisible == true),let panel else{return}
        let fromWorkspace=workspaceMorph.expanded && workspaceIsForeground && EmbeddedCodeRuntime.shared.workspaceWindow?.isVisible == true
        let width=open ? expandedWidth:notchWidth,height=open ? popoutHeight:notchHeight
        let notch=CGRect(x:panel.frame.midX-width/2,y:panel.frame.maxY-height,width:width,height:height)
        let source=fromWorkspace ? EmbeddedCodeRuntime.shared.workspaceWindow!.frame:(panel.isVisible ? (mask.presentation()?.path ?? mask.path).map{panel.convertToScreen($0.boundingBoxOfPath)} ?? notch:notch)
        quickPrepared=(fromWorkspace,source,fromWorkspace ? .workspace:miniOpen && panel.isVisible ? .extended:open && panel.isVisible ? .compact:.hidden)
        setQuickChatVisible(true)
    }
    func transitionQuickSurface(_ window:NSWindow,show:Bool)->Bool{
        hangDiagnostics.record(show ? "mini-code/open":"mini-code/close")
        defer{updateNinjaWallpaper()}
        guard let panel else{return false}
        quickRevision+=1;let revision=quickRevision;quickWindow=window
        quickMorph.motionEnabled=workspaceMorph.motionEnabled
        if show{
            let reversing=quickMorph.transitioning
            let prepared=quickPrepared;quickPrepared=nil
            let fromWorkspace=(prepared?.workspace ?? (workspaceMorph.expanded && workspaceIsForeground && EmbeddedCodeRuntime.shared.workspaceWindow?.isVisible == true)) || (workspaceMorph.transitioning && workspaceMorph.expanded)
            if !reversing || fromWorkspace{quickReturn=fromWorkspace ? .workspace:prepared?.returnTo ?? (miniOpen && panel.isVisible ? .extended:open && panel.isVisible ? .compact:.hidden)}
            let notchFrame=CGRect(x:panel.frame.midX-(open ? expandedWidth:notchWidth)/2,y:panel.frame.maxY-(open ? popoutHeight:notchHeight),width:open ? expandedWidth:notchWidth,height:open ? popoutHeight:notchHeight)
            let source=prepared?.source ?? (fromWorkspace ? EmbeddedCodeRuntime.shared.workspaceWindow!.frame:(panel.isVisible ? (mask.presentation()?.path ?? mask.path).map{panel.convertToScreen($0.boundingBoxOfPath)} ?? notchFrame:notchFrame))
            if let anchor=menuQuickAnchor,let screen=window.screen ?? NSScreen.main {
                let width:CGFloat=min(520,screen.visibleFrame.width-24),height:CGFloat=min(540,screen.visibleFrame.height-24)
                let x=max(screen.visibleFrame.minX+8,min(anchor.midX-width/2,screen.visibleFrame.maxX-width-8))
                window.setFrame(CGRect(x:x,y:anchor.minY-height-6,width:width,height:height),display:false)
            }
            setQuickChatVisible(true)
            if let workspace=EmbeddedCodeRuntime.shared.workspaceWindow,workspace.isVisible{
                workspaceMorph.collapse(workspace,to:window.frame,targetNotch:false,retireContent:true,targetRadius:20,backdrop:false,completion:{})
            }
            if workspaceMorph.transitioning && !workspaceMorph.expanded{workspaceMorph.retireBackdrop()}
            quickMorph.expand(window,from:source,replay:true,destination:window.frame,sourceNotch:!fromWorkspace && menuQuickAnchor == nil && !islandMode,crossfade:true,sourceRadius:islandMode ? islandRadius:10,destinationRadius:20){[weak self] in self?.syncWorkspacePet();self?.updateNinjaWallpaper()}
            // Retire the existing notch through its own morph, while the shared
            // contour carries the boundary into the live mini workspace.
            setOpen(false);dropPanel?.orderOut(nil);syncWorkspacePet()
            DispatchQueue.main.asyncAfter(deadline:.now()+0.16){[weak self] in
                guard let self,self.quickRevision == revision,self.quickChatVisible else{return};self.panel?.orderOut(nil)
            }
        }else{
            quickPrepared=nil
            quickChatVisible=false
            let destination=quickReturn
            if let anchor=menuQuickAnchor,destination != .handoff {
                menuQuickAnchor=nil;quickMorph.collapse(window,to:anchor,retireContent:true,backdrop:false,completion:{[weak self] in self?.restoreWorkspacePet()});floatingTasks.setSuppressed(suppressFloatingTasks);return true
            }
            menuQuickAnchor=nil
            if destination == .workspace{
                revealWorkspace(from:window.frame,sourceIsNotch:false)
            }
            if destination == .workspace || destination == .handoff,let workspace=EmbeddedCodeRuntime.shared.workspaceWindow{
                quickMorph.collapse(window,to:workspace.frame,targetNotch:false,retireContent:true,backdrop:false,completion:{})
            }else{
                let expanded=destination == .extended,visible=destination == .compact || expanded || workspaceNeedsStatus
                let width=expanded ? (expandedSize?.width ?? notchWidth+160):(visible ? notchWidth+160:notchWidth),height=expanded ? canvasHeight:notchHeight
                let target=CGRect(x:panel.frame.midX-width/2,y:panel.frame.maxY-height,width:width,height:height)
                if visible{
                    quickMorph.collapse(window,to:target,into:panel,revealNotch:{[weak self] in
                        guard let self,self.quickRevision == revision,!self.quickChatVisible else{return}
                        self.workspaceNotchOverlay=self.workspaceMorph.expanded;self.compactWhileWorking = !expanded && self.workspaceNeedsStatus
                        self.restoreWorkspacePet()
                        self.pinned=true;self.setOpen(true,mini:expanded,userInitiated:true,forceReveal:true)
                    },targetNotch:!islandMode,targetRadius:islandMode ? islandRadius:10,completion:{})
                }else{
                    setOpen(false);quickMorph.collapse(window,to:target,targetNotch:!islandMode,retireContent:true,targetRadius:islandMode ? islandRadius:10,completion:{[weak self] in self?.restoreWorkspacePet()})
                }
            }
            floatingTasks.setSuppressed(suppressFloatingTasks)
        }
        return true
    }

    func setQuickChatVisible(_ visible:Bool){
        if !visible{quickPrepared=nil}
        guard quickChatVisible != visible else{return}
        quickChatVisible=visible
        updateNinjaWallpaper()
        if visible{
            workspaceAutomaticStatus=false;workspaceStatusRetire?.cancel();workspaceStatusRetire=nil;workspaceStatusMorph.cancel()
            workspaceNotchOverlay=false
            cancelWorkspaceHold();cancelShortcutHold()
            restoreWorkspacePet();floatingTasks.setSuppressed(true)
        }else{
            floatingTasks.setSuppressed(suppressFloatingTasks)
            if quickWindow != nil{return}
            if !miniOpen,let state=latestState,state.workspaceVoiceActive == true || !state.activeTasks.isEmpty{
                passiveCompact=true;passiveVoice=passiveVoice || state.workspaceVoiceActive == true;compactWhileWorking=true;compactThreadID=nil;autoCompact=false;pinned=false
                setOpen(true,mini:false,forceReveal:panel?.isVisible != true);showActivity(displayActivity(state))
            }else if open{placePanel()}
        }
    }
    private func revealNotchForInput(){
        passiveCompact=false;passiveVoice=false;passiveRetire?.cancel();passiveRetire=nil
        workspaceAutomaticStatus=false;workspaceStatusRetire?.cancel();workspaceStatusRetire=nil
        if quickChatVisible{quickReturn = .compact;EmbeddedCodeRuntime.shared.sendAction("quick_dismiss");quickChatVisible=false}
        guard EmbeddedCodeRuntime.shared.active,workspaceMorph.expanded || workspaceMorph.transitioning,let window=EmbeddedCodeRuntime.shared.workspaceWindow else{return}
        workspaceNotchOverlay=true;workspaceMorph.retainOpen(window);restoreWorkspacePet();floatingTasks.setSuppressed(suppressFloatingTasks);setOpen(true,mini:false,userInitiated:true,forceReveal:panel?.isVisible != true)
    }
    func unifiedVoiceHost()->NSView?{
        let surface=quickChatVisible ? quickWindow:(showsNotchSurface && open ? panel:EmbeddedCodeRuntime.shared.workspaceWindow)
        return surface?.isVisible == true ? surface?.contentView:nil
    }
    func unifiedPresentationDiagnostics()->UInt32{
        var value:UInt32=0
        if EmbeddedCodeRuntime.shared.workspaceWindow?.isVisible == true{value |= 1}
        if panel?.isVisible == true{value |= 2}
        if dropPanel?.isVisible == true{value |= 4}
        if sprite.superlayer === workspacePet.layer && !workspacePet.isHidden{value |= 8}
        if workspaceMorph.transitioning{value |= 16}
        if let window=EmbeddedCodeRuntime.shared.workspaceWindow,let screen=window.screen{
            if window.frame == screen.visibleFrame{value |= 64}
            if CommandLine.arguments.contains("--unified-startup-probe"){fputs("KAI_STARTUP_FRAME actual=\(window.frame) visible=\(screen.visibleFrame) transitioning=\(workspaceMorph.transitioning)\n",stderr)}
        }
        if quickChatVisible{value |= 128}
        if !workspaceBackground.isHidden && workspaceBackground.hasImage{value |= 256}
        if indicator.mode != .off && indicator.layer.opacity>0{value |= 32}
        if miniOpen{value |= 512}
        return value
    }
    func workspacePetGeometry(_ frame:CGRect){workspacePetSlot=frame;syncWorkspacePet()}
    func quickPetGeometry(_ frame:CGRect){quickPetSlot=frame;syncWorkspacePet()}
    private func syncWorkspacePet(){
        if latestState?.personalNinja != nil {workspacePet.isHidden=true;return}
        guard quickChatVisible || (workspaceMorph.expanded && !workspaceNotchOverlay),let host=(quickChatVisible ? quickWindow:EmbeddedCodeRuntime.shared.workspaceWindow)?.contentView else{return}
        if workspacePet.superview !== host{workspacePet.removeFromSuperview();host.addSubview(workspacePet)}
        // Notch travel uses notch coordinates; the same layer now belongs to a local header slot.
        let slot=quickChatVisible ? quickPetSlot:workspacePetSlot
        workspacePet.frame=CGRect(x:slot.minX,y:host.bounds.height-slot.maxY,width:slot.width,height:slot.height)
        if sprite.superlayer !== workspacePet.layer{sprite.removeFromSuperlayer();workspacePet.layer?.addSublayer(sprite)}
        CATransaction.begin();CATransaction.setDisableActions(true);sprite.removeAnimation(forKey:"responsiveTravel");sprite.frame=workspacePet.bounds.insetBy(dx:1,dy:0);sprite.opacity=1;sprite.isHidden=false;CATransaction.commit()
        workspacePet.isHidden=false;workspacePet.toolTip=catalog?.pets.first{$0.id == petID}?.displayName ?? "Kai task pet"
    }
    private func restoreWorkspacePet(){
        workspacePet.isHidden=true
        if sprite.superlayer !== notchContent{sprite.removeAnimation(forKey:"responsiveTravel");sprite.removeFromSuperlayer();notchContent.addSublayer(sprite)}
        layoutPet()
    }
    @objc private func workspaceSettings(){bridge.send("workspace_settings");expandWorkspace()}

    private func handleClick(_ point:CGPoint,count:Int) {
        if !miniOpen,!approvalPopout,let state=latestState,let request=CompactApprovalRequest(state:state){presentCompactApproval(request);return}
        if compactWhileWorking && !miniOpen {expandToRunningChat();return}
        guard miniOpen else{return}
        let topFrames=[sprite,indicator.fileLayer,indicator.layer].enumerated().map{index,item -> CGRect in
            let visible=item.presentation() ?? item
            guard !item.isHidden,visible.opacity>0.01,!(index == 1 && miniListening) else{return .zero}
            return visible.frame
        }
        let topTarget=NotchInputRegion.nearestTarget(at:point,frames:topFrames)
        if topTarget == 0 {
            if latestState?.personalNinja != nil || ninjaSpriteStyle != nil {return}
            if count >= 2 {petClickWork?.cancel();petClickWork=nil;petGalleryVisible ? hidePetGallery():showPetGallery()}
            else if !petGalleryVisible {petClickWork?.cancel();let work=DispatchWorkItem{[weak self] in self?.randomizePet()};petClickWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.22,execute:work)}
            return
        }
        if petGalleryVisible {
            let frame=petGallery.layer.frame,uiPoint=scalableUI.convert(point,from:panel.contentView),local=CGPoint(x:uiPoint.x-frame.minX,y:uiPoint.y-frame.minY)
            if frame.contains(uiPoint){if petGallery.handleNavigation(at:local,selected:petID){return};if let id=petGallery.pet(at:local){usePet(id);hidePetGallery()}}
            return
        }
        if topTarget == 1 { miniUI.toggleFiles();if miniUI.isFilesOpen{let requestID=UUID().uuidString;pendingFilesRequestID=requestID;indicator.setFileLoading(true);bridge.send("files",["requestId":requestID])}else{pendingFilesRequestID=nil;indicator.setFileLoading(false)};return }
        if topTarget == 2 {dismissNotchView();return}
        let frame=miniUI.layer.frame,uiPoint=scalableUI.convert(point,from:panel.contentView), local=CGPoint(x:uiPoint.x-frame.minX,y:uiPoint.y-frame.minY)
        if miniUI.interactiveBounds.contains(local) { _=miniUI.handleClick(local,count:count) }
    }

    private func handleHover(_ point:CGPoint?) {
        guard miniOpen else{return}
        let local=point.map{scalableUI.convert($0,from:panel.contentView)}
        if petGalleryVisible {petGallery.hover(at:local.flatMap{petGallery.layer.frame.contains($0) ? $0:nil});miniUI.handleHover(nil)}
        else {miniUI.handleHover(local.flatMap{miniUI.layer.frame.contains($0) ? $0:nil})}
    }
    private func miniPoint(_ point:CGPoint)->CGPoint{
        let converted=scalableUI.convert(point,from:panel.contentView),frame=miniUI.layer.frame
        return CGPoint(x:converted.x-frame.minX,y:converted.y-frame.minY)
    }
    private func handleScroll(_ point:CGPoint,_ delta:CGFloat){
        guard miniOpen else{return};let local=miniPoint(point)
        if miniUI.layer.bounds.contains(local){miniUI.scroll(delta,at:local)}
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

    private func layoutComposer(_ frame:CGRect,animated:Bool){guard let field=composerField else{return};let root=miniUI.layer.frame,modern=frame.height>40,h:CGFloat=modern ? 24:18,target=CGRect(x:root.minX+frame.minX+10,y:root.minY+(modern ? frame.maxY-28:frame.midY-h/2-2.25),width:max(20,frame.width-(modern ? 20:73)),height:h);if animated{NSAnimationContext.runAnimationGroup{context in context.duration=0.42;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);field.animator().frame=target}}else{field.frame=target}}
    private func layoutConversation(_ frame:CGRect){guard let scroll=conversationScroll else{return};let root=miniUI.layer.frame;scroll.frame=CGRect(x:root.minX+frame.minX,y:root.minY+frame.minY,width:frame.width,height:frame.height)}
    private func setPrompt(_ text:String,dictation:Bool=false){
        slashPicker.hide();dictationTextReveal.finish()
        if dictation,let field=composerField{
            let previous=field.stringValue;DictationComposerEditing.update(field,text:text)
            if morphingText,let suffix=TextRevealChange.suffix(old:previous,new:text){
                DispatchQueue.main.async{[weak self,weak field] in guard let self,let field,field.stringValue==text,let editor=field.currentEditor() as? NSTextView,editor.string==text else{return};self.dictationTextReveal.reveal([suffix],in:editor,loading:false)}
            }
        }else{composerField?.stringValue=text}
        miniUI.setComposerHasText(!text.isEmpty);syncMotion(force:!text.isEmpty)
        if dictation{synchronizeComposerDraft()}
    }
    private func updateSlashPicker(){
        guard miniOpen,let field=composerField,let editor=field.currentEditor() as? NSTextView,!editor.hasMarkedText() else{slashPicker.hide();return}
        if slashPicker.superview == nil{scalableUI.addSubview(slashPicker);slashPicker.onAccept={[weak self] name in self?.acceptNativeSlash(name)}}
        slashPicker.update(text:field.stringValue,selection:editor.selectedRange(),commands:latestState?.slashCommands ?? [],anchor:field.frame)
    }
    private func acceptNativeSlash(_ name:String){
        guard pendingSlashID == nil,let field=composerField,let editor=field.currentEditor() as? NSTextView,NativeSlashToken.at(field.stringValue,selection:editor.selectedRange()) != nil else{return}
        let id=UUID().uuidString;pendingSlashID=id;pendingSlashDraft=field.stringValue
        pendingSlashPicker=["agent","model"].contains(name) ? name:nil
        slashPicker.hide();bridge.send("composer_command",["name":name,"draft":field.stringValue,"caret":editor.selectedRange().location,"requestId":id,"pickerSurface":"notch"])
    }
    private func synchronizeComposerDraft(){
        guard EmbeddedCodeRuntime.shared.active,latestState != nil,sendingPrompt == nil,let field=composerField else{return}
        bridge.send("draft",["text":field.stringValue])
    }
    func controlTextDidChange(_ obj:Notification){dictationTextReveal.finish();miniUI.setComposerHasText(!(composerField?.stringValue.isEmpty ?? true));synchronizeComposerDraft();updateSlashPicker();syncMotion(force:true)}
    func control(_ control:NSControl,textView:NSTextView,doCommandBy selector:Selector)->Bool{
        if slashPicker.active {
            switch selector {
            case #selector(NSResponder.moveUp(_:)):slashPicker.move(-1);return true
            case #selector(NSResponder.moveDown(_:)):slashPicker.move(1);return true
            case #selector(NSResponder.cancelOperation(_:)):slashPicker.dismiss();return true
            case #selector(NSResponder.insertTab(_:)),#selector(NSResponder.insertNewline(_:)):slashPicker.accept();return true
            default:slashPicker.hide()
            }
        }
        if selector == #selector(NSResponder.insertNewline(_:)){sendPrompt();return true};return false
    }

    private func sendPrompt(){if attachmentEncoding || attachmentRequest != nil{pendingAttachmentSend=true;miniUI.setCodexMessage("Preparing images…");return};guard pendingSlashID == nil,expandedChatRequest == nil,sendingPrompt == nil, !shortcutPreparation.busy else{return};if !EmbeddedCodeRuntime.shared.active,latestState?.threadId?.hasPrefix("code:") == true,latestState?.inputBlocked == true{miniUI.setCodexMessage("Code is busy or loading — your prompt is kept here.");return};let prompt=(composerField?.stringValue ?? "").trimmingCharacters(in:.whitespacesAndNewlines);guard !prompt.isEmpty || queuedImageCount>0 else{return};sendingPrompt=prompt;setMotion("running-left",replay:true);speech.cancelAudio();let requestID=UUID().uuidString;sendingPromptRequestID=requestID;bridge.send("send",["text":prompt,"requestId":requestID]);if compactOnSend{hidePetGallery(restore:false);autoCompact=false;compactWhileWorking=true;compactThreadID=latestState?.threadId;compactStartHistoryCount=latestState?.history.count ?? 0;pinned=true;setOpen(true,mini:false)}}
    private func answerApproval(_ action:String){
        if action == "interrupt"{speech.cancelAudio()}
        var response=action,fields:[String:Any]=["requestId":miniUI.pendingRequestID ?? ""]
        if let state=latestState,let request=CompactApprovalRequest(state:state),request.isQuestion {
            guard let (mapped,extra)=request.response(action) else{
                miniUI.cancelPending()
                if let review=request.review{nativeReview.present(review,from:composerField ?? panel.contentView!){[weak self] action,fields in self?.bridge.send(action,fields)}}
                return
            }
            response=mapped;fields.merge(extra){_,new in new}
        }
        if let id=latestState?.approvalThreadId{fields["threadId"]=id};if let id=latestState?.approvalRequestId{fields["approvalRequestId"]=id};bridge.send(response,fields)
    }

    @objc private func previewCompactApproval(){
        guard latestState?.approval != true else{featureStatusItem?.title="Review the pending approval before starting a preview";return}
        presentCompactApproval(CompactApprovalRequest(preview:true))
    }
    private func presentCompactApproval(_ request:CompactApprovalRequest,automatic:Bool=false){
        guard !automatic || approvalDismissal.allows(request) else{return}
        if !automatic{approvalDismissal.reset()}
        if experimentalWebMode{toggleExperimentalWebMode()}
        if !approvalPopout{beforeApproval=(open,miniOpen,pinned,autoCompact)}
        approvalPopout=true;autoCompact=false;compactWhileWorking=false;pinned=true
        approvalUI.show(request)
        if let profile=gradientCatalog.profileByPetID[conversationPetID.isEmpty ? petID:conversationPetID]{approvalUI.setProfile(profile)}
        setOpen(true);panel.orderFrontRegardless();dropPanel?.orderFrontRegardless();floatingTasks.setSuppressed(suppressFloatingTasks);sounds.playScene("permission");indicator.showScene("permission")
    }
    private func dismissCompactApproval(){
        approvalPopout=false;approvalUI.reset();pinned=beforeApproval.pinned
        let working=latestState?.activeTurnId != nil || !(latestState?.activeTasks.isEmpty ?? true)
        compactWhileWorking=working && !beforeApproval.mini
        autoCompact=working && !beforeApproval.mini && (beforeApproval.automatic || !beforeApproval.open)
        setOpen((beforeApproval.open && !beforeApproval.automatic) || working,mini:beforeApproval.mini)
        floatingTasks.setSuppressed(suppressFloatingTasks)
        if !showsNotchSurface{panel.orderOut(nil);dropPanel?.orderOut(nil)}
    }
    private func updateCompactApproval(_ state:KaiState){
        approvalDismissal.observe(CompactApprovalRequest(state:state))
        if EmbeddedCodeRuntime.shared.active,workspaceIsForeground || (quickChatVisible && EmbeddedCodeRuntime.shared.isForeground(quickWindow)){
            lastApproval=nil
            if approvalPopout,approvalUI.request?.preview != true{nativeReview.dismiss();dismissCompactApproval()}
            return
        }
        guard let request=CompactApprovalRequest(state:state) else{
            lastApproval=nil
            if approvalPopout,approvalUI.request?.preview != true{nativeReview.dismiss();dismissCompactApproval()}
            return
        }
        if approvalPopout,approvalUI.request?.matches(state) == true{
            approvalUI.show(request);approvalUI.receive(state);return
        }
        guard request.id != lastApproval?.id || request.thread != lastApproval?.thread else{return}
        lastApproval=request;presentCompactApproval(request,automatic:true)
    }
    private func decidePopout(_ action:String,request:CompactApprovalRequest){
        if request.preview{featureStatusItem?.title="Approval preview: \(action == "approve" ? "approved":action == "reject" ? "declined":"stopped") · nothing ran";dismissCompactApproval();return}
        guard let state=latestState,request.matches(state) else{return}
        if action == "approval_stop",chatVoice.threadID == request.thread{speech.cancelAudio();chatVoice.stop()}
        guard let (response,extra)=request.response(action) else{return}
        let id=UUID().uuidString;approvalUI.begin(id)
        var fields=extra;fields["threadId"]=request.thread;fields["approvalRequestId"]=request.id;fields["requestId"]=id
        bridge.send(response,fields)
    }

    private func synchronizeOwnerAttachments(_ state:KaiState){
        guard !attachmentEncoding,state.attachments != attachmentOwnerPaths else{return}
        attachmentOwnerPaths=state.attachments
        let paths=state.attachments,generation=UUID();attachmentGeneration=generation
        queuedImageCount=paths.count
        imageEncodingQueue.addOperation{[weak self] in
            let pixels=paths.compactMap{path -> (String,CGImage)? in
                guard let source=CGImageSourceCreateWithURL(URL(fileURLWithPath:path) as CFURL,nil),let cg=CGImageSourceCreateThumbnailAtIndex(source,0,[kCGImageSourceCreateThumbnailFromImageAlways:true,kCGImageSourceThumbnailMaxPixelSize:168,kCGImageSourceCreateThumbnailWithTransform:true] as CFDictionary) else{return nil}
                return (path,cg)
            }
            DispatchQueue.main.async{[weak self] in guard let self,self.attachmentGeneration==generation,self.attachmentOwnerPaths==paths else{return};self.ownerAttachmentImages=pixels.map{(path:$0.0,image:NSImage(cgImage:$0.1,size:NSSize(width:$0.1.width,height:$0.1.height)))};self.miniUI.synchronizeImages(self.ownerAttachmentImages.map(\.image))}
        }
    }
    private func queueImages(_ images:[NSImage]){
        imageEncodingWork?.cancel();imageEncodingWork=nil
        let generation=UUID();attachmentGeneration=generation
        queuedImageCount=images.count;attachmentEncoding = !images.isEmpty
        guard !images.isEmpty else{
            let request=UUID().uuidString
            if EmbeddedCodeRuntime.shared.active{attachmentRequest=(request,0)}
            bridge.send("attachments",["paths":[String](),"requestId":request])
            if pendingAttachmentSend{pendingAttachmentSend=false;sendPrompt()}
            return
        }
        let retained=EmbeddedCodeRuntime.shared.active ? images.compactMap{image in ownerAttachmentImages.first{$0.image === image}?.path}:[]
        let additions=images.filter{image in !ownerAttachmentImages.contains{$0.image === image}}
        let snapshots=additions.map(AttachmentImageEncoding.Input.init)
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
                self.attachmentFiles.append(contentsOf:prepared);self.queuedImageCount=prepared.count+retained.count
                let request=UUID().uuidString
                if EmbeddedCodeRuntime.shared.active{self.attachmentRequest=(request,self.queuedImageCount)}
                self.bridge.send("attachments",["paths":retained+prepared.map(\.path),"requestId":request])
                if let error{self.pendingAttachmentSend=false;self.miniUI.setCodexMessage(error)}
                else if self.pendingAttachmentSend && !EmbeddedCodeRuntime.shared.active{self.pendingAttachmentSend=false;self.sendPrompt()}
            }
        }
        imageEncodingQueue.addOperation(operation)
    }

    private func receive(_ state:KaiState){
        menuBarIdentity.synchronize(state)
        if let interface=state.interfaceFont,let code=state.codeFont{applyTypography(interface:interface,code:code)}
        let pickerToOpen=pendingSlashPicker
        let slashResult=pendingSlashID.flatMap(state.requestResult)
        let slashAccepted=slashResult?.error == nil && slashResult != nil && composerField?.stringValue == pendingSlashDraft
        if slashResult != nil || latestState?.ownerToken != state.ownerToken{pendingSlashID=nil;pendingSlashDraft=nil;pendingSlashPicker=nil;slashPicker.hide()}
        defer{refreshMedia()}
        nativeReview.update(state)
        if EmbeddedCodeRuntime.shared.active {
            if latestState?.ownerToken != state.ownerToken{
                imageEncodingWork?.cancel();imageEncodingWork=nil;attachmentGeneration=UUID();attachmentEncoding=false;attachmentRequest=nil;pendingAttachmentSend=false
            }
            synchronizeOwnerAttachments(state)
            if let error=state.workspaceVoiceError,error != latestState?.workspaceVoiceError,compactVoiceMode || workspaceVoiceRequest != nil{failCompactVoice(error)}
            if let motion=state.workspaceMotion{workspaceMorph.motionEnabled=motion}
            if let text=state.workspaceTextMorph,text != morphingText{morphingText=text;textRevealPreferences.enabled=text;textMorphItem?.state=text ? .on:.off}
            if let voice=state.workspaceVoiceChoice{realtimeVoiceMenu.synchronize(voice)}
            miniUI.setVoiceActive(state.workspaceVoiceActive == true)
            if let levels=state.workspaceVoiceLevels,["listening","speaking"].contains(state.workspaceVoiceScene ?? ""){indicator.setExternalLevels(levels)}
            if let request=attachmentRequest,let reply=state.requestResult(request.id),reply.error != nil || !state.inputBlocked {
                attachmentRequest=nil
                if let error=reply.error{pendingAttachmentSend=false;miniUI.setCodexMessage(error)}
                else if request.count != state.attachments.count{pendingAttachmentSend=false;miniUI.setCodexMessage("An image could not be imported. Review the draft before sending.")}
                else if pendingAttachmentSend{
                    pendingAttachmentSend=false
                    let owner=state.ownerToken
                    DispatchQueue.main.async{[weak self] in guard let self,self.latestState?.ownerToken==owner else{return};self.sendPrompt()}
                }
            }
        }
        if EmbeddedCodeRuntime.shared.active,let draft=state.composerDraft {
            let local=composerField?.stringValue ?? ""
            let changed=latestState?.ownerToken != state.ownerToken
            if let base=bridge.acceptedDraftBase{unifiedDraftBase=base}
            let accepted=slashAccepted || sendingPromptRequestID.flatMap(state.requestResult).map{$0.error == nil} == true
            let editedAfterSend=accepted && sendingPrompt != nil && local.trimmingCharacters(in:.whitespacesAndNewlines) != sendingPrompt
            if editedAfterSend{unifiedDraftBase=draft}
            else if UnifiedDraftHandoff.shouldAdopt(local:local,base:unifiedDraftBase,remote:draft,selectionChanged:changed,sendAccepted:accepted) {
                unifiedDraftBase=draft
                if !dictating && !finalizingDictation && !shortcutPreparation.busy{setPrompt(draft)}
            }
        }
        if let connection=state.codeConnection{codeConnectionItem?.toolTip=connection.error;codeConnectionItem?.title=EmbeddedCodeRuntime.shared.active ? "Code runtime · in this app":connection.enabled ? "Connect to Code · \(connection.connected ? "connected":"waiting")":"Connect to Code"}
        let previous=latestState,previousNotice=previous?.notice,wasFloating=floatingTasks.tracking;latestState=state
        if (previous?.personalNinja == nil) != (state.personalNinja == nil){updateGlassBackground()}
        // Composer adoption above can render the voice indicator. Keep the
        // connecting request until its new owner snapshot is actually installed.
        if EmbeddedCodeRuntime.shared.active{
            if let request=workspaceVoiceRequest,state.requestResult(request) != nil || state.workspaceVoiceActive == true{
                workspaceVoiceRequest=nil;voiceLaunchTimeout?.cancel();voiceLaunchTimeout=nil
                if let error=state.requestResult(request)?.error{failCompactVoice(error)}
            }
            if previous?.workspaceVoiceActive == true,state.workspaceVoiceActive != true,state.workspaceVoiceError == nil,compactVoiceMode,!voicePrompt.active,pendingVoiceStart == nil,!shortcutPreparation.busy{closeNotch()}
        }
        var confirmedCompletion=false
        if let models=state.models{defaultModelMenu.update(models)}
        let prepending=previous?.threadId == state.threadId && state.historyLoading != true && earlierHistoryRequest.flatMap(state.requestResult) != nil
        if prepending || previous?.threadId != state.threadId{earlierHistoryRequest=nil}
        if let thread=state.threadId,thread != lastOpenedChatID {lastOpenedChatID=thread;UserDefaults.standard.set(thread,forKey:"lastOpenedChatID")}
        if let target=dictationDestination,state.threadId != target {cancelShortcutInput()}
        let preparationReply=shortcutPreparation.pendingRequestID.flatMap(state.requestResult)
        shortcutPreparation.receive(thread:state.threadId,request:preparationReply?.id,error:preparationReply?.error)
        if mediaVisible && MediaCompanion.codexNeedsNotch(state) {
            dismissMedia(closeNotch:false)
            if !miniOpen {autoCompact=true;compactWhileWorking=true;compactThreadID=nil;setOpen(true,mini:false)}
        }
        if !state.standalone,previous?.project != state.project{UserDefaults.standard.set(state.project,forKey:"kaiProject")};miniUI.setState(state);if slashAccepted,let pickerToOpen{miniUI.showAgentPicker(pickerToOpen=="model" ? "models":pickerToOpen)};if chatVoice.active || state.workspaceVoiceActive == true || previous?.workspaceVoiceActive == true{speech.synchronize(state)}else{speech.receive(state)};syncTaskPet(state);if miniOpen {renderConversation(state.history,forceBottom:previous?.threadId != state.threadId,preserveViewport:prepending)}
        floatingTasks.setSuppressed(suppressFloatingTasks)
        petAppIcon.setWorking(!state.activeTasks.isEmpty || state.workspaceVoiceActive == true)
        floatingTasks.update(state.activeTasks,finished:state.finishedTasks ?? [],pets:taskPets)
        if EmbeddedCodeRuntime.shared.active{
            for done in state.finishedTasks ?? []{
                let key=done.id+":"+(done.runId ?? "")
                guard !reviewedFinished.contains(key) else{continue}
                confirmedCompletion=true;reviewedFinished.append(key);if reviewedFinished.count>128{reviewedFinished.removeFirst()}
                let task=previous?.activeTasks.first{$0.id==done.id} ?? KaiActiveTask(id:done.id,name:done.id==state.threadId ? state.threadName:"Completed chat",scene:"success",text:"Completed",runId:done.runId,parentId:done.parentId)
                if done.shouldAutoReview,task.parentId == nil,!floatingReviews.contains(where:{$0.id==task.id&&$0.runId==task.runId}){floatingReviews.append(task)}
            }
        }
        let floatingTransition=wasFloating || floatingTasks.tracking
        let completed=previous?.threadId == state.threadId && state.threadId != nil && (previous?.activeTurnId != nil || previous?.viewedActivity.scene == "thinking") && state.activeTurnId == nil && state.viewedActivity.scene == "off"
        if sendingPrompt != nil {
            if EmbeddedCodeRuntime.shared.active {
                if let reply=sendingPromptRequestID.flatMap(state.requestResult){
                    sendingPrompt=nil;sendingPromptRequestID=nil
                    if let error=reply.error{miniUI.setCodexMessage(error);if compactOnSend && compactWhileWorking{compactWhileWorking=false;compactThreadID=nil;setOpen(true,mini:true)}}
                    else{synchronizeComposerDraft()}
                } else if previous?.ownerToken != state.ownerToken {
                    // Changing chats retires only this view's pending control;
                    // the old owner continues executing its submitted command.
                    sendingPrompt=nil;sendingPromptRequestID=nil
                }
            } else if (state.threadId?.hasPrefix("code:") == true ? state.codeAcceptedRequestId != nil && state.codeAcceptedRequestId == sendingPromptRequestID : state.notice == "Turn started" || state.notice == "Sent to voice" || state.notice.hasPrefix("Steered active turn")) {setPrompt("");miniUI.clearImages();sendingPrompt=nil;sendingPromptRequestID=nil}
            else if state.notice != previousNotice {sendingPrompt=nil;sendingPromptRequestID=nil;if compactOnSend && compactWhileWorking{compactWhileWorking=false;compactThreadID=nil;setOpen(true,mini:true)}}
        }
        if state.threadId?.hasPrefix("code:") == true,state.notice.hasPrefix("Prompt staged in Code"),state.notice != previousNotice{compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)}
        if !dictating && !miniListening && !chatVoice.listening {showActivity(displayActivity(state))}
        if state.approval{updateCompactApproval(state)}
        let legacyCompletion = !EmbeddedCodeRuntime.shared.active && (completed || !Set(previous?.activeTasks.map(\.id) ?? []).subtracting(state.activeTasks.map(\.id)).isEmpty)
        if legacyCompletion || confirmedCompletion {
            sounds.playScene("success");setMotion("jumping",replay:true)
        }
        else if previous?.activity.scene != "failure" && state.activity.scene == "failure" {setMotion("failed",replay:true)}
        else {syncMotion()}
        if !quickChatVisible,!workspaceMorph.expanded,!workspaceMorph.transitioning,!floatingTransition,!approvalPopout,compactWhileWorking,state.threadId == compactThreadID {
            if completed && (!EmbeddedCodeRuntime.shared.active || confirmedCompletion) {
                let output=latestOutputCitation(in:Array(state.history.dropFirst(max(0,compactStartHistoryCount-1))))
                compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)
                setMotion("jumping",replay:true)
                if let output,!compactVoiceMode{miniUI.previewFile(output)}
            }
        }
        if !quickChatVisible && !workspaceMorph.expanded && !workspaceMorph.transitioning && !floatingTransition && !approvalPopout && compactWhileWorking && !autoCompact && !passiveVoice && state.activeTasks.isEmpty && previous?.activeTasks.isEmpty == false && !miniOpen && (!EmbeddedCodeRuntime.shared.active || confirmedCompletion) {
            compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true);setMotion("jumping",replay:true)
        }
        if let request=expandedChatRequest {
            if let reply=state.requestResult(request.id) {
                finishExpandedChatLoading()
                if let error=reply.error{miniUI.setCodexMessage(error)}
                else if state.threadId != request.target {miniUI.setCodexMessage(state.notice)}
            } else {miniUI.setCodexMessage("Loading \(request.name.isEmpty ? "chat":request.name)…")}
        }
        let wasVoiceActive=chatVoice.active
        chatVoice.receive(state)
        if compactVoiceMode,wasVoiceActive,!chatVoice.active {closeNotch(keepExpanded:miniOpen)}
        if voicePrompt.active,let source=voiceCaptureSource,state.threadId != source {failCompactVoice("Chat changed while recording; hold the shortcut to try again")}
        prepareRecordedVoice();renderCompactVoice()
        if let error=state.voice?.historyError{miniUI.setCodexMessage(error)}
        miniUI.finishPending(requestID:miniUI.pendingRequestID.flatMap(state.requestResult)?.id)
        if let pendingFilesRequestID,state.requestResult(pendingFilesRequestID) != nil {self.pendingFilesRequestID=nil;indicator.setFileLoading(false)}
        if !state.approval{updateCompactApproval(state)}
        renderInputIndicator();syncWorkspacePet();updateNinjaWallpaper();reviewNextFloatingTask()
        if (legacyCompletion || confirmedCompletion),!state.approval,!dictating,!compactVoiceMode,!voicePrompt.active,!quickChatVisible {
            completionIndicatorWork?.cancel();currentScene="success";indicator.showScene("success",looping:false)
            let finish=DispatchWorkItem{[weak self] in guard let self,self.currentScene == "success",let state=self.latestState else{return};self.currentScene="";self.showActivity(self.displayActivity(state))}
            completionIndicatorWork=finish;DispatchQueue.main.asyncAfter(deadline:.now()+1.05,execute:finish)
        }
        if !quickChatVisible && !state.activeTasks.isEmpty && !open {autoCompact=true;compactWhileWorking=true;setOpen(true,mini:false)}
        if state.activeTasks.isEmpty && autoCompact && !miniOpen {autoCompact=false;DispatchQueue.main.asyncAfter(deadline:.now()+1.2){[weak self] in guard let self,self.latestState?.activeTasks.isEmpty == true,!self.miniOpen,!self.dictating,!self.finalizingDictation,!(self.workspaceMorph.expanded && self.workspaceNotchOverlay) else{return};self.compactWhileWorking=false;self.setOpen(false)}}
        // Working is derived from the owner snapshot, not a success toast.
        // Failed/stopped/disconnected tasks also retire compact work; a delayed
        // or deduplicated completion notification must never keep it alive.
        if !workspaceNeedsStatus && sendingPrompt == nil {
            compactWhileWorking=false;compactThreadID=nil
        }
        if EmbeddedCodeRuntime.shared.active{workspaceFocusChanged()}
        reconcilePassiveStatus()
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
        shortcutWork?.cancel();shortcutWork=nil;pendingShortcutHold=nil;shortcutKeyCount=nil;shortcutGeneration=UUID()
    }
    private func completeShortcutHold(at time:TimeInterval){
        if let count=shortcutKeyCount,count != keyDownCount(){cancelModifierShortcut();return}
        guard let hold=pendingShortcutHold,time>=hold.deadline,
              hold.generation == shortcutGeneration,miniOpen == hold.expanded,
              shortcutPreferences.action(for:hold.trigger,expanded:hold.expanded) == hold.action else{return}
        shortcutWork?.cancel();shortcutWork=nil;pendingShortcutHold=nil
        if hold.action.isVoice {
            // This hold was armed to stop an existing call. If that call ended
            // while the queue was busy, it must not start a replacement call.
            if latestState?.workspaceVoiceActive == true || compactVoiceMode && (chatVoice.active || voicePrompt.active || shortcutPreparation.busy || pendingVoiceStart != nil){endCompactVoice()}
        }else{startShortcut(hold.action,trigger:hold.trigger)}
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
        if EmbeddedCodeRuntime.shared.active{speech.cancelAudio();bridge.send("workspace_voice");return}
        cancelShortcutHold()
        if compactVoiceMode || chatVoice.active{endCompactVoice();return}
        if shortcutPreparation.busy {cancelShortcutInput();miniUI.setCodexMessage("Voice startup cancelled");return}
        startChatVoice()
    }
    private func startChatVoice(appshot:ApplicationContext?=nil,initialPrompt:Data?=nil){
        if EmbeddedCodeRuntime.shared.active {
            speech.cancelAudio()
            var fields:[String:Any]=[:]
            if let initialPrompt{fields["audio"]=initialPrompt.base64EncodedString()}
            if let appshot{fields["appshot"]=["text":appshot.text,"paths":appshot.imagePath.map{[$0]} ?? []]}
            let request=UUID().uuidString;workspaceVoiceRequest=request;fields["requestId"]=request
            voiceLaunchTimeout?.cancel()
            let timeout=DispatchWorkItem{[weak self] in guard let self,self.workspaceVoiceRequest == request else{return};self.failCompactVoice("Voice did not start. Open the workspace to check the provider connection.")}
            voiceLaunchTimeout=timeout;DispatchQueue.main.asyncAfter(deadline:.now()+20,execute:timeout)
            bridge.send("workspace_voice_prompt",fields);renderCompactVoice();return
        }
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
    private var mediaSuppressed:Bool {approvalPopout || compactVoiceMode || voicePrompt.active || chatVoice.active || miniOpen || dictating || finalizingDictation || pendingAutoSend || miniListening || compactWhileWorking || experimentalWebMode || sendingPrompt != nil || latestState?.activeTurnId != nil || latestState?.approval == true || !(latestState?.activeTasks.isEmpty ?? true)}
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
        guard mediaVisible else{return};mediaVisible=false;currentScene="";media.capture(false);presentedMediaKey=nil;visualTransition.hideArtwork(mediaArtwork);sprite.isHidden=false;visualTransition.reveal(sprite,visible:true);play();if let state=latestState{showActivity(displayActivity(state))};if closeNotch && !miniOpen && !compactWhileWorking && !dictating && !finalizingDictation{setOpen(false)}
    }
    private var activeNinjaStyle:String?{
        guard let state=latestState else{return nil}
        if miniOpen || compactVoiceMode || quickChatVisible || workspaceIsForeground{return state.personalNinja?.style}
        return state.activeTasks.first{$0.id == (floatingFocus ?? compactPetCycleID ?? state.focusThreadId)}?.ninjaStyle
    }
    private func syncConversationTheme(){
        let style=activeNinjaStyle
        let id=style.map{"ninja:"+$0} ?? ConversationTheme.petID(expanded:miniOpen || compactVoiceMode,viewedThread:latestState?.threadId,taskPets:taskPets,indicatorPet:petID,fallback:userPetID)
        guard conversationPetID != id,let profile=style.map({PetGradientProfile.ninja($0)}) ?? gradientCatalog.profileByPetID[id] else{return}
        conversationPetID=id;indicator.setProfile(profile,wave:true);miniUI.setProfile(profile,animated:true)
        assistantPalette=style.map{let c=NinjaAtmosphereView.color($0);return [c.blended(withFraction:0.45,of:.white)!,c,c.blended(withFraction:0.38,of:.black)!]} ?? assistantGradientColors(for:id)
        if let storage=conversationText?.textStorage{ConversationTheme.recolor(storage,palette:assistantPalette)}
        updateGlassBackground()
        syncInlinePets()
    }
    private func petAssignmentTaskIDs(for id:String)->[String]{
        if id.hasPrefix("floating-preview:"){return floatingTasks.trackedTasks.map(\.id)}
        return (latestState?.activeTasks ?? []).filter{$0.scene != "off"}.map(\.id)
    }
    private func reconcileTaskPets(priority:String){
        taskPets=TaskPetAssignments.reconcile(taskPets,taskIDs:petAssignmentTaskIDs(for:priority),available:petSelection.automaticIDs(catalog.pets.map(\.id)),preferred:petID,explicit:explicitTaskPets,priority:priority)
    }
    private func syncCompactPetCycle(_ state:KaiState){
        let eligible=state.activeTasks.filter{$0.scene != "off"}
        guard open,!miniOpen,!compactVoiceMode,!state.approval,state.workspaceVoiceActive != true,floatingFocus==nil,eligible.count>1 else{compactPetCycle?.cancel();compactPetCycle=nil;compactPetCycleID=nil;return}
        if !eligible.contains(where:{$0.id==compactPetCycleID}){compactPetCycleID=eligible.first?.id}
        guard compactPetCycle==nil else{return}
        let work=DispatchWorkItem{[weak self] in
            guard let self else{return};self.compactPetCycle=nil
            guard let state=self.latestState,self.open,!self.miniOpen,!self.compactVoiceMode,!state.approval,state.workspaceVoiceActive != true,self.floatingFocus==nil else{self.compactPetCycleID=nil;return}
            let tasks=state.activeTasks.filter{$0.scene != "off"};guard tasks.count>1 else{self.compactPetCycleID=nil;return}
            let current=tasks.firstIndex(where:{$0.id==self.compactPetCycleID}) ?? 0
            self.compactPetCycleID=tasks[(current+1)%tasks.count].id
            self.syncTaskPet(state);self.showActivity(self.displayActivity(state))
            if !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{let fade=CATransition();fade.type = .fade;fade.duration=0.3;self.sprite.add(fade,forKey:"familyPetChange")}
        };compactPetCycle=work;DispatchQueue.main.asyncAfter(deadline:.now()+3.2,execute:work)
    }
    private func syncTaskPet(_ state:KaiState){
        syncCompactPetCycle(state)
        let ids=Set(state.activeTasks.map(\.id)),pets=petSelection.automaticIDs(catalog.pets.map(\.id))
        if let floatingFocus,!ids.contains(floatingFocus){self.floatingFocus=nil}
        taskPets=TaskPetAssignments.reconcile(taskPets,taskIDs:state.activeTasks.reversed().filter{$0.scene != "off"}.map(\.id),available:pets,preferred:petID,explicit:explicitTaskPets)
        if let viewed=state.threadId,taskPets[viewed] == nil {taskPets[viewed]=pets.contains(userPetID) ? userPetID : (pets.first ?? userPetID)}
        if miniOpen || compactVoiceMode,let viewed=state.threadId,let assigned=taskPets[viewed] {
            if petID != assigned {petID=assigned;currentScene="";play()}
        } else if let focus=floatingFocus ?? compactPetCycleID ?? state.focusThreadId {
            let assigned=taskPets[focus] ?? petID
            if focusedTaskID != focus || petID != assigned {focusedTaskID=focus;petID=assigned;currentScene="";play()}
        } else {
            focusedTaskID=nil
        }
        let focused=state.activeTasks.first(where:{$0.id == (floatingFocus ?? compactPetCycleID ?? state.focusThreadId)})
        let style=(miniOpen ? state.personalNinja?.style:nil) ?? focused?.ninjaStyle
        if ninjaSpriteStyle != style{play()}
        syncConversationTheme()
        syncInlinePets()
    }
    private func syncInlinePets(){
        guard let catalog,let state=latestState else{return}
        let selected=conversationPetID.isEmpty ? petID:conversationPetID
        var seen=Set<String>();let owners=(state.activeTasks.filter{$0.id.hasPrefix("child:")}.map(\.id)+taskPets.keys.filter{$0.hasPrefix("child:")}.sorted()).filter{seen.insert($0).inserted}.prefix(127)
        var assignments=owners.compactMap{owner in taskPets[owner].map{(key:owner,value:$0)}};assignments.insert((key:"selected",value:selected),at:0)
        var visuals=[String:InlineTaskPet](),payload=[String:[String:Any]]()
        for (owner,petID) in assignments {
            guard let pet=catalog.pets.first(where:{$0.id==petID}) else{continue}
            let scene=state.activeTasks.first{$0.id==owner}?.scene ?? "off",motionID=scene=="off" ? "idle":["thinking","waiting","permission","question"].contains(scene) ? "waiting":"running"
            guard let motion=pet.animationOverrides?[motionID] ?? catalog.stateAnimations[motionID],let timeline=SpriteTimeline(row:motion.row,rows:pet.grid.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop) else{continue}
            let url=pet.sourceURL ?? assetRoot.appendingPathComponent(pet.spritesheet),key=url.path+":"+motionID
            let stops=gradientCatalog.profileByPetID[petID]?.gradients.ambient.stops ?? []
            var colors=stops.map{stop->NSColor in let n=UInt64(stop.color.dropFirst(),radix:16) ?? 0;return NSColor(srgbRed:CGFloat((n>>16)&255)/255,green:CGFloat((n>>8)&255)/255,blue:CGFloat(n&255)/255,alpha:1)}
            if colors.isEmpty{colors=[.white]};colors.sort{($0.usingColorSpace(.sRGB)?.brightnessComponent ?? 0)>($1.usingColorSpace(.sRGB)?.brightnessComponent ?? 0)}
            let radial=[colors.first!.blended(withFraction:0.2,of:.white) ?? colors.first!,colors[colors.count/2].blended(withFraction:0.16,of:.black) ?? colors[colors.count/2],colors.last!.blended(withFraction:0.58,of:.black) ?? colors.last!]
            let palette=radial.map{color->UInt32 in let c=color.usingColorSpace(.sRGB) ?? .white;return UInt32((c.redComponent*255).rounded())<<16 | UInt32((c.greenComponent*255).rounded())<<8 | UInt32((c.blueComponent*255).rounded())}
            payload[owner]=["path":url.path,"rows":pet.grid.rows,"row":motion.row,"columns":motion.columns,"durationsMs":motion.durationsMs,"colors":palette]
            if let image=inlinePetImages.image(at:url){visuals[owner]=inlinePetVisuals[owner].flatMap{$0.key==key ? $0:nil} ?? InlineTaskPet(key:key,image:image,timeline:timeline)}
            else if inlinePetLoading.insert(url).inserted{inlinePetImages.load(url){[weak self] image in guard let self else{return};self.inlinePetLoading.remove(url);if image != nil{self.syncInlinePets()}}}
        }
        inlinePetVisuals=visuals;miniUI.setTaskPets(visuals)
        if conversationRenderer.setTaskPets(visuals),miniOpen{renderConversation(state.history)}
        if EmbeddedCodeRuntime.shared.active,let bytes=try? JSONSerialization.data(withJSONObject:payload,options:.sortedKeys),bytes != lastPetVisualPayload{lastPetVisualPayload=bytes;bridge.send("pet_visuals",["pets":payload])}
    }
    private func closeNotch(keepExpanded:Bool=false){
        dictationTextReveal.finish()
        passiveVoice=passiveVoice || chatVoice.active || (EmbeddedCodeRuntime.shared.active && latestState?.workspaceVoiceActive == true)
        beforeVoiceShortcut=nil;voiceShortcutArmed=false
        voiceExpanded=false;voiceApprovalMenuItem?.isHidden=true
        compactVoiceMode=chatVoice.active || (EmbeddedCodeRuntime.shared.active && latestState?.workspaceVoiceActive == true)
        compactVoiceError=nil;compactVoiceIndicator=nil;voiceCaptureSource=nil;currentScene=""
        if let path=pendingVoiceStart?.context?.imagePath{attachmentFiles.append(URL(fileURLWithPath:path))}
        voicePrompt.cancel();if workspaceVoiceRequest == nil{voiceLaunchTimeout?.cancel();voiceLaunchTimeout=nil};pendingVoiceStart=nil
        cancelShortcutHold();shortcutPreparation.cancel()
        hidePetGallery(restore:false)
        pendingAutoSend=false;finalizingDictation=false;dictationGeneration=UUID()
        if dictating{finishDictation(allowAutoSend:false)}
        pinned=false
        if keepExpanded {
            compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)
        } else if workspaceNeedsStatus {
            passiveCompact=true;autoCompact=false;compactWhileWorking=true;compactThreadID=nil
            setOpen(true,mini:false,userInitiated:true);if let state=latestState{showActivity(displayActivity(state))}
        } else {compactWhileWorking=false;compactThreadID=nil;setOpen(false)}
        reconcilePassiveStatus()
    }
    private func reconcilePassiveStatus(){
        guard passiveCompact,!miniOpen else{passiveRetire?.cancel();passiveRetire=nil;return}
        if workspaceNeedsStatus{passiveRetire?.cancel();passiveRetire=nil;return}
        guard passiveRetire == nil else{return}
        let finish=DispatchWorkItem{[weak self] in
            guard let self else{return};self.passiveRetire=nil
            guard self.passiveCompact,!self.miniOpen,!self.workspaceNeedsStatus else{return}
            self.passiveCompact=false;self.passiveVoice=false;self.compactWhileWorking=false;self.compactThreadID=nil;self.autoCompact=false
            self.compactVoiceMode=false;self.currentScene="";self.indicator.hide();self.setOpen(false)
        };passiveRetire=finish;DispatchQueue.main.asyncAfter(deadline:.now()+0.35,execute:finish)
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

    private func renderInputIndicator(){
        guard open else{return}
        if dictating || voicePrompt.phase == .recording {
            if currentScene != "input:listening"{currentScene="input:listening";indicator.startListening(useMicrophone:false)}
        }else if finalizingDictation || shortcutPreparation.busy || pendingVoiceStart != nil || workspaceVoiceRequest != nil || pendingAutoSend {
            let scene=workspaceVoiceRequest != nil ? "workspace-voice:connecting":"input:thinking"
            if currentScene != scene{currentScene=scene;indicator.showThinking()}
        }
    }
    private func showActivity(_ activity:KaiActivity){
        if dictating || finalizingDictation || miniListening || voicePrompt.active || shortcutPreparation.busy || workspaceVoiceRequest != nil{renderInputIndicator();return}
        if compactVoiceMode{renderCompactVoice();return}
        guard open else{return}
        if EmbeddedCodeRuntime.shared.active,latestState?.workspaceVoiceActive == true{
            let scene=latestState?.workspaceVoiceScene ?? "thinking",key="workspace-voice:"+scene
            if currentScene != key{currentScene=key;if ["listening","speaking"].contains(scene){indicator.startListening(useMicrophone:false)}else{indicator.showThinking()}}
            return
        }
        if passiveCompact,activity.scene == "off",!workspaceNeedsStatus{currentScene="";indicator.hide();return}
        if mediaVisible && activity.scene == "off"{return}
        if currentScene == "success" && activity.scene == "off"{return}
        let provider=latestState?.activeTasks.first{$0.id == latestState?.threadId}?.agent ?? latestState?.selectedAgent ?? "codex"
        let scene=EmbeddedCodeRuntime.shared.active && activity.scene == "off" ? provider:IntegrationIcons.id(for:activity.text) ?? activity.scene
        guard currentScene != scene else{return};currentScene=scene;sounds.playScene(activity.scene)
        if EmbeddedCodeRuntime.shared.active && activity.scene == "off"{indicator.showScene(provider,looping:false)}
        else{switch activity.scene{case "off":indicator.hide();case "listening":indicator.startListening(useMicrophone:false);case "thinking":indicator.showThinking();default:indicator.showScene(scene)}}
    }

    private func setMotion(_ id:String,replay:Bool=false){
        guard catalog.stateAnimations[id] != nil,(motionID != id || replay) else{return}
        motionID=id;play();refreshChecks()
    }
    private func syncMotion(force:Bool=false){
        if compactVoiceMode{renderCompactVoice();return}
        if !force && ["waving","running-left","jumping","failed"].contains(motionID){return}
        let motion:String
        let activity=latestState.map{displayActivity($0)}
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
    private func renderConversation(_ history:[KaiMessage],forceBottom:Bool=false,preserveViewport:Bool=false,deferReveal:Bool=false){
        guard let view=conversationText,let scroll=conversationScroll else{return}
        // Most conversations need no WebKit process. Start it only for equations.
        if miniOpen,renderMath,!mathRenderer.isInstalled,history.contains(where:{MathExpression.containsRenderableMath($0.lines)}),mathRenderer.install(in:scalableUI,below:scroll) {
            conversationRenderer.invalidateAll()
        }
        let anchor=ConversationViewport.anchor(view,in:scroll)
        historyControl.update(hasMore:latestState?.historyHasMore == true,loading:earlierHistoryRequest != nil || latestState?.historyLoading == true,earlier:latestState?.historyEarlier == true)
        let folding=conversationFold.pending
        let followBottom = !folding && (forceBottom || ConversationViewport.isAtBottom(view,in:scroll))
        let changed=conversationRenderer.render(history,threadID:latestState?.threadId,in:view,scroll:scroll,renderMath:renderMath,palette:assistantPalette,anchorCharacter:anchor?.character,collectRevealRanges:morphingText && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion)
        guard conversationRenderer.didChange || forceBottom else{return}
        ConversationViewport.layout(view,in:scroll)
        if preserveViewport {ConversationViewport.revealOlderText(view,characters:conversationRenderer.prependedCharacterCount)}
        if !forceBottom,let anchor,(preserveViewport || !followBottom || folding) {
            ConversationViewport.restore(anchor,adding:(conversationRenderer.mappedAnchorCharacter ?? anchor.character)-anchor.character,text:view,scroll:scroll)
        } else if changed && followBottom || forceBottom {scrollConversationToBottom(threadID:latestState?.threadId)}
        if folding{conversationFold.finish(in:scroll,threadID:latestState?.threadId)}
        if morphingText,!preserveViewport,!deferReveal{
            textReveal.reveal(conversationRenderer.revealRanges,in:view,loading:conversationRenderer.didChangeThread)
        }
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
    func textView(_ textView:NSTextView,clickedOnLink link:Any,at charIndex:Int)->Bool{guard let url=(link as? URL) ?? (link as? String).flatMap(URL.init(string:)) else{return false};if url.scheme == "kai-ninja-file",let path=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "path"})?.value{bridge.send("ninja_artifact",["path":path]);return true};if url.scheme == "kai-interaction",url.host == "details",let id=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "id"})?.value,latestState?.history.contains(where:{$0.detailId == id}) == true{if id.hasPrefix("work-"),let scroll=conversationScroll{conversationFold.begin(in:scroll,threadID:latestState?.threadId)};bridge.send("interaction_details",["id":id]);if !id.hasPrefix("work-"){expandWorkspace()};return true};if url.scheme == "kai-file-action",let action=url.host,let path=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "path"})?.value,path.hasPrefix("/"){fileActions.perform(action,url:URL(fileURLWithPath:path));return true};if url.scheme == "kai-reply",let answer=URLComponents(url:url,resolvingAgainstBaseURL:false)?.queryItems?.first(where:{$0.name == "answer"})?.value{NSPasteboard.general.clearContents();NSPasteboard.general.setString(answer,forType:.string);return true};if url.isFileURL{miniUI.previewFile(url);return true};guard ["http","https"].contains(url.scheme?.lowercased() ?? "") else{return false};NSWorkspace.shared.open(url);return true}

    private func beginDictation(trigger:DictationTrigger){
        revealNotchForInput()
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
            if !self.pendingAutoSend,self.sendingPrompt == nil,let state=self.latestState{self.showActivity(self.displayActivity(state))}
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
    private func globalDrag(_ event:NSEvent){guard !experimentalWebMode,let screen=surfaceScreen else{return};let p=NSEvent.mouseLocation,nearNotch=abs(p.x-screen.frame.midX)<notchWidth/2+45 && p.y>screen.frame.maxY-surfaceTopInset-notchHeight-70 && p.y<screen.frame.maxY-surfaceTopInset;if event.type == .leftMouseDragged,nearNotch{let images=draggedImages(NSPasteboard(name:.drag));guard !images.isEmpty else{return};approachImages=images;if !approachActive{approachActive=true;handleImageEntered()}}else if event.type == .leftMouseUp{defer{approachActive=false;approachImages.removeAll()};if approachActive,nearNotch,!approachImages.isEmpty{handleImageDrop(approachImages)}}}

    @objc private func changeMediaSize(_ slider:NSSlider){mediaSize=CGFloat(slider.doubleValue);UserDefaults.standard.set(Double(mediaSize),forKey:"mediaSize");layoutMediaArtwork(animated:true)}
    private func layoutMediaArtwork(animated:Bool){
        guard let screen=surfaceScreen else{return}
        let expanded=expandedWidth,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2
        let center=CGPoint(x:(physicalLeft+kaiLeft)/2,y:canvasHeight-notchHeight/2)
        let frame=NotchVisualTransition.artworkFrame(size:mediaSize,notchHeight:notchHeight,center:center,scale:screen.backingScaleFactor)
        visualTransition.resize(mediaArtwork,to:frame,animated:animated)
    }
    private func layoutPet() {
        // Preparing the hidden notch must not move the pet still owned by Code.
        guard sprite.superlayer !== workspacePet.layer else{return}
        guard let screen=surfaceScreen else{return};let expanded=expandedWidth,physicalLeft=(canvasWidth-notchWidth)/2,kaiLeft=(canvasWidth-expanded)/2,scale=screen.backingScaleFactor,topBandY=canvasHeight-notchHeight,h=min(36,notchHeight-3)*petScale,w=h*192/208
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
        indicator.moveAccents(toBottom:canvasHeight-(bottom ? popoutHeight:notchHeight),expanded:bottom,duration:duration)
    }

    private func showNinjaSprite(style:String){
        let mark=MenuBarMark(kind:"ninja",id:style,name:"Ninja",style:style)
        guard let image=MenuBarIdentityMenu.image(mark),let cg=image.cgImage(forProposedRect:nil,context:nil,hints:nil)else{return}
        let animate=miniOpen && latestState?.personalNinja != nil && workspaceMorph.motionEnabled && !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        if animate,ninjaSpriteStyle==style,sprite.animation(forKey:"ninjaLoop") != nil{return}
        sprite.removeAnimation(forKey:"spriteFrames");sprite.removeAnimation(forKey:"ninjaLoop")
        ninjaSpriteStyle=style;playToken += 1;CATransaction.begin();CATransaction.setDisableActions(true);sprite.contents=cg;sprite.contentsRect=CGRect(x:0,y:0,width:1,height:1);sprite.isHidden=false;sprite.opacity=1;CATransaction.commit()
        if animate {
            if ninjaFrames[style]==nil,let url=Bundle.main.resourceURL?.appendingPathComponent("providers/ninja-"+style+".gif"),let source=CGImageSourceCreateWithURL(url as CFURL,nil){ninjaFrames[style]=(0..<min(24,CGImageSourceGetCount(source))).compactMap{CGImageSourceCreateImageAtIndex(source,$0,nil)}}
            if let frames=ninjaFrames[style],frames.count>1{let loop=CAKeyframeAnimation(keyPath:"contents");loop.values=frames;loop.duration=Double(frames.count)*0.08;loop.repeatCount = .infinity;loop.calculationMode = .discrete;sprite.add(loop,forKey:"ninjaLoop")}
        }
    }

    private func play() {
        let task=latestState?.activeTasks.first(where:{$0.id == (floatingFocus ?? compactPetCycleID ?? latestState?.focusThreadId)})
        if let ninja=latestState?.personalNinja,miniOpen {showNinjaSprite(style:ninja.style);return}
        if let style=task?.ninjaStyle{showNinjaSprite(style:style);return}
        ninjaSpriteStyle=nil;sprite.removeAnimation(forKey:"ninjaLoop")
        guard let pet = catalog.pets.first(where: { $0.id == petID }), let motion = pet.animationOverrides?[motionID] ?? catalog.stateAnimations[motionID] else{return}
        let image:CGImage
        if loadedSpritePetID == petID,let loadedSprite {image=loadedSprite}
        else {
            let url=pet.sourceURL ?? assetRoot.appendingPathComponent(pet.spritesheet)
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
        petSelection.lastUsedID=petID
        petAppIcon.play(petID:petID,sheet:image,rows:pet.grid.rows,row:motion.row,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop)
        if mediaVisible {return}
        if renderedPetID != petID && !renderedPetID.isEmpty {let blend=CATransition();blend.type = .fade;blend.duration=0.36;blend.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.7,0.25,1);sprite.add(blend,forKey:"petBlend")}
        CATransaction.begin(); CATransaction.setDisableActions(true); sprite.contents = image;sprite.isHidden=false;sprite.opacity=1; CATransaction.commit()
        renderedPetID=petID
        syncConversationTheme()
        sprite.contentsScale = NSScreen.main?.backingScaleFactor ?? 2; frame = 0; playToken += 1
        showFrame(pet, motion, playToken);syncWorkspacePet();if petGalleryVisible{petGallery.play(motionID)}
    }

    private func showFrame(_ pet: Pet, _ motion: Motion, _ token: Int) {
        guard token == playToken,open || compactWhileWorking || experimentalWebMode || workspaceMorph.expanded || quickChatVisible,
              let timeline=SpriteTimeline(row:motion.row,rows:pet.grid.rows,columns:motion.columns,durationsMs:motion.durationsMs,loops:motion.loop) else{return}
        sprite.removeAnimation(forKey:"spriteFrames")
        let reduceMotion=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion
        CATransaction.begin();CATransaction.setDisableActions(true)
        sprite.contentsRect=motion.loop || reduceMotion ? timeline.frames[0]:timeline.frames[timeline.frames.count-1]
        CATransaction.commit()
        if !reduceMotion,timeline.frames.count>1 {sprite.add(timeline.animation(),forKey:"spriteFrames")}
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
    @objc private func toggleFromMenu() { cancelSizeTest();if EmbeddedCodeRuntime.shared.active,workspaceMorph.expanded,!miniOpen{prepareNotchOverlay();pinned=true;compactWhileWorking=false;setOpen(true,mini:true,userInitiated:true);return};if compactVoiceMode{setOpen(true,mini:!miniOpen,userInitiated:true);return};if compactWhileWorking {compactWhileWorking=false;compactThreadID=nil;pinned=true;setOpen(true,mini:true)}else if open{closeNotch()}else{pinned=true;setOpen(true,mini:true)} }
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
    private func usePet(_ id:String,explicit:Bool=true){if explicit{petSelection.selectedID=id};userPetID=id;petID=id;if let chatID=miniOpen ? latestState?.threadId : (focusedTaskID ?? latestState?.threadId){taskPets[chatID]=id;if explicit{explicitTaskPets.insert(chatID)};reconcileTaskPets(priority:chatID)};syncConversationTheme();petGallery.layout(in:petGallery.layer.bounds,selected:id);refreshChecks();play();floatingTasks.refresh(pets:taskPets)}
    @objc private func closePetPicker(){status.menu?.cancelTracking()}
    @objc private func toggleTextMorph(){
        morphingText.toggle();textRevealPreferences.enabled=morphingText
        if EmbeddedCodeRuntime.shared.active{bridge.send("preferences",["textMorph":morphingText])}
        textReveal.finish();refreshChecks()
    }
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
    @objc private func toggleBottomGlass(){if codeWallpaperEnabled{codeWallpaperEnabled=false;UserDefaults.standard.set(false,forKey:"notchCodeWallpaper");glassEnabled=true}else{glassEnabled.toggle()};UserDefaults.standard.set(glassEnabled,forKey:"notchBottomGlass");placePanel();refreshChecks()}
    @objc private func toggleCodeWallpaper(){codeWallpaperEnabled.toggle();UserDefaults.standard.set(codeWallpaperEnabled,forKey:"notchCodeWallpaper");placePanel();refreshChecks()}
    func setWorkspaceWallpaper(_ image:CGImage?,opacity:Float){workspaceBackground.setImage(image,opacity:opacity);AdaptiveTextBackdrop.shared.nativeBackgroundChanged()}
    func ninjaAtmosphereGeometry(_ rect:CGRect){ninjaWorkspaceSlot=rect;updateNinjaWallpaper()}
    private func updateNinjaWallpaper(){
        guard let state=latestState,let ninja=state.personalNinja else{ninjaScene.suspend();return}
        let host:NSView?,rect:CGRect
        if quickChatVisible{host=quickWindow?.contentView;rect=host?.bounds ?? .zero}
        else if miniOpen && showsNotchSurface{host=scalableUI;rect=scalableUI.bounds}
        else if workspaceMorph.expanded && !workspaceNotchOverlay{
            host=EmbeddedCodeRuntime.shared.workspaceWindow?.contentView
            rect=host?.isFlipped == true ? ninjaWorkspaceSlot:CGRect(x:ninjaWorkspaceSlot.minX,y:(host?.bounds.height ?? 0)-ninjaWorkspaceSlot.maxY,width:ninjaWorkspaceSlot.width,height:ninjaWorkspaceSlot.height)
        }else{ninjaScene.suspend();return}
        guard let host,rect.width>0,rect.height>0 else{ninjaScene.suspend();return}
        ninjaScene.mount(in:host,frame:rect)
        ninjaScene.configure(identity:ninja.host+":"+ninja.id+":"+(state.threadId ?? ""),name:ninja.name,style:ninja.style,working:state.activeTurnId != nil || state.workspaceVoiceActive == true,motion:workspaceMorph.motionEnabled,visible:true,bottomAnchored:miniOpen && showsNotchSurface && !quickChatVisible)
    }
    private func updateGlassBackground(){
        updateNinjaWallpaper()
        guard let root=panel?.contentView?.layer else{return}
        let wallpaper=codeWallpaperEnabled && EmbeddedCodeRuntime.shared.active && latestState?.personalNinja == nil
        let visible=open && miniOpen && !experimentalWebMode
        let enabled=glassEnabled && !wallpaper && visible
        root.backgroundColor=enabled ? NSColor.clear.cgColor:NSColor.black.cgColor
        glassBackground.update(canvas:CGSize(width:canvasWidth,height:canvasHeight),width:expandedWidth,enabled:enabled,color:assistantPalette.first ?? .systemBlue,preferences:GlassPreferences(defaults:.standard))
        workspaceBackground.update(canvas:CGSize(width:canvasWidth,height:canvasHeight),width:expandedWidth,enabled:wallpaper&&visible,height:GlassPreferences(defaults:.standard).height)
        EmbeddedCodeRuntime.shared.setWallpaperVisible(wallpaper&&visible)
        if let panel {
            AdaptiveTextBackdrop.shared.registerNotch(window:panel,host:scalableUI,area:miniUI.layer.frame,active:visible&&showsNotchSurface,frozen:workspaceMorph.transitioning||quickMorph.transitioning){[weak self,weak panel] point,external in
                guard let self,let panel else{return external}
                let rootPoint=self.scalableUI.convert(point,to:panel.contentView)
                var base=enabled ? external:[0.0,0.0,0.0]
                base=self.glassBackground.adaptiveComposite(rootPoint,base:base)
                return self.workspaceBackground.adaptiveComposite(rootPoint,base:base)
            }
        }
        AdaptiveTextBackdrop.shared.nativeBackgroundChanged()
    }
    private func refreshChecks() {
        codeConnectionItem?.state=(UserDefaults.standard.object(forKey:"connectCode") as? Bool ?? true) ? .on:.off
        petFilterItem?.state=petSelection.excludeCodexPets ? .on:.off
        glassItem?.state=glassEnabled && !codeWallpaperEnabled ? .on:.off
        codeWallpaperItem?.state=codeWallpaperEnabled ? .on:.off
        for (key,slider) in glassSliders{slider.isEnabled = !codeWallpaperEnabled || key=="height"}
        glassChoiceItems.forEach{$0.isEnabled = !codeWallpaperEnabled}
        experimentalModeItem?.state=experimentalWebMode ? .on:.off
        textMorphItem?.state=morphingText ? .on:.off
        mathModeItem?.state=renderMath ? .on:.off
        soundsItem?.state=soundsEnabled ? .on:.off
        petMenuRows.forEach { $0.value.selected = $0.key == petID }; motionItems.forEach { $0.value.state = $0.key == motionID ? .on:.off }
        indicatorItems.forEach { $0.value.state = $0.key == indicatorID ? .on:.off }
        accentItems.forEach { $0.value.state = $0.key == accentSide ? .on:.off }
        showItem?.title = compactVoiceMode ? (miniOpen ? "compact notch":"expand notch"):(open ? "hide notch":"show notch")
    }
}

#if !KAI_UNIFIED
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

#endif
