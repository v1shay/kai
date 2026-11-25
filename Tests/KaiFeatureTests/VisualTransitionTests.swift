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
    func testMediaArtworkCrossfadesToPetWithListeningMeterStillVisible()async throws {
        let (transition,artwork,pet,meter)=await MainActor.run{(NotchVisualTransition(),CALayer(),CALayer(),NotchIndicatorEngine())}
        let context=CGContext(data:nil,width:2,height:2,bitsPerComponent:8,bytesPerRow:8,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        let image=context.makeImage()!
        let frame=CGRect(x:10,y:5,width:32,height:32)
        await MainActor.run {
            artwork.frame=frame;pet.frame=frame;pet.opacity=0
            transition.artwork(image,on:artwork)
            meter.startListening(useMicrophone:false) // media level meter
            transition.hideArtwork(artwork);transition.reveal(pet,visible:true)
            meter.setExternalLevels([0,0,0,0]);meter.startListening(useMicrophone:false)
            XCTAssertNotNil(artwork.animation(forKey:"statusFade"))
            XCTAssertNotNil(pet.animation(forKey:"statusFade"))
            XCTAssertEqual(meter.layer.opacity,1)
        }
        try await Task.sleep(nanoseconds:800_000_000)
        await MainActor.run {
            XCTAssertTrue(artwork.isHidden);XCTAssertEqual(pet.opacity,1)
            XCTAssertEqual(pet.frame,frame);XCTAssertEqual(meter.mode,.listening)
            XCTAssertEqual(meter.layer.opacity,1);meter.hide()
        }
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
