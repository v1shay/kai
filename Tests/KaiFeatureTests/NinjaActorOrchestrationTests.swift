import AppKit
import XCTest
@testable import NotchPrototype

final class NinjaActorOrchestrationTests: XCTestCase {
    @MainActor final class Fixture {
        let jobs = NinjaActorPipelineTests.Jobs()
        let window: NSPanel
        let view: NinjaAtmosphereView
        init() {
            _ = NSApplication.shared
            window = NSPanel(contentRect: CGRect(x: 120, y: 120, width: 420, height: 300),
                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            window.isReleasedWhenClosed = false
            window.ignoresMouseEvents = true
            view = NinjaAtmosphereView(frame: CGRect(x: 0, y: 0, width: 420, height: 300),
                                       actorExecutor: jobs.execute)
            view.isHidden = true
            window.contentView!.addSubview(view)
            window.orderFrontRegardless()
            let end = Date(timeIntervalSinceNow: 0.4)
            while !window.occlusionState.contains(.visible) && Date() < end {
                RunLoop.current.run(mode: .default, before: Date(timeIntervalSinceNow: 0.005))
            }
        }
        func configure(style: String = "ice", motion: Bool = true, identity: String = "fixture") {
            view.configure(identity: identity, style: style, working: false,
                           motion: motion, visible: true)
        }
        func close() {
            view.suspend()
            view.removeFromSuperview()
            window.orderOut(nil)
            window.close()
            jobs.drain()
        }
        func wave() { view.beginActorWave() }
    }

    @MainActor func requireVisible(_ fixture: Fixture) {
        XCTAssertTrue(fixture.window.isVisible)
        XCTAssertTrue(fixture.window.occlusionState.contains(.visible))
    }

    @MainActor func testActualCallerCoalescesAndInstallsOriginalPlanOrderAndEpoch() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        fixture.configure()
        XCTAssertEqual(fixture.jobs.started.count, 1)
        let epoch = fixture.view.diagnosticEpoch
        XCTAssertNotNil(epoch)
        let result = fixture.jobs.finish()!
        let gait = fixture.view.diagnosticActorGait!
        let plan = NinjaMotionPlan(size: CGSize(width: 420, height: 102), working: false, finished: false)
        XCTAssertEqual(gait.duration, plan.duration)
        XCTAssertEqual(gait.beginTime, epoch)
        let values = gait.values as! [CGImage]
        XCTAssertEqual(values.count, plan.samples.count)
        for (index, sample) in plan.samples.enumerated() {
            let key = NinjaPixelActor.Key(style: "ice", action: sample.action, step: sample.step, left: sample.left)
            XCTAssertTrue(values[index] === result.frames[key])
        }
        XCTAssertEqual(fixture.view.diagnosticEpoch, epoch)
    }

