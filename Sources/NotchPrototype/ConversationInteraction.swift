import AppKit

/// Only recognized, well-formed interaction envelopes become cards. This data
/// describes an already submitted reply; it never executes an embedded command.
enum ConversationInteraction {
    private static let envelope=try! NSRegularExpression(pattern:"<send_user_message_question_reply>\\s*(.*?)\\s*</send_user_message_question_reply>",options:[.dotMatchesLineSeparators])
    private struct Reply:Decodable {let question:String;let answer:String}
    static func lines(_ source:[KaiLine])->[KaiLine]{
        var output=[KaiLine](),run=[KaiLine]()
        func flush(){
            let text=run.map(\.text).joined(separator:"\n"),ns=text as NSString
            func ordinary(_ range:NSRange)->[KaiLine]{
                var offset=0,result=[KaiLine]()
                for line in run {
                    let length=(line.text as NSString).length
                    let intersection=NSIntersectionRange(range,NSRange(location:offset,length:length))
                    if intersection.length>0 {result += ns.substring(with:intersection).components(separatedBy:"\n").map{KaiLine(text:$0,style:line.style,mathEnabled:line.mathEnabled)}}
                    offset += length+1
                }
                return result
            }
            var cursor=0
            for match in envelope.matches(in:text,range:NSRange(location:0,length:ns.length)){
                guard let data=ns.substring(with:match.range(at:1)).data(using:.utf8),let replies=try? JSONDecoder().decode([Reply].self,from:data),!replies.isEmpty else{continue}
                if match.range.location>cursor {output += ordinary(NSRange(location:cursor,length:match.range.location-cursor))}
                for (index,reply) in replies.enumerated(){
                    let id="\(match.range.location)-\(index)"
                    output.append(KaiLine(text:"✓  Question answered",style:"interaction:\(id):title",mathEnabled:false))
                    output.append(KaiLine(text:reply.question,style:"interaction:\(id):question",mathEnabled:false))
                    output.append(KaiLine(text:reply.answer,style:"interaction:\(id):answer",mathEnabled:false))
                }
                cursor=NSMaxRange(match.range)
            }
            if cursor == 0 {output += run}
            else if cursor<ns.length {output += ordinary(NSRange(location:cursor,length:ns.length-cursor))}
            run.removeAll()
        }
        for line in source {if line.style == "code"{flush();output.append(line)}else{run.append(line)}}
        flush();return citationCards(voiceLines(output))
    }
    private static func voiceLines(_ lines:[KaiLine])->[KaiLine]{
        var result=[KaiLine](),voice=false,group=0,section="",role=""
        let tags=try! NSRegularExpression(pattern:"</?(?:realtime_delegation|input|transcript_delta)>")
        for line in lines {
            guard line.style != "code",!line.style.hasPrefix("interaction:") else{result.append(line);continue}
            var text=line.text
            let opening=text.contains("<realtime_delegation>"),closing=text.contains("</realtime_delegation>")
            if opening {voice=true;section="";group += 1}
            if voice || opening {
                let next=text.contains("<input>") ? "Voice prompt":text.contains("<transcript_delta>") ? "Voice transcript":section
                if !next.isEmpty,next != section {
                    section=next;role=next == "Voice prompt" ? "voiceprompt":"voicetranscript"

                }
                text=tags.stringByReplacingMatches(in:text,range:NSRange(text.startIndex...,in:text),withTemplate:"").trimmingCharacters(in:.whitespacesAndNewlines)
                if !text.isEmpty {result.append(KaiLine(text:text,style:section.isEmpty ? line.style:"voice:\(role)",mathEnabled:false));role="continuation"}
                if closing {voice=false;section=""}
            }else if text.hasPrefix("[STATUS]") || text.hasPrefix("[COMPLETE]") {
                group += 1
                let complete=text.hasPrefix("[COMPLETE]")

                text=String(text.dropFirst(complete ? 10:8)).trimmingCharacters(in:.whitespacesAndNewlines)
                if !text.isEmpty{result.append(KaiLine(text:text,style:complete ? "voice:voicecomplete":"voice:voicestatus",mathEnabled:false))}
            }else{result.append(line)}
        }
        return result
    }
    private static func citationCards(_ lines:[KaiLine])->[KaiLine]{
        var result=[KaiLine]()
        for (index,line) in lines.enumerated(){
            guard line.style != "code",!line.style.hasPrefix("interaction:") else{result.append(line);continue}
            let citations=fileCitations(in:line.text),source=line.text as NSString
            guard !citations.isEmpty else{result.append(line);continue}
            var cursor=0
            for citation in citations {
                let before=source.substring(with:NSRange(location:cursor,length:citation.range.location-cursor)).trimmingCharacters(in:.whitespacesAndNewlines)
                if !before.isEmpty{result.append(KaiLine(text:before,style:line.style,mathEnabled:line.mathEnabled))}
                result.append(KaiLine(text:source.substring(with:citation.range),style:"interaction:document-\(index)-\(citation.range.location):document",mathEnabled:false))
                cursor=NSMaxRange(citation.range)
            }
            let after=source.substring(from:cursor).trimmingCharacters(in:.whitespacesAndNewlines)
            if !after.isEmpty{result.append(KaiLine(text:after,style:line.style,mathEnabled:line.mathEnabled))}
        }
        return result
    }
}

