import Foundation

final class PetSelectionPreferences {
    static let excludedIDs:Set<String>=["codex","savage-codex-hacker"]
    private let defaults:UserDefaults
    private let persistChanges:Bool
    private var transientExcluded:Bool?,transientSelected:String?,transientLast:String?
    private var selectedWasSet=false,lastWasSet=false
    init(defaults:UserDefaults,persistChanges:Bool=NativePreferencePersistence.enabled){self.defaults=defaults;self.persistChanges=persistChanges}
    var excludeCodexPets:Bool {
        get{transientExcluded ?? (defaults.object(forKey:"excludeAutomaticCodexPets") as? Bool ?? true)}
        set{if persistChanges{defaults.set(newValue,forKey:"excludeAutomaticCodexPets")}else{transientExcluded=newValue}}
    }
    var selectedID:String? {
        get{selectedWasSet ? transientSelected:defaults.string(forKey:"explicitlySelectedPet")}
        set{if persistChanges{defaults.set(newValue,forKey:"explicitlySelectedPet")}else{selectedWasSet=true;transientSelected=newValue}}
    }
    var lastUsedID:String? {
        get{lastWasSet ? transientLast:defaults.string(forKey:"lastUsedPet")}
        set{if lastUsedID != newValue{if persistChanges{defaults.set(newValue,forKey:"lastUsedPet")}else{lastWasSet=true;transientLast=newValue}}}
    }
    func automaticIDs(_ ids:[String])->[String]{excludeCodexPets ? ids.filter{!Self.excludedIDs.contains($0)}:ids}
    func initialID(_ ids:[String])->String? {
        if let selectedID,ids.contains(selectedID){return selectedID}
        if let lastUsedID,automaticIDs(ids).contains(lastUsedID){return lastUsedID}
        return automaticIDs(ids).first
    }
}
