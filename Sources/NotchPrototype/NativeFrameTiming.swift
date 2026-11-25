import Foundation
import QuartzCore

/// Scalar, main-thread-only diagnostics for the explicitly opted-in Ninja probe.
@MainActor enum NativeFrameTiming {
    enum Stage: UInt16 {
        case runtimeReceive = 1, appReceive, syncTaskPet, play, ninjaSprite
        case ninjaImageLookup, ninjaCGImage, inlinePets, inlineRows
        case inlineConversation, inlineEnvelope, bridgeSend, runtimeSend
        case conversationTheme, compactCycle, inlineCounts, compactEligibility
        case glassBackground, ninjaConfigure, ninjaAnimate, actorBatchPublish
        case actorRasterMicroseconds, actorRasterMisses
        case menuIdentity, typography, nativeReview, miniState, speechReceive, actorRasterRequest
    }

    struct Scope {
        let sequence: UInt64
        let parent: UInt64
        let stage: Stage
    }

    struct Record {
        let time: Double
        let sequence: UInt64
        let parent: UInt64
        let value: Int64
        let stage: Stage
        let edge: UInt8
    }

    /// Pure storage logic accepts timestamps so tests require no native windows.
    struct Buffer {
        let capacity: Int
        private(set) var records: [Record] = []
        private(set) var dropped: UInt64 = 0
        private(set) var depth = 0
        private(set) var closed = false
        private var sequence: UInt64 = 0
        private var parent: UInt64 = 0

        init(capacity: Int) {
            self.capacity = capacity
            records.reserveCapacity(capacity)
        }

        mutating func begin(_ stage: Stage, at time: Double) -> Scope? {
            guard !closed else { return nil }
            sequence += 1
            let scope = Scope(sequence: sequence, parent: parent, stage: stage)
            append(Record(time: time, sequence: sequence, parent: parent,
                          value: 0, stage: stage, edge: 0))
            parent = scope.sequence
            depth += 1
            return scope
        }

        mutating func end(_ scope: Scope, at time: Double) {
            guard !closed else { return }
            append(Record(time: time, sequence: scope.sequence, parent: scope.parent,
                          value: 0, stage: scope.stage, edge: 1))
            parent = scope.parent
            depth -= 1
        }

        mutating func instant(_ stage: Stage, value: Int64, at time: Double) {
            guard !closed else { return }
            sequence += 1
            append(Record(time: time, sequence: sequence, parent: parent,
                          value: value, stage: stage, edge: 2))
        }

        private mutating func append(_ record: Record) {
            guard records.count < capacity else { dropped += 1; return }
            records.append(record)
        }

        mutating func take() -> (records: [Record], dropped: UInt64, open: Int)? {
            guard !closed else { return nil }
            closed = true
            let result = (records, dropped, depth)
            records = []
            return result
        }
    }

    static func admitted(flag: String?, arguments: () -> [String]) -> Bool {
        guard flag == "1" else { return false }
        let args = arguments()
        return args.contains("--unified-smoke-test")
            && args.contains("--unified-ninja-render-probe")
    }

    static let enabled = admitted(
        flag: ProcessInfo.processInfo.environment["KAI_DIAGNOSTIC_FRAME_TIMING"],
        arguments: { CommandLine.arguments })
    private static var buffer = Buffer(capacity: 32768)
    static let flushPacket = Data(#"{"type":"diagnosticFrameTimingFlush"}"#.utf8)

    static func begin(_ stage: Stage) -> Scope? {
        guard enabled, !buffer.closed else { return nil }
        let at = CACurrentMediaTime()
        return buffer.begin(stage, at: at)
    }

    static func end(_ scope: Scope?) {
        guard let scope, !buffer.closed else { return }
        let at = CACurrentMediaTime()
        buffer.end(scope, at: at)
    }

    static func instant(_ stage: Stage, value: Int64) {
        guard enabled, !buffer.closed else { return }
        let at = CACurrentMediaTime()
        buffer.instant(stage, value: value, at: at)
    }

    /// The fixed diagnostic packet is consumed before normal owner-state decode.
    static func consumeFlushPacket(_ data: Data) -> Bool {
        guard enabled else { return false }
        guard data.count == flushPacket.count, data == flushPacket else { return false }
        flush()
        return true
    }

    private static func flush() {
        guard enabled, let snapshot = buffer.take() else { return }
        // Buffer mutation is finished before formatting or writing to stderr.
        fputs("KAI_NATIVE_FRAME_TIMING_BEGIN pid=\(ProcessInfo.processInfo.processIdentifier) clock=CACurrentMediaTime count=\(snapshot.records.count) capacity=32768 dropped=\(snapshot.dropped) open_scopes=\(snapshot.open)\n", stderr)
        for record in snapshot.records {
            let line = String(format: "KAI_NATIVE_FRAME_TIMING time=%.9f sequence=%llu parent=%llu stage=%u edge=%u value=%lld\n",
                              record.time, record.sequence, record.parent,
                              record.stage.rawValue, record.edge, record.value)
            fputs(line, stderr)
        }
        fputs("KAI_NATIVE_FRAME_TIMING_END\n", stderr)
    }
}
