import Foundation

/// Isolated probes may read the real bundle's appearance preferences while
/// keeping their synthetic workspace and menu updates inside the process.
enum NativePreferencePersistence {
    static let enabled = permitsWrites(arguments: CommandLine.arguments)
    static func permitsWrites(arguments: [String]) -> Bool {
        !arguments.contains("--unified-smoke-test")
    }
}
