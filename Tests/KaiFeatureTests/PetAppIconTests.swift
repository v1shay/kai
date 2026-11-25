import AppKit
import XCTest
@testable import NotchPrototype

final class PetAppIconTests:XCTestCase {
    func testPlaybackPreservesUnevenTimingAndFiniteFinalFrame()throws {
        let loop=try XCTUnwrap(PetIconPlayback(columns:[2,4,7],durationsMs:[100,200,300],loops:true))
        XCTAssertEqual(loop.column(at:0),2)
        XCTAssertEqual(loop.column(at:0.15),4)
        XCTAssertEqual(loop.column(at:0.45),7)
        XCTAssertEqual(loop.column(at:0.65),2)
        let once=try XCTUnwrap(PetIconPlayback(columns:[2,4,7],durationsMs:[100,200,300],loops:false))
        XCTAssertEqual(once.column(at:100),7)
        XCTAssertNil(PetIconPlayback(columns:[8],durationsMs:[100],loops:true))
        XCTAssertNil(PetIconPlayback(columns:[0],durationsMs:[],loops:true))
    }

    @MainActor func testIconCropsCorrectCellAndPreservesTransparentPadding()throws {
        let context=try XCTUnwrap(CGContext(data:nil,width:80,height:20,bitsPerComponent:8,bytesPerRow:320,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue))
        // Top row is red, bottom row is blue; only column 3 contains pixels.
        context.setFillColor(NSColor.red.cgColor);context.fill(CGRect(x:30,y:10,width:10,height:10))
        context.setFillColor(NSColor.blue.cgColor);context.fill(CGRect(x:30,y:0,width:10,height:10))
        let sheet=try XCTUnwrap(context.makeImage())
        let icon=try XCTUnwrap(PetAppIcon.frame(sheet:sheet,rows:2,row:0,column:3,size:128))
        let bitmap=try XCTUnwrap(NSBitmapImageRep(data:try XCTUnwrap(icon.tiffRepresentation)))
        let center=try XCTUnwrap(bitmap.colorAt(x:64,y:64)?.usingColorSpace(.deviceRGB))
        XCTAssertGreaterThan(center.redComponent,0.9);XCTAssertLessThan(center.blueComponent,0.1)
        XCTAssertEqual(bitmap.colorAt(x:0,y:0)?.alphaComponent,0)
        XCTAssertEqual(bitmap.pixelsWide,128);XCTAssertEqual(bitmap.pixelsHigh,128)
        XCTAssertNil(PetAppIcon.frame(sheet:sheet,rows:2,row:2,column:3,size:128))
    }
}
