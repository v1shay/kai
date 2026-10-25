import XCTest
@testable import NotchPrototype

final class WorkspaceHoldGestureTests:XCTestCase {
    func testOneContinuousHoldOpensMiniThenWorkspaceOnlyOnce(){
        var hold=WorkspaceHoldGesture();hold.begin(at:10,workspace:false)
        XCTAssertNil(hold.advance(at:10.99));XCTAssertEqual(hold.advance(at:11),.mini)
        XCTAssertNil(hold.advance(at:11.5));XCTAssertEqual(hold.advance(at:11.8),.workspace)
        XCTAssertNil(hold.advance(at:12));hold.cancel();XCTAssertNil(hold.advance(at:20))
    }
    func testReleaseCancelsAndWorkspaceHoldOnlyFolds(){
        var hold=WorkspaceHoldGesture();hold.begin(at:0,workspace:false);hold.cancel()
        XCTAssertNil(hold.advance(at:3));hold.begin(at:4,workspace:true)
        XCTAssertNil(hold.advance(at:5));XCTAssertEqual(hold.advance(at:5.81),.fold)
        XCTAssertNil(hold.advance(at:7))
    }
    func testDelayedMainThreadCannotOpenMiniAfterWorkspace(){
        var hold=WorkspaceHoldGesture();hold.begin(at:0,workspace:false)
        XCTAssertEqual(hold.advance(at:3),.workspace);XCTAssertNil(hold.advance(at:3.1))
    }
    @MainActor func testProviderMarksAreAvailableAndUnknownProviderIsRejected(){
        for id in ProviderIcons.names{XCTAssertNotNil(ProviderIcons.image(id),id)}
        XCTAssertNil(ProviderIcons.image("unknown"))
    }
    @MainActor func testMorphReversalFinishesWithOnlyTheRequestedSurface()async throws{
        _=NSApplication.shared
        let window=NSWindow(contentRect:CGRect(x:100,y:100,width:1000,height:700),styleMask:[.titled],backing:.buffered,defer:false)
        window.isReleasedWhenClosed=false
        defer{window.orderOut(nil)}
        let morph=WorkspaceMorph(),notch=CGRect(x:500,y:800,width:400,height:300)
        morph.expand(window,from:notch);morph.collapse(window,to:notch){}
        morph.expand(window,from:notch)
        try await Task.sleep(nanoseconds:900_000_000)
        XCTAssertTrue(morph.expanded);XCTAssertFalse(morph.transitioning);XCTAssertTrue(window.isVisible);XCTAssertEqual(window.alphaValue,1)
        morph.collapse(window,to:notch){}
        try await Task.sleep(nanoseconds:800_000_000)
        XCTAssertFalse(morph.expanded);XCTAssertFalse(morph.transitioning);XCTAssertFalse(window.isVisible)
    }
}
