import AppKit

struct DefaultModelPreferences {
    let defaults:UserDefaults
    static let key="defaultCodexModel"
    var selectedID:String? {
        guard let value=defaults.string(forKey:Self.key),!value.isEmpty else{return nil}
        return value
    }
    func set(_ model:String?){
        if let model,!model.isEmpty{defaults.set(model,forKey:Self.key)}else{defaults.removeObject(forKey:Self.key)}
    }
    func fields(action:String)->[String:Any] {
        guard ["new","new_project"].contains(action),let selectedID else{return [:]}
        return ["defaultModel":selectedID]
    }
}

@MainActor final class DefaultModelMenu:NSObject {
    let menu=NSMenu()
    private let preferences:DefaultModelPreferences
    private var modelIDs=[String](),titles=[String]()
    init(defaults:UserDefaults = .standard){
        preferences=DefaultModelPreferences(defaults:defaults);super.init();menu.autoenablesItems=false;rebuild([])
    }
    func update(_ models:[KaiModel]){
        let unique=models.reduce(into:[KaiModel]()){result,model in if !result.contains(where:{$0.model == model.model}){result.append(model)}}
        guard modelIDs != unique.map(\.model) || titles != unique.map(\.displayName) else{return}
        rebuild(unique)
    }
    private func rebuild(_ models:[KaiModel]){
        modelIDs=models.map(\.model);titles=models.map(\.displayName);menu.removeAllItems()
        let automatic=menu.addItem(withTitle:"Follow chat model (default)",action:#selector(selectModel(_:)),keyEquivalent:"")
        automatic.target=self;automatic.representedObject=""
        menu.addItem(.separator())
        for model in models {
            let item=menu.addItem(withTitle:model.displayName,action:#selector(selectModel(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=model.model;item.toolTip=model.model
        }
        if let saved=preferences.selectedID,!modelIDs.contains(saved){
            let item=menu.addItem(withTitle:"\(saved) · unavailable",action:nil,keyEquivalent:"")
            item.representedObject=saved;item.isEnabled=false
        }
        menu.addItem(.separator())
        let hint=menu.addItem(withTitle:"Used when creating a new chat",action:nil,keyEquivalent:"")
        hint.isEnabled=false;refresh()
    }
    private func refresh(){for item in menu.items{item.state=(item.representedObject as? String) == (preferences.selectedID ?? "") ? .on:.off}}
    @objc private func selectModel(_ item:NSMenuItem){
        guard let value=item.representedObject as? String,value.isEmpty || modelIDs.contains(value) else{return}
        preferences.set(value.isEmpty ? nil:value)
        for stale in menu.items where stale.action == nil && stale.representedObject is String{menu.removeItem(stale)}
        refresh()
    }
}
