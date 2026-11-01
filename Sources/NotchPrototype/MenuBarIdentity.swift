import AppKit

struct MenuBarNinja:Codable,Equatable {
    let id:String,name:String,style:String,host:String
}
struct MenuBarMark:Codable,Equatable {
    let kind:String,id:String,name:String,style:String
    static let kai=MenuBarMark(kind:"kai",id:"kai",name:"Kai Unified",style:"")
    @MainActor static func provider(_ id:String)->Self{Self(kind:"provider",id:id,name:MenuBarIdentityMenu.providers.first{$0.0==id}?.1 ?? id,style:"")}
    init(ninja:MenuBarNinja){kind="ninja";id=ninja.id;name=ninja.name;style=ninja.style}
    init(kind:String,id:String,name:String,style:String){self.kind=kind;self.id=id;self.name=name;self.style=style}
}

/// Native, persisted identity choices. Updates only when the existing owner
/// snapshot changes; no timers, provider probes, or extra status items.
@MainActor final class MenuBarIdentityMenu:NSObject {
    static let providers=[("codex","Codex"),("claude","Claude Code"),("gemini","Gemini CLI"),("opencode","OpenCode"),("cursor","Cursor"),("grok","Grok Build"),("antigravity","Antigravity"),("ninja","Ninja")]
    private static let ninjaImages=NSCache<NSString,NSImage>()
    private static let choiceKey="menuBar.identity",lastKey="menuBar.lastAgent",pinnedKey="menuBar.pinnedNinja"
    let menu=NSMenu()
    var onChange:((MenuBarMark)->Void)?
    private let defaults:UserDefaults
    private var ninjas=[MenuBarNinja]()
    private var lastAgent:MenuBarMark
    private var pinnedNinja:MenuBarMark?
    private var delivered:MenuBarMark?
    var selection:String{
        let value=defaults.string(forKey:Self.choiceKey) ?? "kai"
        if value=="kai" || value=="current"{return value}
        if let id=value.removingPrefix("provider:"),ProviderIcons.identifier(id) != nil{return value}
        if let id=value.removingPrefix("ninja:"),!id.isEmpty{return value}
        return "kai"
    }
    var mark:MenuBarMark {
        if selection=="current"{return lastAgent}
        if let id=selection.removingPrefix("provider:"),let canonical=ProviderIcons.identifier(id){return .provider(canonical)}
        if let id=selection.removingPrefix("ninja:"){
            if let ninja=ninjas.first(where:{$0.id==id}){return MenuBarMark(ninja:ninja)}
            if let pinnedNinja,pinnedNinja.id==id{return pinnedNinja}
        }
        return .kai
    }
    init(defaults:UserDefaults = .standard){
        self.defaults=defaults
        lastAgent=defaults.data(forKey:Self.lastKey).flatMap{try? JSONDecoder().decode(MenuBarMark.self,from:$0)} ?? .kai
        pinnedNinja=defaults.data(forKey:Self.pinnedKey).flatMap{try? JSONDecoder().decode(MenuBarMark.self,from:$0)}
        super.init();menu.autoenablesItems=false;rebuild()
    }
    func synchronize(_ state:KaiState){update(agent:state.selectedAgent,currentNinja:state.menuBarNinja,ninjas:state.menuBarNinjas)}
    func update(agent:String?,currentNinja:MenuBarNinja?=nil,ninjas:[MenuBarNinja]?=nil){
        var rebuildNeeded=false
        if let ninjas,self.ninjas != ninjas{self.ninjas=ninjas;rebuildNeeded=true}
        let current=currentNinja.map(MenuBarMark.init(ninja:)) ?? ProviderIcons.identifier(agent).map(MenuBarMark.provider)
        if let current,current != lastAgent{
            lastAgent=current;defaults.set(try? JSONEncoder().encode(current),forKey:Self.lastKey)
        }
        if let id=selection.removingPrefix("ninja:"),let ninja=self.ninjas.first(where:{$0.id==id}){
            remember(MenuBarMark(ninja:ninja))
        }
        if rebuildNeeded{rebuild()}else{refresh()}
        publish()
    }
    private func add(_ title:String,value:String,to menu:NSMenu,image:NSImage?=nil){
        let item=menu.addItem(withTitle:title,action:#selector(choose(_:)),keyEquivalent:"")
        item.target=self;item.representedObject=value;item.image=image
    }
    private func rebuild(){
        menu.removeAllItems()
        add("Kai · 海",value:"kai",to:menu)
        add("Current / last-used agent",value:"current",to:menu)
        menu.addItem(.separator())
        let providerRoot=menu.addItem(withTitle:"Agent provider",action:nil,keyEquivalent:"")
        let providerMenu=NSMenu();providerMenu.autoenablesItems=false;providerRoot.submenu=providerMenu
        for (id,title) in Self.providers{add(title,value:"provider:"+id,to:providerMenu,image:Self.image(.provider(id)))}
        let ninjaRoot=menu.addItem(withTitle:"Personal Ninja",action:nil,keyEquivalent:"")
        let ninjaMenu=NSMenu();ninjaMenu.autoenablesItems=false;ninjaRoot.submenu=ninjaMenu
        for ninja in ninjas{add(ninja.name+" · "+ninja.host,value:"ninja:"+ninja.id,to:ninjaMenu,image:Self.image(MenuBarMark(ninja:ninja)))}
        if ninjas.isEmpty{let hint=ninjaMenu.addItem(withTitle:"Create a Ninja in Code",action:nil,keyEquivalent:"");hint.isEnabled=false}
        refresh()
    }
    private func refresh(){
        for item in menu.items{
            item.state=(item.representedObject as? String)==selection ? .on:.off
            if let submenu=item.submenu{
                for child in submenu.items{child.state=(child.representedObject as? String)==selection ? .on:.off}
                item.state=submenu.items.contains{$0.state == .on} ? .on:.off
            }
        }
    }
    private func remember(_ ninja:MenuBarMark){
        guard pinnedNinja != ninja else{return};pinnedNinja=ninja
        defaults.set(try? JSONEncoder().encode(ninja),forKey:Self.pinnedKey)
    }
    @objc private func choose(_ item:NSMenuItem){
        guard let value=item.representedObject as? String else{return}
        if let id=value.removingPrefix("ninja:"){
            guard let ninja=ninjas.first(where:{$0.id==id}) else{return};remember(MenuBarMark(ninja:ninja))
        }
        defaults.set(value,forKey:Self.choiceKey);refresh();publish()
    }
    private func publish(){let next=mark;guard delivered != next else{return};delivered=next;onChange?(next)}
    static func image(_ mark:MenuBarMark)->NSImage?{
        if mark.kind=="provider"{
            guard let original=ProviderIcons.image(mark.id),let image=original.copy() as? NSImage else{return nil}
            image.size=CGSize(width:16,height:16);image.isTemplate=true;return image
        }
        guard mark.kind=="ninja",["fire","lightning","earth","ice","energy","water"].contains(mark.style) else{return nil}
        let resource="ninja-"+mark.style
        if let cached=ninjaImages.object(forKey:resource as NSString){return cached}
        let root=URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let url=Bundle.main.resourceURL?.appendingPathComponent("providers/"+resource+".svg")
        guard let image=url.flatMap(NSImage.init(contentsOf:)) ?? NSImage(contentsOf:root.appendingPathComponent("integrations/Code/assets/"+resource+".svg")) else{return nil}
        image.size=CGSize(width:18,height:18);image.isTemplate=false;ninjaImages.countLimit=6;ninjaImages.setObject(image,forKey:resource as NSString);return image
    }
}
private extension String {
    func removingPrefix(_ prefix:String)->String?{hasPrefix(prefix) ? String(dropFirst(prefix.count)):nil}
}
