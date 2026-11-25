import AppKit

@MainActor enum IntegrationIcons {
    static let names=["googleslides","googlesheets","googledocs","googledrive","googlecalendar","gmail","notion","github","spotify","supabase","vercel","linear","dropbox","figma","box","jira","confluence","youtube","slack","linkedin"]
    static func id(for text:String)->String? {
        let text=text.lowercased().replacingOccurrences(of:"_",with:"").replacingOccurrences(of:"-",with:"").replacingOccurrences(of:" ",with:"")
        if text.contains("docs.google.com/presentation"){return "googleslides"}
        if text.contains("docs.google.com/spreadsheets"){return "googlesheets"}
        if text.contains("docs.google.com/document"){return "googledocs"}
        if text.contains("google_drive") || text.contains("googledrive") || text.contains("drive.google.com"){return "googledrive"}
        return names.first{ text.contains($0) }
    }
    private static let cache=NSCache<NSString,NSImage>()
    static func image(_ id:String,color:NSColor = .white)->NSImage? {
        let key="\(id)-\(color.description)" as NSString
        if let image=cache.object(forKey:key){return image}
        let bundled=Bundle.main.resourceURL?.appendingPathComponent("integrations/\(id).svg")
        let source=URL(fileURLWithPath:#filePath).deletingLastPathComponent().appendingPathComponent("Resources/integrations/\(id).svg")
        guard let svg=NSImage(contentsOf:bundled ?? source) ?? NSImage(contentsOf:source) else{return nil}
        let image=NSImage(size:CGSize(width:24,height:24),flipped:false){rect in
            svg.draw(in:rect);color.setFill();rect.fill(using:.sourceIn);return true
        }
        cache.countLimit=128;cache.setObject(image,forKey:key);return image
    }
}
