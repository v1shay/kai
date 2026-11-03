import AppKit

@MainActor enum ProviderIcons {
    static let names=["codex","claude","gemini","opencode","cursor","grok","antigravity","ninja"]
    static func identifier(_ value:String?)->String? {
        guard let raw=value?.lowercased() else{return nil}
        let text=["cursor-agent":"cursor","agy":"antigravity"][raw] ?? raw
        return names.first{ text == $0 || text.hasPrefix($0+" ·") || text.hasSuffix(":"+$0) }
    }
    private static let cache=NSCache<NSString,NSImage>()
    static func image(_ value:String?,color:NSColor = .white)->NSImage?{
        guard let id=identifier(value) else{return nil};return mark(id,color:color)
    }
    static func mark(_ id:String,color:NSColor = .white)->NSImage?{
        let key=(id+color.description) as NSString
        if let image=cache.object(forKey:key){return image}
        let root=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let url=Bundle.main.resourceURL?.appendingPathComponent("providers/\(id).svg")
        guard let svg=url.flatMap(NSImage.init(contentsOf:)) ?? NSImage(contentsOf:root.appendingPathComponent("integrations/Code/assets/\(id).svg")) else{return nil}
        let image=NSImage(size:CGSize(width:24,height:24),flipped:false){rect in svg.draw(in:rect);if id != "codex"{color.setFill();rect.fill(using:.sourceIn)};return true}
        cache.countLimit=48;cache.setObject(image,forKey:key);return image
    }
}

/// The very same animated sprite layer is reparented here while the IDE is open.
@MainActor final class WorkspacePetView:NSView {
    var onClick:(()->Void)?
    override init(frame:NSRect){super.init(frame:frame);wantsLayer=true;setAccessibilityRole(.button);setAccessibilityLabel("Kai task pet — choose a pet")}
    required init?(coder:NSCoder){fatalError()}
    override func mouseDown(with event:NSEvent){onClick?()}
    override func accessibilityPerformPress()->Bool{onClick?();return true}
}
