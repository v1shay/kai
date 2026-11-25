import AppKit
import XCTest
@testable import NotchPrototype

final class NotchInputTests:XCTestCase {
    func testCompactAndClosedCanvasNeverAcceptInput() {
        XCTAssertFalse(NotchInputRegion.mainAcceptsInput(expanded:false,web:false))
        XCTAssertTrue(NotchInputRegion.mainAcceptsInput(expanded:true,web:false))
        XCTAssertTrue(NotchInputRegion.mainAcceptsInput(expanded:false,web:true))
        XCTAssertTrue(NotchInputRegion.compactAcceptsInput(open:true,expanded:false,web:false,working:true))
        for (open,expanded,web,working) in [(false,false,false,true),(true,true,false,true),(true,false,true,true),(true,false,false,false)] {
            XCTAssertFalse(NotchInputRegion.compactAcceptsInput(open:open,expanded:expanded,web:web,working:working))
        }
    }
    func testCompactInputHasNoExpandedCanvasOrInvisibleStrip()async {
        await MainActor.run {
            let screen=CGRect(x:0,y:0,width:1512,height:982)
            let frame=NotchInputRegion.compactFrame(screen:screen,center:756,width:370,height:39)
            XCTAssertEqual(frame.maxY,screen.maxY);XCTAssertEqual(frame.height,39)
            XCTAssertFalse(frame.contains(CGPoint(x:756,y:screen.maxY-40)))
            XCTAssertFalse(frame.contains(CGPoint(x:756,y:screen.maxY-300)))
            XCTAssertFalse(frame.contains(CGPoint(x:frame.minX-1,y:screen.maxY-10)))
            let panel=NotchPanel(contentRect:CGRect(x:500,y:100,width:600,height:800),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
            panel.ignoresMouseEvents = !NotchInputRegion.mainAcceptsInput(expanded:false,web:false)
            XCTAssertTrue(panel.ignoresMouseEvents)
        }
    }
}
