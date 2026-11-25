import AppKit
import XCTest
@testable import NotchPrototype

final class IndicatorResourceTests: XCTestCase {
    @MainActor
    func testConstructionAndRepeatedHideDoNotAllocateAnAudioEngine() {
        let indicator = NotchIndicatorEngine()
        XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
        for _ in 0..<10 {
            indicator.hide()
            XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
        }
    }

    @MainActor
    func testAllStaticScenesKeepTheMicrophoneEngineUnallocated() {
        let indicator = NotchIndicatorEngine()
        for scene in IndicatorScenes.all.keys.sorted() {
            indicator.showScene(scene)
            indicator.setFileLoading(true)
            indicator.setFileLoading(false)
            XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated, scene)
        }
        indicator.showThinking()
        XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
        indicator.hide()
    }

    @MainActor
    func testListeningWithoutCaptureAndDemoRemainAudioEngineFree() async throws {
        let indicator = NotchIndicatorEngine()
        indicator.startListening(useMicrophone: false)
        try await Task.sleep(nanoseconds: 350_000_000)
        XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
        indicator.setExternalLevels([0.1, 0.2, 0.3, 0.4])
        indicator.hide()
        indicator.startDemo()
        try await Task.sleep(nanoseconds: 350_000_000)
        XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
        indicator.hide()
        indicator.hide()
        XCTAssertFalse(indicator.diagnosticVoiceMeterEngineAllocated)
    }

    @MainActor
    func testTwoGlowMasksAreSharedAcrossEveryIndicatorAndLayoutMode() throws {
        let first = NotchIndicatorEngine()
        let originalLeft = try mask(first, mirrored: true)
        let originalRight = try mask(first, mirrored: false)
        XCTAssertFalse(originalLeft === originalRight)
        for height: CGFloat in [39, 48, 65] {
            for expanded in [false, true] {
                let indicator = NotchIndicatorEngine()
                indicator.compactBandHeight = height
                indicator.leftAccentLayer.bounds = CGRect(x: 0, y: 0, width: 115, height: 82)
                indicator.accentLayer.bounds = indicator.leftAccentLayer.bounds
                indicator.layout()
                indicator.setAccentExpanded(expanded, duration: 0)
                XCTAssertTrue(try mask(indicator, mirrored: true) === originalLeft)
                XCTAssertTrue(try mask(indicator, mirrored: false) === originalRight)
                indicator.hide()
            }
        }
        first.hide()
    }

    @MainActor
    func testSharedGlowMasksPreserveEveryLegacyBitmapByte() throws {
        let indicator = NotchIndicatorEngine()
        for mirrored in [false, true] {
            let actual = try mask(indicator, mirrored: mirrored)
            let expected = try legacyMask(mirrored: mirrored)
            XCTAssertEqual(actual.width, expected.width)
            XCTAssertEqual(actual.height, expected.height)
            XCTAssertEqual(actual.bitsPerPixel, expected.bitsPerPixel)
            XCTAssertEqual(actual.bytesPerRow, expected.bytesPerRow)
            let actualBytes = try XCTUnwrap(actual.dataProvider?.data) as Data
            let expectedBytes = try XCTUnwrap(expected.dataProvider?.data) as Data
            XCTAssertEqual(actualBytes, expectedBytes)
        }
        indicator.hide()
    }

    @MainActor
    private func mask(_ indicator: NotchIndicatorEngine, mirrored: Bool) throws -> CGImage {
        let root = mirrored ? indicator.leftAccentLayer : indicator.accentLayer
        let compact = try XCTUnwrap(root.sublayers?.last)
        return try XCTUnwrap(AdaptiveColorMath.cgImage(compact.mask?.contents))
    }

    // Independent copy of the original fixed generator. The production cache
    // is never used to produce expected bytes.
    @MainActor
    private func legacyMask(mirrored: Bool) throws -> CGImage {
        let scale = 2, width = 136, height = 78
        let space = CGColorSpaceCreateDeviceRGB()
        let context = try XCTUnwrap(CGContext(
            data: nil, width: width, height: height,
            bitsPerComponent: 8, bytesPerRow: width * 4, space: space,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
        ))
        let gradient = try XCTUnwrap(CGGradient(
            colorsSpace: space,
            colors: [NSColor.white.cgColor, NSColor.white.withAlphaComponent(0.32).cgColor, NSColor.clear.cgColor] as CFArray,
            locations: [0, 0.42, 1]
        ))
        context.scaleBy(x: CGFloat(scale), y: CGFloat(scale) * 0.62)
        let center = CGPoint(x: mirrored ? 5 : 63, y: (39 * 0.22) / 0.62)
        context.drawRadialGradient(
            gradient, startCenter: center, startRadius: 0,
            endCenter: center, endRadius: 66, options: [.drawsAfterEndLocation]
        )
        return try XCTUnwrap(context.makeImage())
    }
}
