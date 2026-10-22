import Foundation

enum ShortcutAction: String, CaseIterable {
    case sendNewChat, sendActiveChat, sendNewChatWithAppshot, realtimeVoice, realtimeVoiceWithAppshot
    case realtimeVoiceNewChat, realtimeVoiceNewChatWithAppshot

    var title: String {
        switch self {
        case .sendNewChat: return "Send in new chat"
        case .sendActiveChat: return "Send in active / last opened chat"
        case .sendNewChatWithAppshot: return "Send in new chat with appshot context"
        case .realtimeVoice: return "Realtime voice in selected chat"
        case .realtimeVoiceWithAppshot: return "Realtime voice in selected chat with appshot context"
        case .realtimeVoiceNewChat: return "Realtime voice in new chat"
        case .realtimeVoiceNewChatWithAppshot: return "Realtime voice in new chat with appshot context"
        }
    }

    var isVoice: Bool {
        switch self {
        case .realtimeVoice, .realtimeVoiceWithAppshot, .realtimeVoiceNewChat, .realtimeVoiceNewChatWithAppshot: return true
        default: return false
        }
    }
    var capturesContext: Bool { self == .sendNewChatWithAppshot || self == .realtimeVoiceWithAppshot || self == .realtimeVoiceNewChatWithAppshot }
    var createsChat: Bool { self == .sendNewChat || self == .sendNewChatWithAppshot || self == .realtimeVoiceNewChat || self == .realtimeVoiceNewChatWithAppshot }

    static func defaultAction(for trigger: DictationTrigger) -> Self {
        switch trigger {
        case .command: return .sendActiveChat
        case .function: return .sendNewChat
        case .option: return .sendNewChatWithAppshot
        }
    }

    func holdDuration(for trigger: DictationTrigger) -> TimeInterval {
        if isVoice{return 0}
        return trigger == .option ? 0.2 : 1.0
    }
}

/// A chord must be fully released before it can become a single-key shortcut.
/// In particular, releasing Control after Control-Option must not take an appshot.
struct ShortcutGesture {
    enum Change: Equatable { case unchanged, cancel, begin(DictationTrigger) }
    private var down=Set<DictationTrigger>()
    private var held:DictationTrigger?

    mutating func update(command:Bool,option:Bool,control:Bool,function:Bool,shift:Bool)->Change {
        let previous=down
        down=Set([(DictationTrigger.command,command),(.option,option),(.function,function)].compactMap{$0.1 ? $0.0:nil})
        let next=ShortcutPreferences.heldTrigger(command:command,option:option,control:control,function:function,shift:shift)
        guard next != held else{return .unchanged}
        held=next
        if let next,!previous.contains(next) {return .begin(next)}
        return .cancel
    }
}

struct ShortcutPreferences {
    let defaults: UserDefaults

    var expandedDictationOnly:Bool {
        get {defaults.bool(forKey:"expandedDictationOnly")}
        nonmutating set {defaults.set(newValue,forKey:"expandedDictationOnly")}
    }
    func action(for trigger:DictationTrigger,expanded:Bool)->ShortcutAction? {
        if expanded && expandedDictationOnly {return trigger == .command ? .sendActiveChat:nil}
        return action(for:trigger)
    }

    func holdDuration(for trigger:DictationTrigger)->TimeInterval {
        let action=action(for:trigger),fallback=action.isVoice ? 0.25:action.holdDuration(for:trigger)
        guard let value=defaults.object(forKey:"shortcutHold.\(trigger.rawValue)") as? Double,value.isFinite else{return fallback}
        return min(5,max(0.1,value))
    }
    func setHoldDuration(_ value:TimeInterval,for trigger:DictationTrigger){
        guard value.isFinite else{return}
        defaults.set(min(5,max(0.1,value)),forKey:"shortcutHold.\(trigger.rawValue)")
    }
    func resetHoldDurations(){for trigger in DictationTrigger.allCases{defaults.removeObject(forKey:"shortcutHold.\(trigger.rawValue)")}}

    func action(for trigger: DictationTrigger) -> ShortcutAction {
        // The first version stored "dictation" without its destination. Restore
        // that key's original action when migrating or reading an unknown value.
        ShortcutAction(rawValue: defaults.string(forKey: "shortcutAction.\(trigger.rawValue)") ?? "") ?? .defaultAction(for: trigger)
    }

    func set(_ action: ShortcutAction, for trigger: DictationTrigger) {
        defaults.set(action.rawValue, forKey: "shortcutAction.\(trigger.rawValue)")
    }

    // Input shortcuts are deliberate single-modifier holds. Chords retain their
    // behavior, including Control-Option's running-chat shortcut.
    static func heldTrigger(command: Bool, option: Bool, control: Bool, function: Bool, shift: Bool) -> DictationTrigger? {
        guard !control, !shift else { return nil }
        let held: [DictationTrigger] = [(DictationTrigger.command, command), (.option, option), (.function, function)]
            .compactMap { $0.1 ? $0.0 : nil }
        return held.count == 1 ? held.first : nil
    }
}
