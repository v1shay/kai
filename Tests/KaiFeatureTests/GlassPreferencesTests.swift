import XCTest
import AppKit
@testable import NotchPrototype
final class GlassPreferencesTests:XCTestCase {
    func testSmoothCurvesAreMonotoneAndFeatherOut(){
        for blend in GlassPreferences.Blend.allCases {
            XCTAssertEqual(blend.strength(0),1,accuracy:0.00001);XCTAssertEqual(blend.strength(1),0,accuracy:0.00001)
            var previous=1.0
            for i in 0...100 {let value=blend.strength(Double(i)/100);XCTAssertGreaterThanOrEqual(value,0);XCTAssertLessThanOrEqual(value,previous+0.00001);previous=value}
            XCTAssertLessThan(abs(blend.strength(0.999)-blend.strength(1)),0.0001)
        }
    }
    @MainActor func testControlsStayInsideNotchAndResetPersistedValues(){
        let name="GlassTests-"+UUID().uuidString,defaults=UserDefaults(suiteName:name)!
        defer{defaults.removePersistentDomain(forName:name)}
        defaults.set(Double.infinity,forKey:"glass.height");defaults.set(-100,forKey:"glass.opacity");defaults.set(1000,forKey:"glass.radius")
        let preferences=GlassPreferences(defaults:defaults)
        XCTAssertEqual(preferences.height,0.38);XCTAssertEqual(preferences.opacity,0);XCTAssertEqual(preferences.radius,48)
        let view=NotchGlassBackground(frame:.zero)
        var custom=GlassPreferences();custom.height=0.85;custom.offset=0.25
        view.update(canvas:CGSize(width:600,height:500),width:500,enabled:true,color:.systemPink,preferences:custom)
        XCTAssertEqual(view.subviews.first?.frame.maxY,500)
        XCTAssertNil(view.hitTest(NSPoint(x:250,y:250)))
        GlassPreferences.reset(defaults);XCTAssertEqual(GlassPreferences(defaults:defaults).opacity,0.65)
    }
}
