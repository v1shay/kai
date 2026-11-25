import Foundation
import XCTest
@testable import NotchPrototype

final class NativeFrameTimingTests: XCTestCase {
    @MainActor func testAdmissionRequiresFlagAndBothProbeArguments() {
        let both = ["--unified-smoke-test", "--unified-ninja-render-probe"]
        XCTAssertTrue(NativeFrameTiming.admitted(flag: "1", arguments: { both }))
        for flag in [nil, "", "0", "true", "2"] {
            XCTAssertFalse(NativeFrameTiming.admitted(flag: flag, arguments: { both }))
        }
        for args in [[], [both[0]], [both[1]], ["--unified-smoke-test-extra", both[1]]] {
            XCTAssertFalse(NativeFrameTiming.admitted(flag: "1", arguments: { args }))
        }
    }

    @MainActor func testDisabledAdmissionDoesNotReadArguments() {
        var readArguments = false
        XCTAssertFalse(NativeFrameTiming.admitted(flag: nil, arguments: {
            readArguments = true
            return []
        }))
        XCTAssertFalse(readArguments)
    }

    @MainActor func testNestedScopesKeepParentAndScalarClock() {
        var buffer = NativeFrameTiming.Buffer(capacity: 8)
        let outer = buffer.begin(.runtimeReceive, at: 100)!
        let inner = buffer.begin(.appReceive, at: 100.25)!
        buffer.end(inner, at: 100.5)
        buffer.end(outer, at: 101)
        XCTAssertEqual(buffer.records.map(\.parent), [0, outer.sequence, outer.sequence, 0])
        XCTAssertEqual(buffer.records.map(\.edge), [0, 0, 1, 1])
        XCTAssertEqual(buffer.records.map(\.time), [100, 100.25, 100.5, 101])
        XCTAssertEqual(buffer.depth, 0)
    }

    @MainActor func testReentrantReceiveReturnsToCallingSendScope() {
        var buffer = NativeFrameTiming.Buffer(capacity: 12)
        let send = buffer.begin(.runtimeSend, at: 1)!
        let receive = buffer.begin(.runtimeReceive, at: 2)!
        buffer.end(receive, at: 3)
        buffer.instant(.inlineCounts, value: 7, at: 4)
        buffer.end(send, at: 5)
        XCTAssertEqual(buffer.records[3].parent, send.sequence)
        XCTAssertEqual(buffer.records[3].value, 7)
        XCTAssertEqual(buffer.depth, 0)
    }

    @MainActor func testCapRetainsPrefixAndReportsEveryDroppedRecord() {
        let stride = MemoryLayout<NativeFrameTiming.Record>.stride
        print("KAI_NATIVE_FRAME_TIMING_TEST_LAYOUT record_stride=\(stride) capacity=32768 nominal_record_bytes=\(stride * 32768)")
        XCTAssertLessThanOrEqual(stride * 32768, 2 * 1024 * 1024)
        var buffer = NativeFrameTiming.Buffer(capacity: 3)
        let outer = buffer.begin(.inlinePets, at: 1)!
        for value in 0..<5 { buffer.instant(.inlineCounts, value: Int64(value), at: 2) }
        buffer.end(outer, at: 3)
        XCTAssertEqual(buffer.records.count, 3)
        XCTAssertEqual(buffer.dropped, 4)
        XCTAssertEqual(buffer.records.map(\.value), [0, 0, 1])
        XCTAssertEqual(buffer.depth, 0)
    }

    @MainActor func testFlushClosesOnceAndExposesIncompleteScope() {
        var buffer = NativeFrameTiming.Buffer(capacity: 4)
        let scope = buffer.begin(.inlinePets, at: 1)!
        let snapshot = buffer.take()!
        XCTAssertEqual(snapshot.records.count, 1)
        XCTAssertEqual(snapshot.open, 1)
        XCTAssertEqual(snapshot.dropped, 0)
        XCTAssertTrue(buffer.records.isEmpty)
        buffer.end(scope, at: 2)
        XCTAssertNil(buffer.begin(.play, at: 3))
        buffer.instant(.inlineCounts, value: 1, at: 4)
        XCTAssertNil(buffer.take())
        XCTAssertTrue(buffer.records.isEmpty)
    }

    @MainActor func testNoProbeArgumentsDisableActualRecorder() {
        XCTAssertNil(NativeFrameTiming.begin(.play))
        NativeFrameTiming.end(nil)
        XCTAssertFalse(NativeFrameTiming.consumeFlushPacket(Data(#"{"type":"diagnosticFrameTimingFlush"}"#.utf8)))
        XCTAssertFalse(NativeFrameTiming.consumeFlushPacket(Data(#"{"type":"diagnosticFrameTimingFlush"}"#.utf8)))
        XCTAssertFalse(NativeFrameTiming.consumeFlushPacket(Data(#"{"type":"unrelated"}"#.utf8)))
    }
}
