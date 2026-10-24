import Foundation

final class PetSelectionPreferences {
    static let excludedIDs:Set<String>=["codex","savage-codex-hacker"]
    private let defaults:UserDefaults
    init(defaults:UserDefaults){self.defaults=defaults}
    var excludeCodexPets:Bool {
        get{defaults.object(forKey:"excludeAutomaticCodexPets") as? Bool ?? true}
        set{defaults.set(newValue,forKey:"excludeAutomaticCodexPets")}
    }
    var selectedID:String? {
        get{defaults.string(forKey:"explicitlySelectedPet")}
        set{defaults.set(newValue,forKey:"explicitlySelectedPet")}
    }
    var lastUsedID:String? {
        get{defaults.string(forKey:"lastUsedPet")}
        set{if lastUsedID != newValue{defaults.set(newValue,forKey:"lastUsedPet")}}
    }
    func automaticIDs(_ ids:[String])->[String]{excludeCodexPets ? ids.filter{!Self.excludedIDs.contains($0)}:ids}
    func initialID(_ ids:[String])->String? {
        if let lastUsedID,ids.contains(lastUsedID),lastUsedID == selectedID || automaticIDs(ids).contains(lastUsedID){return lastUsedID}
        if let selectedID,ids.contains(selectedID){return selectedID}
        return automaticIDs(ids).first
    }
}
