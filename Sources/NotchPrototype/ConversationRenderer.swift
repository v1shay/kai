import AppKit

/// Owns message ranges so a streaming update or finished equation replaces only
/// its message. AppKit layout is deferred until the complete batch is applied.
@MainActor final class ConversationRenderer {
    private static let pathRegex=try? NSRegularExpression(pattern:#"(?:file://)?(?:/Users/|Users/|~/)[^\s\]\[\)\(>,]+"#)
    private let mathRenderer:MathRenderer
    private var renderedThreadID:String?,renderedBlocks=[KaiMessage](),renderedRanges=[NSRange]()
    private var renderedMessageIDs=[String]()
    private var mathBlocks=[String:Set<Int>](),dirtyMathBlocks=Set<Int>()
    private(set) var lastReplacedBlockCount=0
    private(set) var prependedCharacterCount=0
    private(set) var mappedAnchorCharacter:Int?
    private(set) var didChange=false
    var hasDirtyMath:Bool {!dirtyMathBlocks.isEmpty}
    init(mathRenderer:MathRenderer){self.mathRenderer=mathRenderer}
    func invalidateMath(_ latex:String){dirtyMathBlocks.formUnion(mathBlocks[latex] ?? [])}
    func invalidateAll(){renderedBlocks.removeAll()}
    func refreshFileAction(_ action:String,url:URL,in view:NSTextView){
        guard let storage=view.textStorage,let link=FileActionSymbols.link(action,url:url) else{return}
        storage.enumerateAttribute(.link,in:NSRange(location:0,length:storage.length)){value,range,_ in
            guard value as? URL == link else{return}
            storage.enumerateAttribute(.attachment,in:range){value,attachmentRange,_ in
                guard let attachment=value as? NSTextAttachment else{return}
                attachment.image=FileActionSymbols.image(action,url:url,padded:true)
                view.layoutManager?.invalidateDisplay(forCharacterRange:attachmentRange)
            }
        }
    }
    func render(_ history:[KaiMessage],threadID:String?,in view:NSTextView,scroll:NSScrollView,renderMath:Bool,palette assistantPalette:[NSColor],anchorCharacter:Int?=nil)->Bool {
        lastReplacedBlockCount=0;prependedCharacterCount=0;mappedAnchorCharacter=anchorCharacter;didChange=false
        let changedThread=threadID != renderedThreadID
        if changedThread {mathRenderer.beginDocument();renderedThreadID=threadID;renderedBlocks.removeAll();renderedRanges.removeAll();mathBlocks.removeAll();dirtyMathBlocks.removeAll();view.textStorage?.setAttributedString(NSAttributedString(string:""))}
        // Swift arrays and strings share storage. Compare the decoded models
        // directly instead of copying the entire transcript into signatures.
        let signatures=history
        guard signatures != renderedBlocks || !dirtyMathBlocks.isEmpty else{return false}
        didChange=true
        (view as? FileActionHoverTextView)?.clearFileActionHover()
        // Keep the already displayed blocks intact when older exchanges arrive.
        let added=signatures.count-renderedBlocks.count
        let prependCount = !changedThread && !renderedBlocks.isEmpty && added>0 && Array(history.dropFirst(added).map{$0.id}) == renderedMessageIDs ? added:0
        let selection=view.selectedRanges
        if prependCount>0 {
            renderedBlocks.insert(contentsOf:Array(history.prefix(prependCount)),at:0)
            renderedRanges.insert(contentsOf:Array(repeating:NSRange(location:0,length:0),count:prependCount),at:0)
            mathBlocks=mathBlocks.mapValues{Set($0.map{$0+prependCount})}
            dirtyMathBlocks=Set(dirtyMathBlocks.map{$0+prependCount})
        }
        let nearBottom=changedThread || (scroll.verticalScroller?.floatValue ?? 1) > 0.93
        let pathRegex=Self.pathRegex
        let palette=assistantPalette.isEmpty ? [NSColor.white] : assistantPalette
        guard let storage=view.textStorage else{return false}
        var ranges=renderedRanges
        storage.beginEditing()
        defer{
            storage.endEditing()
            if prependCount>0 {
                prependedCharacterCount=ranges[prependCount-1].location+ranges[prependCount-1].length
                view.selectedRanges=selection.map{value in var range=value.rangeValue;range.location += prependedCharacterCount;return NSValue(range:range)}
            }
        }
        for (index,block) in history.enumerated() {
            if index >= prependCount,index < renderedBlocks.count,index < ranges.count,signatures[index] == renderedBlocks[index],!dirtyMathBlocks.contains(index){continue}
            let content=NSMutableAttributedString(string:"")
            let blockStart=index < ranges.count ? ranges[index].location : storage.length
            for key in Array(mathBlocks.keys) {mathBlocks[key]?.remove(index);if mathBlocks[key]?.isEmpty == true{mathBlocks.removeValue(forKey:key)}}
            let processedLines=ConversationInteraction.lines(block.lines)
            var visualLines=renderMath ? MathExpression.collapsedLines(processedLines) : processedLines
            // Message formatters add empty separators; use one measured gap instead.
            while visualLines.last?.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty == true {visualLines.removeLast()}
            while visualLines.first?.text.trimmingCharacters(in:.whitespacesAndNewlines).isEmpty == true {visualLines.removeFirst()}
            let displayed=visualLines.map{ $0.style != "code" ? displayCitations(in:$0.text) : ($0.text,[(NSRange,URL)]())}
            let totalCharacters=max(1,displayed.reduce(0){$0+$1.0.count+1})
            var characterOffset=0
            for (lineIndex,pair) in zip(visualLines,displayed).enumerated() {
            let (line,rendered)=pair
            let displayText=rendered.0
            let lineCharacterStart=characterOffset
            let card=line.style.hasPrefix("interaction:")
            let cardParts=line.style.split(separator:":")
            let cardRole=cardParts.last.map(String.init) ?? ""
            let style=line.style,base=NSFont.systemFont(ofSize:9),font:NSFont
            switch style {case "activity":font=NSFontManager.shared.convert(NSFont.systemFont(ofSize:8),toHaveTrait:.italicFontMask);case "heading":font=NSFont.systemFont(ofSize:9,weight:.semibold);case "code":font=NSFont.monospacedSystemFont(ofSize:8,weight:.regular);default:font=base}
            let paragraph=NSMutableParagraphStyle();paragraph.lineSpacing=2
            paragraph.paragraphSpacing=lineIndex == visualLines.count-1 ? 5:0
            if block.kind == "activity" {paragraph.headIndent=17}
            let start=content.length
            var attributes:[NSAttributedString.Key:Any]=[.font:font,.paragraphStyle:paragraph]
            if card {
                paragraph.headIndent=12;paragraph.firstLineHeadIndent=12;paragraph.tailIndent = -12
                paragraph.paragraphSpacing=["answer","document"].contains(cardRole) ? 12:4
                paragraph.paragraphSpacingBefore=(["title","document"].contains(cardRole) || cardRole.hasPrefix("voice")) ? 9:0
                attributes[.kaiInteractionCard]=cardParts.dropLast().joined(separator:":")
                attributes[.kaiInteractionTint]=palette.first!
                attributes[.font]=NSFont.systemFont(ofSize:(cardRole == "title" || cardRole.hasPrefix("voice")) ? 8:9,weight:cardRole == "answer" ? .medium:.regular)
                if cardRole == "answer" {
                    var url=URLComponents();url.scheme="kai-reply";url.host="copy";url.queryItems=[URLQueryItem(name:"answer",value:displayText)]
                    attributes[.link]=url.url
                    attributes[.toolTip]="Click to copy this answer"
                }
            }
            if block.kind == "assistant" && !card {
                content.append(NSAttributedString(string:displayText+"\n",attributes:attributes))
                var utf16Offset=0,runStart=0,runColorIndex:Int?
                for character in displayText {
                    let index=min(palette.count-1,characterOffset*(palette.count-1)/max(1,totalCharacters-1))
                    if let runColorIndex,runColorIndex != index {
                        content.addAttributes([.foregroundColor:palette[runColorIndex],.kaiGradientPosition:CGFloat(runColorIndex)/CGFloat(max(1,palette.count-1))],range:NSRange(location:start+runStart,length:utf16Offset-runStart))
                        runStart=utf16Offset
                    }
                    runColorIndex=index
                    utf16Offset += character.utf16.count
                    characterOffset += 1
                }
                if let runColorIndex {content.addAttributes([.foregroundColor:palette[runColorIndex],.kaiGradientPosition:CGFloat(runColorIndex)/CGFloat(max(1,palette.count-1))],range:NSRange(location:start+runStart,length:utf16Offset-runStart))}
                let newlineColorIndex=min(palette.count-1,characterOffset*(palette.count-1)/max(1,totalCharacters-1))
                content.addAttributes([.foregroundColor:palette[newlineColorIndex],.kaiGradientPosition:CGFloat(newlineColorIndex)/CGFloat(max(1,palette.count-1))],range:NSRange(location:start+utf16Offset,length:1))
            } else {
                content.append(NSAttributedString(string:displayText+"\n",attributes:attributes.merging([.foregroundColor:cardRole == "document" ? palette.first! : NSColor.white.withAlphaComponent(block.kind == "activity" ? 0.42:0.86)]){$1}))
            }
            if cardRole == "document",let url=rendered.1.first?.1,let image=IntegrationIcons.id(for:url.absoluteString).flatMap{IntegrationIcons.image($0,color:palette.first!)} ?? NSImage(systemSymbolName:"doc",accessibilityDescription:"Document")?.withSymbolConfiguration(.init(paletteColors:[palette.first!])),displayText.hasPrefix("▧") {
                let attachment=NSTextAttachment();attachment.image=image;attachment.bounds=CGRect(x:0,y:-2,width:9,height:9)
                content.replaceCharacters(in:NSRange(location:start,length:1),with:NSAttributedString(attachment:attachment))
                content.addAttributes(attributes,range:NSRange(location:start,length:1))
            }
            if line.style.hasPrefix("voice:"),cardRole != "continuation" {
                let symbol=["voiceprompt":"mic.fill","voicetranscript":"waveform","voicestatus":"ellipsis.circle","voicecomplete":"checkmark.circle.fill"][cardRole] ?? "waveform"
                let attachment=NSTextAttachment();attachment.image=NSImage(systemSymbolName:symbol,accessibilityDescription:displayText)?.withSymbolConfiguration(.init(paletteColors:[palette.first!]))
                attachment.bounds=CGRect(x:0,y:-1,width:9,height:9)
                let prefix=NSMutableAttributedString(attachment:attachment);prefix.append(NSAttributedString(string:"  "))
                prefix.addAttributes(attributes,range:NSRange(location:0,length:prefix.length));content.insert(prefix,at:start)
            }
            characterOffset += 1
            for (range,url) in rendered.1 {content.addAttributes([.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue,.toolTip:url.isFileURL ? url.path:url.absoluteString],range:NSRange(location:start+range.location,length:range.length))}
            for match in (block.kind == "activity" ? [] : pathRegex?.matches(in:displayText,range:NSRange(displayText.startIndex...,in:displayText)) ?? []) {
                let raw=(displayText as NSString).substring(with:match.range).trimmingCharacters(in:CharacterSet(charactersIn:".,;:"))
                let path=raw.hasPrefix("Users/") ? "/"+raw:raw
                let url=raw.hasPrefix("file://") ? URL(string:raw) : URL(fileURLWithPath:(path as NSString).expandingTildeInPath)
                if let url,FileManager.default.fileExists(atPath:url.path) {
                    var link:[NSAttributedString.Key:Any]=[.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue]
                    if block.kind != "assistant" {link[.foregroundColor]=NSColor.white}
                    content.addAttributes(link,range:NSRange(location:start+match.range.location,length:(raw as NSString).length))
                }
            }
            if style != "code" && style != "activity" {
                for match in webLinkDetector.matches(in:displayText,range:NSRange(displayText.startIndex...,in:displayText)) {
                    guard let url=match.url,["http","https"].contains(url.scheme?.lowercased() ?? "") else{continue}
                    content.addAttributes([.link:url,.underlineStyle:NSUnderlineStyle.single.rawValue],range:NSRange(location:start+match.range.location,length:match.range.length))
                }
            }
            if cardRole == "document",let url=rendered.1.first?.1,url.isFileURL {
                let actions=NSMutableAttributedString(string:"")
                for action in ["reveal","copy"] {
                    let actionStart=actions.length
                    actions.append(NSAttributedString(string:"   ",attributes:attributes))
                    let attachment=NSTextAttachment();attachment.image=FileActionSymbols.image(action,url:url,padded:true)
                    attachment.bounds=CGRect(x:0,y:-6,width:24,height:22)
                    let button=NSMutableAttributedString(attachment:attachment)
                    button.addAttributes(attributes,range:NSRange(location:0,length:button.length))
                    if let link=FileActionSymbols.link(action,url:url){button.addAttributes([.link:link,.toolTip:action == "copy" ? "Copy file":"Show in Finder"],range:NSRange(location:0,length:button.length))}
                    actions.append(button)
                    if let link=FileActionSymbols.link(action,url:url){actions.addAttributes([.link:link,.toolTip:action == "copy" ? "Copy file":"Show in Finder"],range:NSRange(location:actionStart,length:actions.length-actionStart))}
                }
                content.insert(actions,at:start+(displayText as NSString).length)
            }
            if renderMath && style != "code" && line.mathEnabled != false {
                for math in MathExpression.find(in:displayText).reversed() {
                    mathBlocks[math.latex,default:[]].insert(index)
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
            if let iconID=IntegrationIcons.id(for:block.lines.map(\.text).joined(separator:" ")),block.kind == "activity",let image=IntegrationIcons.image(iconID,color:palette.first!) {
                let attachment=NSTextAttachment();attachment.image=image;attachment.bounds=CGRect(x:0,y:-2,width:9,height:9)
                let prefix=NSMutableAttributedString(attachment:attachment);prefix.append(NSAttributedString(string:"  "));if content.length>0{prefix.addAttributes(content.attributes(at:0,effectiveRange:nil),range:NSRange(location:0,length:prefix.length))};content.insert(prefix,at:0)
            } else if block.kind == "activity",content.length>0 {
                let attachment=NSTextAttachment();attachment.image=ConversationActivityIcon.image(block.activityIcon ?? "tool")
                attachment.bounds=CGRect(x:0,y:-1,width:14,height:9)
                let prefix=NSMutableAttributedString(attachment:attachment)
                prefix.append(NSAttributedString(string:" ",attributes:content.attributes(at:0,effectiveRange:nil)))
                prefix.addAttributes(content.attributes(at:0,effectiveRange:nil),range:NSRange(location:0,length:prefix.length))
                content.insert(prefix,at:0)
            }
            let oldRange=index < ranges.count ? ranges[index] : NSRange(location:blockStart,length:0)
            lastReplacedBlockCount += 1
            let delta=content.length-oldRange.length
            if let anchor=mappedAnchorCharacter,anchor>=NSMaxRange(oldRange) {
                mappedAnchorCharacter=anchor+delta
            }
            storage.replaceCharacters(in:oldRange,with:content)
            let newRange=NSRange(location:blockStart,length:content.length)
            if index < ranges.count {
                ranges[index]=newRange
                for next in (index+1)..<ranges.count {ranges[next].location += delta}
            } else {ranges.append(newRange)}
        }
        if ranges.count > history.count {
            let start=ranges[history.count].location
            storage.deleteCharacters(in:NSRange(location:start,length:storage.length-start))
            ranges.removeSubrange(history.count...)
        }
        renderedBlocks=signatures;renderedMessageIDs=history.map{$0.id};renderedRanges=ranges;dirtyMathBlocks.removeAll()
        return nearBottom
    }
}
