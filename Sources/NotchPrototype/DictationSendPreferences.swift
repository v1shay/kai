import Foundation

enum DictationTrigger: String, CaseIterable {
    case command, function, option
    var title: String { rawValue.capitalized }
}

struct DictationSendPreferences {
    let defaults: UserDefaults
    func enabled(for trigger: DictationTrigger) -> Bool {
        let key = "dictationAutoSend.\(trigger.rawValue)"
        return defaults.object(forKey: key) == nil ? trigger == .option : defaults.bool(forKey: key)
    }
    func set(_ enabled: Bool, for trigger: DictationTrigger) {
        defaults.set(enabled, forKey: "dictationAutoSend.\(trigger.rawValue)")
    }
}