extension NSAttributedString.Key {
    static let kaiInteractionCard=NSAttributedString.Key("KaiInteractionCard")
    static let kaiInteractionTint=NSAttributedString.Key("KaiInteractionTint")
}

/// Draw each card once in text layout coordinates, without adding backdrop
/// views or rebuilding the conversation's view hierarchy during streaming.
final class InteractionLayoutManager:NSLayoutManager {
    weak var cardHost:NSClipView?
    private var surfaces=[Int:NSView]()
    override func drawBackground(forGlyphRange glyphs:NSRange,at origin:NSPoint){
        guard let storage=textStorage,let container=textContainers.first else{super.drawBackground(forGlyphRange:glyphs,at:origin);return}
        for surface in surfaces.values{surface.isHidden=true}
        let characters=characterRange(forGlyphRange:glyphs,actualGlyphRange:nil)
        storage.enumerateAttribute(.kaiInteractionCard,in:characters){value,visibleRange,_ in
            guard value != nil else{return}
            var range=NSRange()
            _=storage.attribute(.kaiInteractionCard,at:visibleRange.location,longestEffectiveRange:&range,in:NSRange(location:0,length:storage.length))
            var inkRange=range
            while inkRange.length>0,(storage.string as NSString).substring(with:NSRange(location:NSMaxRange(inkRange)-1,length:1)) == "\n" {inkRange.length -= 1}
            let glyphRange=self.glyphRange(forCharacterRange:inkRange,actualCharacterRange:nil)
            var rect=self.boundingRect(forGlyphRange:glyphRange,in:container)
            rect.origin.x += origin.x-9;rect.origin.y += origin.y-5
            rect.size.width=min(container.size.width-6,max(30,rect.size.width+18));rect.size.height += 10
            let path=NSBezierPath(roundedRect:rect,xRadius:10,yRadius:10)
            let tint=(storage.attribute(.kaiInteractionTint,at:range.location,effectiveRange:nil) as? NSColor) ?? .systemBlue
            if let host=self.cardHost,let textView=container.textView {
                let surface:NSView
                if let existing=self.surfaces[range.location]{surface=existing}
                else if #available(macOS 26.0,*),let type=NSClassFromString("NSGlassEffectView") as? NSView.Type{surface=type.init(frame:.zero)}
                else {surface=NSVisualEffectView()}
                if surface.superview == nil {
                    if let frost=surface as? NSVisualEffectView{frost.material = .hudWindow;frost.blendingMode = .behindWindow;frost.state = .active}
                    surface.appearance=NSAppearance(named:.darkAqua);surface.wantsLayer=true
                    surface.layer?.cornerRadius=10;surface.layer?.masksToBounds=true
                    host.addSubview(surface,positioned:.below,relativeTo:textView)
                    self.surfaces[range.location]=surface
                }
                surface.isHidden=false
                surface.frame=textView.convert(rect,to:host)
                surface.layer?.backgroundColor=tint.withAlphaComponent(0.10).cgColor
                surface.layer?.borderColor=tint.blended(withFraction:0.45,of:.white)?.withAlphaComponent(0.30).cgColor
                surface.layer?.borderWidth=0.5
            }else{
                tint.withAlphaComponent(0.12).setFill();path.fill()
                tint.withAlphaComponent(0.30).setStroke();path.lineWidth=0.5;path.stroke()
            }
        }
        for key in Array(surfaces.keys) where surfaces[key]?.isHidden == true{surfaces.removeValue(forKey:key)?.removeFromSuperview()}
        super.drawBackground(forGlyphRange:glyphs,at:origin)
    }
}