    @MainActor func testStyleReplacementRetainsValidPortraitAndRejectsOldBatch() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure(style: "ice")
        fixture.jobs.finish()
        let initial = fixture.view.diagnosticActorFrame!
        fixture.configure(style: "fire")
        XCTAssertTrue(fixture.view.diagnosticActorFrame === initial)
        fixture.configure(style: "earth", identity: "new-owner")
        fixture.jobs.finish()
        XCTAssertTrue(fixture.view.diagnosticActorFrame === initial)
        fixture.jobs.finish()
        XCTAssertFalse(fixture.view.diagnosticActorFrame === initial)
    }

    @MainActor func testHidePreventsPendingPublicationAndRevealRequestsCurrentFrames() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        fixture.view.suspend()
        fixture.jobs.finish()
        XCTAssertNil(fixture.view.diagnosticActorFrame)
        XCTAssertEqual(fixture.jobs.started.count, 1)
        fixture.configure()
        XCTAssertEqual(fixture.jobs.started.count, 2)
        fixture.jobs.finish()
        XCTAssertNotNil(fixture.view.diagnosticActorFrame)
    }

    @MainActor func testDetachAndResizeRejectPriorViewportCompletion() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        fixture.view.removeFromSuperview()
        fixture.jobs.finish()
        XCTAssertNil(fixture.view.diagnosticActorFrame)
        fixture.window.contentView!.addSubview(fixture.view)
        fixture.view.needsLayout = true
        fixture.view.layoutSubtreeIfNeeded()
        XCTAssertEqual(fixture.jobs.started.count, 2)
        fixture.view.frame.size = CGSize(width: 300, height: 250)
        fixture.jobs.finish()
        XCTAssertNil(fixture.view.diagnosticActorFrame)
        fixture.view.needsLayout = true
        fixture.view.layoutSubtreeIfNeeded()
        // Pure actor pixels are independent of viewport geometry and can be reused.
        if !fixture.jobs.pending.isEmpty { fixture.jobs.finish() }
        XCTAssertNotNil(fixture.view.diagnosticActorFrame)
    }

    @MainActor func testReducedMotionRequestsOnlyRestAndAddsNoGaitOrWave() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure(motion: false)
        XCTAssertEqual(fixture.jobs.started.count, 1)
        XCTAssertEqual(fixture.jobs.started[0].keys.count, 1)
        fixture.jobs.finish()
        XCTAssertNotNil(fixture.view.diagnosticActorFrame)
        XCTAssertNil(fixture.view.diagnosticActorGait)
        fixture.wave()
        XCTAssertNil(fixture.view.diagnosticActorWave)
        XCTAssertEqual(fixture.jobs.started.count, 1)
    }

    @MainActor func testLateWaveCompletionKeepsEpochAndCannotResurrectExpiredWave() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        let epoch = fixture.view.diagnosticEpoch
        fixture.wave()
        XCTAssertNotNil(fixture.view.diagnosticActorWaveStarted)
        let advanced = fixture.view.diagnosticActorLocalTime + 3
        fixture.view.layer!.speed = 0
        fixture.view.layer!.timeOffset = advanced
        fixture.jobs.finish()
        XCTAssertNil(fixture.view.diagnosticActorWave)
        XCTAssertEqual(fixture.view.diagnosticActorGait?.beginTime, epoch)
        XCTAssertEqual(fixture.view.diagnosticEpoch, epoch)
    }
    @MainActor func testActualMouseHandlerStartsFiniteWaveAtPresentedActor() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        fixture.jobs.finish()
        CATransaction.flush()
        RunLoop.current.run(until: Date(timeIntervalSinceNow: 0.15))
        XCTAssertNil(fixture.view.diagnosticActorWaveStarted)
        // Match the handler's presentation/model fallback using this owned layer.
        let point = fixture.view.diagnosticPosition ?? fixture.view.diagnosticModelPosition
        let event = NSEvent.mouseEvent(with: .mouseMoved,
            location: fixture.view.convert(point, to: nil), modifierFlags: [],
            timestamp: ProcessInfo.processInfo.systemUptime,
            windowNumber: fixture.window.windowNumber, context: nil,
            eventNumber: 0, clickCount: 0, pressure: 0)!
        fixture.view.mouseMoved(with: event)
        let start = fixture.view.diagnosticActorWaveStarted
        XCTAssertNotNil(start)
        let wave = fixture.view.diagnosticActorWave
        XCTAssertNotNil(wave)
        XCTAssertEqual(wave?.beginTime, start)
        XCTAssertEqual(wave?.duration, 1.3)
        XCTAssertEqual(wave?.values?.count, 16)
    }

    @MainActor func testWorkingToFinishedRejectsPriorGaitAndPreservesEpoch() {
        let fixture = Fixture(); defer { fixture.close() }
        requireVisible(fixture)
        fixture.configure()
        fixture.jobs.finish()
        let initial = fixture.view.diagnosticActorFrame
        let epoch = fixture.view.diagnosticEpoch
        fixture.view.configure(identity: "fixture", style: "ice", working: true,
                               motion: true, visible: true)
        fixture.view.configure(identity: "fixture", style: "ice", working: false,
                               motion: true, visible: true)
        fixture.jobs.finish()
        XCTAssertTrue(fixture.view.diagnosticActorFrame === initial)
        let result = fixture.jobs.finish()!
        let plan = NinjaMotionPlan(size: CGSize(width: 420, height: 102), working: false, finished: true)
        let values = fixture.view.diagnosticActorGait!.values as! [CGImage]
        XCTAssertEqual(values.count, plan.samples.count)
        for (index, sample) in plan.samples.enumerated() {
            let key = NinjaPixelActor.Key(style: "ice", action: sample.action, step: sample.step, left: sample.left)
            XCTAssertTrue(values[index] === result.frames[key])
        }
        XCTAssertEqual(fixture.view.diagnosticActorGait?.beginTime, epoch)
        XCTAssertEqual(fixture.view.diagnosticEpoch, epoch)
    }

}
