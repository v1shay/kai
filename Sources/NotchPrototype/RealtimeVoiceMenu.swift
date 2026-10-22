import AppKit

/// Native voices accepted by Codex's V3 realtime connection (the V1 voice family).
/// This preference is independent of local ONNX speech playback.
enum RealtimeVoiceChoice:String,CaseIterable {
    case codexDefault="default"
    case arbor,breeze,cove,ember,juniper,maple,sol,spruce,vale
    var title:String {self == .codexDefault ? "Codex default":rawValue.capitalized}
    var apiValue:String? {self == .codexDefault ? nil:rawValue}
}

@MainActor final class RealtimeVoiceMenu:NSObject {
    let menu=NSMenu()
    private let defaults:UserDefaults
    private static let key="realtimeVoice"
    var onSelection:((RealtimeVoiceChoice)->Void)?
    var selection:RealtimeVoiceChoice {
        RealtimeVoiceChoice(rawValue:defaults.string(forKey:Self.key) ?? "") ?? .codexDefault
    }

    init(defaults:UserDefaults = .standard){
        self.defaults=defaults
        super.init()
        menu.autoenablesItems=false
        for choice in RealtimeVoiceChoice.allCases {
            let item=menu.addItem(withTitle:choice.title,action:#selector(selectVoice(_:)),keyEquivalent:"")
            item.target=self;item.representedObject=choice.rawValue
        }
        menu.addItem(.separator())
        let hint=menu.addItem(withTitle:"Applies to the next voice conversation",action:nil,keyEquivalent:"")
        hint.isEnabled=false
        refresh()
    }
    private func refresh(){
        for item in menu.items {
            item.state=(item.representedObject as? String) == selection.rawValue ? .on:.off
        }
    }
    @objc private func selectVoice(_ item:NSMenuItem){
        guard let raw=item.representedObject as? String,let choice=RealtimeVoiceChoice(rawValue:raw) else{return}
        defaults.set(choice.rawValue,forKey:Self.key);refresh();onSelection?(choice)
    }
}
