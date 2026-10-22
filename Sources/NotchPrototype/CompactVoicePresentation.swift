import Foundation

/// Voice starts compact; automatic updates preserve the user's expansion choice.
struct CompactVoicePresentation {
    enum Activity:String {case listening,thinking,speaking,attention,failure}
    static func layout(active:Bool,open:Bool,expanded:Bool,manualExpansion:Bool=false)->(open:Bool,expanded:Bool){
        active ? (true,manualExpansion):(open,expanded)
    }
    static func activity(recording:Bool,connecting:Bool,backendScene:String,voice:VoiceActivity)->Activity {
        if recording{return .listening}
        if ["permission","question"].contains(backendScene){return .attention}
        if backendScene == "failure"{return .failure}
        if !connecting,voice == .speaking{return .speaking}
        let working=["thinking","terminal","tool","files","search","image","read","agents","plan","code","working"].contains(backendScene)
        if connecting || working || voice == .thinking{return .thinking}
        return voice == .speaking ? .speaking:.listening
    }
}

enum VoiceActivity:String {case listening,thinking,speaking}
