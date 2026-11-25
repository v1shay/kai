import AppKit
import Foundation

@main
struct RelaunchFixture {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count >= 2 else {
            throw Failure("Usage: test-kai-ui-reopen SIGNED_APP [COUNT]")
        }
        let url = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath()
        let appPath = url.path
        let server = appPath + "/Contents/MacOS/kai-server"
        let count = CommandLine.arguments.count > 2 ? Int(CommandLine.arguments[2]) : 100
        guard let count, (1...1000).contains(count) else { throw Failure("Invalid launch count") }
        let original = try health(server)
        for app in NSWorkspace.shared.runningApplications where app.bundleURL?.path == appPath {
            try await quit(app)
        }
        for attempt in 1...count {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            configuration.addsToRecentItems = false
            let app = try await open(url, configuration: configuration)
            try await wait("UI launch") { app.isFinishedLaunching && !app.isTerminated }
            try await Task.sleep(for: .milliseconds(220))
            guard try health(server) == original else { throw Failure("Server identity changed on launch \(attempt)") }
            try await quit(app)
            if attempt.isMultiple(of: 10) {
                print("Verified \(attempt) UI launches and quits")
                fflush(stdout)
            }
        }
        print("All \(count) actual UI launches and quits retained the server identity")
    }

    struct Failure: Error { let detail: String; init(_ detail: String) { self.detail = detail } }

    @MainActor static func open(_ url: URL, configuration: NSWorkspace.OpenConfiguration) async throws -> NSRunningApplication {
        try await withCheckedThrowingContinuation { continuation in
            NSWorkspace.shared.openApplication(at: url, configuration: configuration) { application, error in
                if let application {
                    continuation.resume(returning: application)
                } else {
                    continuation.resume(throwing: error ?? Failure("macOS could not launch Kai"))
                }
            }
        }
    }

    @MainActor static func quit(_ app: NSRunningApplication) async throws {
        if app.isTerminated { return }
        guard app.terminate() else { throw Failure("macOS could not request Kai quit") }
        try await wait("UI termination") { app.isTerminated }
    }

    @MainActor static func wait(_ stage: String, predicate: () -> Bool) async throws {
        let end = Date().addingTimeInterval(15)
        while !predicate() {
            if Date() >= end { throw Failure("Timed out waiting for " + stage) }
            try await Task.sleep(for: .milliseconds(66))
        }
    }

    static func health(_ server: String) throws -> String {
        let process=Process()
        let input=Pipe(), output=Pipe()
        process.executableURL=URL(fileURLWithPath: server)
        process.arguments=["call"]
        process.standardInput=input
        process.standardOutput=output
        try process.run()
        try input.fileHandleForWriting.write(contentsOf: Data(#"{"action":"health"}"#.utf8))
        try input.fileHandleForWriting.close()
        let data=output.fileHandleForReading.readDataToEndOfFile()
        process.waitUntilExit()
        guard process.terminationStatus==0,
              let value=try JSONSerialization.jsonObject(with: data) as? [String:Any],
              let id = value["device"] as? String else { throw Failure("Missing server health") }
        return id
    }
}
