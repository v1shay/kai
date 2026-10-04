import XCTest
import AppKit
@testable import NotchPrototype

final class VisualTransitionTests:XCTestCase {
    func testArtworkSizeStaysWithinNotchAndIndependentOfPet() {
        let center=CGPoint(x:50,y:20)
        let small=NotchVisualTransition.artworkFrame(size:16,notchHeight:39,center:center,scale:2)
        let large=NotchVisualTransition.artworkFrame(size:100,notchHeight:39,center:center,scale:2)
        XCTAssertEqual(small.size,CGSize(width:16,height:16))
        XCTAssertEqual(large.size,CGSize(width:36,height:36))
        XCTAssertEqual(small.midX,large.midX);XCTAssertEqual(small.midY,large.midY)
        XCTAssertEqual(NotchVisualTransition.artworkFrame(size:.nan,notchHeight:39,center:center,scale:2).width,32)
    }
    func testMediaReentryCancelsAnOlderDelayedHide() async throws {
        let context=CGContext(data:nil,width:2,height:2,bitsPerComponent:8,bytesPerRow:8,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        let image=context.makeImage()!
        let (transition,layer)=await MainActor.run{(NotchVisualTransition(),CALayer())}
        await MainActor.run {
            layer.frame=CGRect(x:0,y:0,width:32,height:32)
            transition.artwork(image,on:layer)
            transition.hideArtwork(layer)
            transition.artwork(image,on:layer)
            XCTAssertEqual(layer.opacity,1)
        }
        try await Task.sleep(nanoseconds:600_000_000)
        await MainActor.run {
            XCTAssertFalse(layer.isHidden)
            XCTAssertNotNil(layer.contents)
            XCTAssertEqual(layer.opacity,1)
        }
    }
}
