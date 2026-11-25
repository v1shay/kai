import Foundation

enum PetCategory:String,CaseIterable {
    case characters="Characters",sports="Sports",creatures="Creatures",desktop="Desktop",originals="Originals"
    static func category(id:String,name:String,kind:String?,explicit:String?=nil)->PetCategory {
        if let explicit,let value=Self(rawValue:explicit){return value}
        let value=(id+" "+name).lowercased()
        if ["chef","curry","king","lebron","messi","cr7","neymar","haaland","hooper","mamba","kobe"].contains(where:value.contains){return .sports}
        if ["clippy","finder","macintosh","crt","bsod","codex","stacky"].contains(where:value.contains){return .desktop}
        if ["bubbles","bean","homelander","omni-man","totoro","teletubby","nezuko","agumon","killua","lawliet","patrick","peter","invincible","kabi"].contains(where:value.contains){return .characters}
        if ["animal","creature"].contains(kind ?? "") || ["hoots","rocky","seedy","dog","webling"].contains(where:value.contains){return .creatures}
        if kind=="person"{return .characters}
        return .originals
    }
    static func of(_ pet:Pet)->PetCategory{category(id:pet.id,name:pet.displayName,kind:pet.kind,explicit:pet.category)}
}
