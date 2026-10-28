import AppKit

/// Installed families are resolved by AppKit, including fonts added in Font Book.
/// Enumeration is demand-driven; shaping never scans the filesystem.
enum AppTypography {
    static var interfaceFamily=UserDefaults.standard.string(forKey:"interfaceFont") ?? "System"
    static var codeFamily=UserDefaults.standard.string(forKey:"codeFont") ?? "Menlo"
    static func families()->[String]{Array(Set(NSFontManager.shared.availableFontFamilies)).filter{!$0.hasPrefix(".")}.sorted{$0.localizedStandardCompare($1) == .orderedAscending}}
    static func ui(ofSize size:CGFloat,weight:NSFont.Weight = .regular)->NSFont{
        resolve(interfaceFamily,size:size,weight:weight) ?? NSFont.systemFont(ofSize:size,weight:weight)
    }
    static func code(ofSize size:CGFloat,weight:NSFont.Weight = .regular)->NSFont{
        resolve(codeFamily,size:size,weight:weight) ?? NSFont.monospacedSystemFont(ofSize:size,weight:weight)
    }
    private static func resolve(_ family:String,size:CGFloat,weight:NSFont.Weight)->NSFont?{
        guard !["System",".AppleSystemUIFont",""].contains(family) else{return nil}
        return NSFontManager.shared.font(withFamily:family,traits:[],weight:min(15,max(0,Int((5+weight.rawValue*5).rounded()))),size:size)
    }
    static func apply(interface:String,code:String)->Bool{
        guard interface != interfaceFamily || code != codeFamily else{return false}
        interfaceFamily=interface;codeFamily=code;return true
    }
    static func update(_ root:CALayer){
        if let text=root as? CATextLayer {
            let old=text.font as? NSFont
            let mono=old?.fontDescriptor.symbolicTraits.contains(.monoSpace) == true
            let weight=old?.fontDescriptor.symbolicTraits.contains(.bold) == true ? NSFont.Weight.semibold:.regular
            text.font=mono ? Self.code(ofSize:text.fontSize,weight:weight):Self.ui(ofSize:text.fontSize,weight:weight)
            if let original=text.string as? NSAttributedString{
                let value=NSMutableAttributedString(attributedString:original)
                original.enumerateAttribute(.font,in:NSRange(location:0,length:original.length)){font,range,_ in
                    guard let font=font as? NSFont else{return};let weight:NSFont.Weight=font.fontDescriptor.symbolicTraits.contains(.bold) ? .semibold:.regular
                    value.addAttribute(.font,value:font.fontDescriptor.symbolicTraits.contains(.monoSpace) ? Self.code(ofSize:font.pointSize,weight:weight):Self.ui(ofSize:font.pointSize,weight:weight),range:range)
                };text.string=value
            }
        }
        root.sublayers?.forEach(update)
    }
}

@MainActor final class TypographyMenu:NSObject,NSMenuDelegate {
    var onSelect:((String,String)->Void)?
    let item=NSMenuItem(title:"Fonts",action:nil,keyEquivalent:"")
    private let ui=NSMenu(title:"Interface font"),code=NSMenu(title:"Code font")
    override init(){
        super.init();let root=NSMenu();item.submenu=root
        for (title,menu) in [("Interface font",ui),("Code font",code)]{let row=NSMenuItem(title:title,action:nil,keyEquivalent:"");row.submenu=menu;menu.delegate=self;root.addItem(row)}
    }
    func menuNeedsUpdate(_ menu:NSMenu){
        menu.removeAllItems();let slot=menu === ui ? "interface":"code",selected=slot == "interface" ? (AppTypography.interfaceFamily == ".AppleSystemUIFont" ? "System":AppTypography.interfaceFamily):AppTypography.codeFamily
        for family in ["System"]+AppTypography.families(){
            let row=menu.addItem(withTitle:family,action:#selector(select(_:)),keyEquivalent:"");row.target=self;row.representedObject=[slot,family];row.state=family == selected ? .on:.off
        }
    }
    @objc private func select(_ sender:NSMenuItem){guard let value=sender.representedObject as? [String],value.count == 2 else{return};onSelect?(value[0],value[1])}
}
