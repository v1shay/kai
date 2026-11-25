import AppKit

enum WorkspaceArrowKey: UInt16 {
    case left = 123
    case right = 124
    case down = 125
    case up = 126

    init?(_ event: NSEvent) {
        if let hardware = Self(rawValue: event.keyCode) {
            self = hardware
            return
        }
        guard let text = event.charactersIgnoringModifiers, text.unicodeScalars.count == 1,
              let value = text.unicodeScalars.first?.value else { return nil }
        switch value {
        case 0xf700: self = .up
        case 0xf701: self = .down
        case 0xf702: self = .left
        case 0xf703: self = .right
        default:
            // Some external input sources provide the legacy arrow character
            // only. Real Control and Command shortcuts must stay distinct.
            guard event.modifierFlags.isDisjoint(with: [.control, .command]) else { return nil }
            switch value {
            case 0x1c: self = .left
            case 0x1d: self = .right
            case 0x1e: self = .up
            case 0x1f: self = .down
            default: return nil
            }
        }
    }

    var splitCommand: String {
        switch self {
        case .left: "split_left"
        case .right: "split_right"
        case .down: "split_down"
        case .up: "split_up"
        }
    }
}
