import AppKit

extension NSAttributedString.Key {
    static let kaiGradientPosition = NSAttributedString.Key("kaiGradientPosition")
}

@MainActor enum ConversationTheme {
    static func petID(expanded:Bool,viewedThread:String?,taskPets:[String:String],indicatorPet:String,fallback:String)->String {
        expanded ? viewedThread.flatMap{taskPets[$0]} ?? fallback : indicatorPet
    }
    /// A theme change only changes ink. Keep glyphs, attachments and selection intact.
    static func recolor(_ storage:NSTextStorage,palette:[NSColor]) {
        guard !palette.isEmpty else{return}
        var runs=[(NSRange,NSColor)]()
        storage.enumerateAttribute(.kaiGradientPosition,in:NSRange(location:0,length:storage.length)) { value,range,_ in
            guard let fraction=value as? CGFloat else{return}
            let index=min(palette.count-1,max(0,Int((fraction*CGFloat(palette.count-1)).rounded())))
            runs.append((range,palette[index]))
        }
        var cards=[NSRange]()
        storage.enumerateAttribute(.kaiInteractionTint,in:NSRange(location:0,length:storage.length)){value,range,_ in if value != nil{cards.append(range)}}
        storage.beginEditing()
        for (range,color) in runs {storage.addAttribute(.foregroundColor,value:color,range:range)}
        for range in cards{storage.addAttribute(.kaiInteractionTint,value:palette[0],range:range)}
        storage.endEditing()
    }
}
