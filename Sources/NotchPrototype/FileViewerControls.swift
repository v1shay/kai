import AppKit

enum FileActionSymbols {
    static func updateHover(_ action:String?,targets:[(CGRect,String)],backdrop:CALayer,animated:Bool=true){
        let old=backdrop.presentation()?.opacity ?? backdrop.opacity
        CATransaction.begin();CATransaction.setDisableActions(true)
        backdrop.backgroundColor=NSColor.white.withAlphaComponent(0.16).cgColor
        if let index=targets.firstIndex(where:{$0.1 == action}) {
            let target=targets[index].0
            // Keep the painted shape centered even when adjacent hit areas are close.
            let spacing=targets.enumerated().filter{$0.offset != index}.map{abs($0.element.0.midX-target.midX)}.min() ?? 22
            let side=min(20,max(0,spacing-1))
            backdrop.frame=CGRect(x:target.midX-side/2,y:target.midY-side/2,width:side,height:side)
            backdrop.cornerRadius=side/2;backdrop.opacity=1
        }else{backdrop.opacity=0}
        CATransaction.commit()
        if animated{let fade=CABasicAnimation(keyPath:"opacity");fade.fromValue=old;fade.toValue=backdrop.opacity;fade.duration=0.12;backdrop.add(fade,forKey:"fileActionHover")}
    }
    private static var confirmations=[String:TimeInterval]()
    private static func key(_ action:String,_ url:URL)->String{action+"|"+url.standardizedFileURL.absoluteString}
    @discardableResult static func confirm(_ action:String,url:URL)->TimeInterval {
        let active=feedbackProgress(action,url:url)>0
        let stamp=ProcessInfo.processInfo.systemUptime-(active ? 0.16:0)
        confirmations[key(action,url)]=stamp;return stamp
    }
    static func confirmation(_ action:String,url:URL)->TimeInterval?{confirmations[key(action,url)]}
    static func clear(_ action:String,url:URL,stamp:TimeInterval){if confirmation(action,url:url) == stamp{confirmations.removeValue(forKey:key(action,url))}}
    static func feedbackProgress(_ action:String,url:URL,now:TimeInterval=ProcessInfo.processInfo.systemUptime)->CGFloat {
        guard let stamp=confirmation(action,url:url) else{return 0}
        let elapsed=max(0,now-stamp)
        return CGFloat(min(1,elapsed/0.16)*max(0,min(1,(1.3-elapsed)/0.2)))
    }
    private static let finder:NSImage?=NSWorkspace.shared.urlForApplication(withBundleIdentifier:"com.apple.finder").map{NSWorkspace.shared.icon(forFile:$0.path)}
    static func image(_ action:String,url:URL?=nil,padded:Bool=false)->NSImage? {
        let original=(action == "reveal" ? finder:nil) ?? NSImage(systemSymbolName:action == "reveal" ? "folder":"doc.on.clipboard",accessibilityDescription:action == "reveal" ? "Show in Finder":"Copy file")?.withSymbolConfiguration(.init(paletteColors:[NSColor.white.withAlphaComponent(0.8)]))
        let progress=url.map{feedbackProgress(action,url:$0)} ?? 0
        let check=NSImage(systemSymbolName:"checkmark",accessibilityDescription:"Done")?.withSymbolConfiguration(.init(paletteColors:[NSColor.systemGreen]))
        let size=padded ? CGSize(width:24,height:22):CGSize(width:12,height:12)
        return NSImage(size:size,flipped:false){_ in
            let rect=CGRect(x:(size.width-12)/2,y:(size.height-12)/2,width:12,height:12)
            original?.draw(in:rect,from:.zero,operation:.sourceOver,fraction:1-progress)
            check?.draw(in:rect,from:.zero,operation:.sourceOver,fraction:progress)
            return true
        }
    }
    static func link(_ action:String,url:URL)->URL? {
        var value=URLComponents();value.scheme="kai-file-action";value.host=action
        value.queryItems=[URLQueryItem(name:"path",value:url.path)];return value.url
    }
}

