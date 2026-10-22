import XCTest
import AppKit
@testable import NotchPrototype
final class AccentCornerTests:XCTestCase {
    @MainActor func testCompactGlowCoversOuterShouldersThroughFullNotchHeight(){
        let indicator=NotchIndicatorEngine()
        for height:CGFloat in [39,48,65]{
            indicator.compactBandHeight=height
            indicator.leftAccentLayer.frame=CGRect(x:0,y:0,width:115,height:82)
            indicator.accentLayer.frame=CGRect(x:0,y:0,width:115,height:82)
            indicator.layout()
            let left=indicator.leftAccentLayer.sublayers!.last!,right=indicator.accentLayer.sublayers!.last!
            XCTAssertEqual(left.frame.minX,0);XCTAssertEqual(right.frame.maxX,115)
            XCTAssertEqual(left.frame.height,height);XCTAssertEqual(right.frame.height,height)
            XCTAssertEqual(left.mask?.contentsGravity,.resize)
        }
    }
}
