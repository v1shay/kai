import XCTest
import AppKit
@testable import NotchPrototype

final class NotchGlassTests:XCTestCase {
    @MainActor func testGlassRemainsBoundedAndNeverInterceptsInput(){
        let view=NotchGlassBackground(frame:.zero)
        for height:CGFloat in [320,600,1400]{
            view.update(canvas:CGSize(width:800,height:height),width:700,enabled:true,color:.systemPurple)
            XCTAssertFalse(view.isHidden)
            XCTAssertLessThanOrEqual(NotchGlassBackground.depth(for:height),height*0.85)
            XCTAssertLessThanOrEqual(NotchGlassBackground.depth(for:height),height*0.85)
            XCTAssertNil(view.hitTest(CGPoint(x:400,y:10)))
            let material=view.subviews.first!
            XCTAssertEqual(material.frame.maxX,750)
            XCTAssertEqual(material.frame.minX,50)
            XCTAssertEqual(material.frame.height,NotchGlassBackground.depth(for:height))
        }
        view.update(canvas:CGSize(width:800,height:600),width:700,enabled:false,color:.systemBlue)
        XCTAssertTrue(view.isHidden)
    }
}