@MainActor final class FileActionHoverTextView:NSTextView {
    private var actionTracking:NSTrackingArea?
    private(set) var fileActionHoverRect:CGRect?
    private(set) var hoveredFileAction:URL?
    override func updateTrackingAreas(){
        super.updateTrackingAreas()
        if let actionTracking{removeTrackingArea(actionTracking)}
        let tracking=NSTrackingArea(rect:.zero,options:[.inVisibleRect,.activeAlways,.mouseMoved,.mouseEnteredAndExited],owner:self)
        addTrackingArea(tracking);actionTracking=tracking
    }
    func clearFileActionHover(){
        fileActionHoverRect=nil;hoveredFileAction=nil;needsDisplay=true
    }
    override func drawBackground(in rect:NSRect){
        super.drawBackground(in:rect)
        guard let highlight=fileActionHoverRect else{return}
        NSColor.white.withAlphaComponent(0.16).setFill()
        NSBezierPath(roundedRect:highlight,xRadius:highlight.height/2,yRadius:highlight.height/2).fill()
    }
    func updateFileActionHover(at point:CGPoint?){
        clearFileActionHover()
        guard let point,let storage=textStorage,storage.length>0,let manager=layoutManager,let container=textContainer else{return}
        let local=CGPoint(x:point.x-textContainerOrigin.x,y:point.y-textContainerOrigin.y)
        let glyph=manager.glyphIndex(for:local,in:container)
        guard glyph<manager.numberOfGlyphs else{return}
        let character=manager.characterIndexForGlyph(at:glyph)
        var range=NSRange()
        guard character<storage.length,let link=storage.attribute(.link,at:character,longestEffectiveRange:&range,in:NSRange(location:0,length:storage.length)) as? URL,link.scheme == "kai-file-action" else{return}
        let glyphs=manager.glyphRange(forCharacterRange:range,actualCharacterRange:nil)
        guard manager.boundingRect(forGlyphRange:glyphs,in:container).contains(local) else{return}
        // The link includes leading spaces for easier clicking. Paint only around
        // the attachment, so those spaces cannot shift or square off the highlight.
        storage.enumerateAttribute(.attachment,in:range){value,attachmentRange,stop in
            guard value is NSTextAttachment else{return}
            let attachmentGlyphs=manager.glyphRange(forCharacterRange:attachmentRange,actualCharacterRange:nil)
            let attachmentRect=manager.boundingRect(forGlyphRange:attachmentGlyphs,in:container)
            fileActionHoverRect=CGRect(x:attachmentRect.midX-10+textContainerOrigin.x,y:attachmentRect.midY-10+textContainerOrigin.y,width:20,height:20)
            stop.pointee=true
        }
        hoveredFileAction=link;needsDisplay=true;NSCursor.pointingHand.set()
    }
    override func mouseMoved(with event:NSEvent){super.mouseMoved(with:event);updateFileActionHover(at:convert(event.locationInWindow,from:nil))}
    override func mouseEntered(with event:NSEvent){super.mouseEntered(with:event);updateFileActionHover(at:convert(event.locationInWindow,from:nil))}
    override func mouseExited(with event:NSEvent){super.mouseExited(with:event);clearFileActionHover()}
    override func viewDidHide(){super.viewDidHide();clearFileActionHover()}
}

@MainActor final class FileActionsMenu:NSObject {
    var onPerformed:((String,URL)->Void)?
    var reveal:([URL])->Void={urls in
        let workspace=NSWorkspace.shared
        guard let finder=workspace.urlForApplication(withBundleIdentifier:"com.apple.finder") else{return}
        let configuration=NSWorkspace.OpenConfiguration();configuration.activates=true
        workspace.openApplication(at:finder,configuration:configuration){application,error in
            guard error == nil else{return}
            DispatchQueue.main.async {
                workspace.activateFileViewerSelecting(urls)
                application?.activate(options:.activateIgnoringOtherApps)
            }
        }
    }
    func menu(for url:URL)->NSMenu {
        let menu=NSMenu();menu.autoenablesItems=false
        let available=url.isFileURL && FileManager.default.fileExists(atPath:url.path)
        for (title,action) in [("Copy file",#selector(copyFile(_:))),("Show in Finder",#selector(showInFinder(_:)))] {
            let item=menu.addItem(withTitle:title,action:action,keyEquivalent:"")
            item.target=self;item.representedObject=url;item.isEnabled=available
        }
        return menu
    }
    @discardableResult func copy(_ url:URL,to pasteboard:NSPasteboard)->Bool {
        guard url.isFileURL,FileManager.default.fileExists(atPath:url.path) else{return false}
        pasteboard.clearContents()
        return pasteboard.writeObjects([url as NSURL])
    }
    @discardableResult func perform(_ action:String,url:URL,pasteboard:NSPasteboard = .general)->Bool {
        guard url.isFileURL,FileManager.default.fileExists(atPath:url.path) else{return false}
        let succeeded:Bool
        if action == "copy"{succeeded=copy(url,to:pasteboard)}
        else if action == "reveal"{reveal([url]);succeeded=true}
        else{return false}
        if succeeded{onPerformed?(action,url)}
        return succeeded
    }
    @objc private func copyFile(_ item:NSMenuItem){if let url=item.representedObject as? URL{perform("copy",url:url)}}
    @objc private func showInFinder(_ item:NSMenuItem){
        guard let url=item.representedObject as? URL,url.isFileURL,FileManager.default.fileExists(atPath:url.path) else{return}
        perform("reveal",url:url)
    }
}

@MainActor private final class FileViewerText:NSTextView {
    var onFileActions:(()->Void)?
    override func mouseDown(with event:NSEvent){
        if event.clickCount>=2 {onFileActions?();return}
        super.mouseDown(with:event)
    }
    override func rightMouseDown(with event:NSEvent){onFileActions?()}
}

/// Native text layout keeps long files scrollable and selectable without a
/// giant Core Animation text texture or the old 3,200-byte preview prefix.
@MainActor final class FileViewerContent:NSScrollView {
    private let text=FileViewerText()
    private(set) var fileURL:URL?
    var onFileActions:((URL)->Void)?
    var displayedText:String{text.string}
    override init(frame:NSRect){
        super.init(frame:frame)
        drawsBackground=false;hasVerticalScroller=true;scrollerStyle = .overlay;borderType = .noBorder
        text.drawsBackground=false;text.isEditable=false;text.isSelectable=true
        text.font=NSFont.monospacedSystemFont(ofSize:8,weight:.regular)
        text.textColor=NSColor.white.withAlphaComponent(0.78);text.textContainerInset=CGSize(width:3,height:3)
        text.isVerticallyResizable=true;text.isHorizontallyResizable=false;text.autoresizingMask=[.width]
        text.minSize = .zero;text.maxSize=CGSize(width:CGFloat.greatestFiniteMagnitude,height:CGFloat.greatestFiniteMagnitude)
        text.frame=CGRect(origin:.zero,size:contentSize)
        text.textContainer?.widthTracksTextView=true;documentView=text
        text.onFileActions={ [weak self] in guard let self,let url=self.fileURL else{return};self.onFileActions?(url)}
    }
    required init?(coder:NSCoder){fatalError()}
    func show(_ value:String,url:URL){
        let changed=fileURL != url || text.string != value
        fileURL=url
        guard changed else{return}
        text.string=value
        text.setSelectedRange(NSRange(location:0,length:0))
        text.sizeToFit()
        contentView.scroll(to:.zero);reflectScrolledClipView(contentView)
    }
}
