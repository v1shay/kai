import AppKit
import ImageIO
import XCTest
@testable import NotchPrototype
final class PetIconWorkerTests:XCTestCase {
    var fixtureRoot:URL {
        if let root=ProcessInfo.processInfo.environment["KAI_ICON_FIXTURE_ROOT"] {return URL(fileURLWithPath:root)}
        return URL(fileURLWithPath:#filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
    }
    var fixtureURLs:[URL]{["codex_built_in/seedy/spritesheet.webp","codex_built_in/codex/spritesheet.webp"].map{fixtureRoot.appendingPathComponent("kai_pets").appendingPathComponent($0)}}
    func sheet()->CGImage {
        let c=CGContext(data:nil,width:88,height:39,bitsPerComponent:8,bytesPerRow:352,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        for row in 0..<3 {for col in 0..<8 {
            c.setFillColor(CGColor(red:CGFloat(col)/8,green:CGFloat(row)/3,blue:0.7,alpha:CGFloat(col+1)/8))
            c.fill(CGRect(x:col*11,y:row*13,width:9,height:11))
        }}
        return c.makeImage()!
    }
    func baseline(sheet:CGImage,rows:Int,row:Int,column:Int,size:Int)->CGImage? {
        guard rows>0,(0..<rows).contains(row),(0..<8).contains(column),size>0 else{return nil}
        let width=sheet.width/8,height=sheet.height/rows
        guard width>0,height>0,
              let crop=sheet.cropping(to:CGRect(x:column*width,y:row*height,width:width,height:height)),
              let context=CGContext(data:nil,width:size,height:size,bitsPerComponent:8,bytesPerRow:size*4,
                                    space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
        let scale=CGFloat(size)*0.92/CGFloat(max(width,height)),w=CGFloat(width)*scale,h=CGFloat(height)*scale
        context.interpolationQuality = .high
        context.draw(crop,in:CGRect(x:(CGFloat(size)-w)/2,y:(CGFloat(size)-h)/2,width:w,height:h))
        guard let image=context.makeImage() else{return nil}
        return image
    }
    func bytes(_ image:CGImage)->Data {
        let c=CGContext(data:nil,width:image.width,height:image.height,bitsPerComponent:8,bytesPerRow:image.width*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        c.draw(image,in:CGRect(x:0,y:0,width:image.width,height:image.height))
        return Data(bytes:c.data!,count:image.width*image.height*4)
    }
    func testExactOriginalPixelsAcrossCellsAndSizes()throws {
        var fixtures:[(CGImage,Int)]=[(sheet(),3)]
        for url in fixtureURLs {
            let source=try XCTUnwrap(CGImageSourceCreateWithURL(url as CFURL,nil))
            fixtures.append((try XCTUnwrap(CGImageSourceCreateImageAtIndex(source,0,[kCGImageSourceShouldCacheImmediately:true] as CFDictionary)),9))
        }
        var compared=0
        for (image,rows) in fixtures {for row in [0,rows/2,rows-1] {for col in 0..<8 {for size in [128,512] {
            let old=try XCTUnwrap(baseline(sheet:image,rows:rows,row:row,column:col,size:size))
            let new=try XCTUnwrap(PetIconFramePipeline.frame(sheet:image,rows:rows,row:row,column:col,size:size))
            XCTAssertEqual(new.width,old.width);XCTAssertEqual(new.height,old.height)
            XCTAssertEqual(bytes(new),bytes(old));compared += new.width*new.height*4
        }}}}
        print("KAI_ICON_PIXEL_EQUAL cases=144 bytes=\(compared) fixtures=\(fixtures.count)")
    }
    @MainActor func testMailboxLatestOnlyStaleAndStop() {
        var queued:[(PetIconFrameRequest,(PetIconFrameResult)->Void)]=[]
        let pipeline=PetIconFramePipeline(execute:{queued.append(($0,$1))})
        let image=sheet();func request(_ row:Int)->PetIconFrameRequest{PetIconFrameRequest(sheet:image,rows:3,row:row,columns:[0,1],still:nil)}
        var published:[Int]=[]
        pipeline.submit(request(0)){_ in published.append(0)}
        pipeline.submit(request(1)){_ in published.append(1)}
        pipeline.submit(request(2)){_ in published.append(2)}
        XCTAssertEqual(pipeline.activeCount,1);XCTAssertEqual(pipeline.pendingCount,1);XCTAssertEqual(queued.count,1)
        queued[0].1(PetIconFrameResult());XCTAssertTrue(published.isEmpty);XCTAssertEqual(queued.count,2);XCTAssertEqual(queued[1].0.row,2)
        queued[1].1(PetIconFrameResult());XCTAssertEqual(published,[2]);XCTAssertEqual(pipeline.activeCount,0)
        pipeline.submit(request(0)){_ in published.append(3)}
        pipeline.submit(request(1)){_ in published.append(4)}
        pipeline.cancel();XCTAssertEqual(pipeline.pendingCount,0)
        queued[2].1(PetIconFrameResult());XCTAssertEqual(published,[2]);XCTAssertEqual(pipeline.activeCount,0)
    }
    @MainActor func testSameRequestCoalescesAndNewSheetInvalidates() {
        var queued:[(PetIconFrameRequest,(PetIconFrameResult)->Void)]=[]
        let pipeline=PetIconFramePipeline(execute:{queued.append(($0,$1))})
        let first=sheet(),second=sheet();func request(_ s:CGImage)->PetIconFrameRequest{PetIconFrameRequest(sheet:s,rows:3,row:1,columns:[0,7],still:nil)}
        var published:[Int]=[]
        pipeline.submit(request(first)){_ in published.append(1)}
        pipeline.submit(request(first)){_ in published.append(2)}
        XCTAssertEqual(queued.count,1);XCTAssertEqual(pipeline.pendingCount,0)
        pipeline.submit(request(second)){_ in published.append(3)}
        queued[0].1(PetIconFrameResult());XCTAssertTrue(published.isEmpty)
        queued[1].1(PetIconFrameResult());XCTAssertEqual(published,[3])
        pipeline.submit(request(first)){_ in published.append(4)};pipeline.cancel()
        pipeline.submit(request(second)){_ in published.append(5)}
        queued[2].1(PetIconFrameResult());XCTAssertEqual(published,[3])
        queued[3].1(PetIconFrameResult());XCTAssertEqual(published,[3,5])
    }
    @MainActor func testRealWorkerOffMainAndCompletionMain()async {
        let done=expectation(description:"worker")
        let pipeline=PetIconFramePipeline()
        let request=PetIconFrameRequest(sheet:sheet(),rows:3,row:1,columns:[0,1,7],still:PetIconStillRequest(sheet:sheet(),rows:3,row:0,column:7))
        pipeline.submit(request){result in
            XCTAssertTrue(Thread.isMainThread);XCTAssertEqual(result.frames.count,3);XCTAssertEqual(result.still?.width,512);done.fulfill()
        }
        await fulfillment(of:[done],timeout:10)
        XCTAssertEqual(pipeline.activeCount,0)
    }
    @MainActor func testRealLazyWebPSharedReadWhileWorkerRenders()async throws {
        let sourceURL=fixtureURLs[0]
        let source=try XCTUnwrap(CGImageSourceCreateWithURL(sourceURL as CFURL,nil))
        let image=try XCTUnwrap(CGImageSourceCreateImageAtIndex(source,0,nil))
        let referenceSource=try XCTUnwrap(CGImageSourceCreateWithURL(sourceURL as CFURL,nil))
        let reference=try XCTUnwrap(CGImageSourceCreateImageAtIndex(referenceSource,0,nil))
        let expected=(0..<8).map{bytes(baseline(sheet:reference,rows:9,row:4,column:$0,size:128)!)}
        let done=expectation(description:"parallel immutable image reads")
        let pipeline=PetIconFramePipeline()
        pipeline.submit(PetIconFrameRequest(sheet:image,rows:9,row:4,columns:Array(0..<8),still:nil)){result in
            for col in 0..<8 {XCTAssertEqual(self.bytes(result.frames[col]!),expected[col])}
            done.fulfill()
        }
        // Same immutable ImageIO image, separate private contexts; no shared context mutation.
        for _ in 0..<4 {for col in 0..<8 {XCTAssertEqual(bytes(baseline(sheet:image,rows:9,row:4,column:col,size:128)!),expected[col])}}
        await fulfillment(of:[done],timeout:10)
    }

    @MainActor func testRowsAndStillSnapshotIdentityArePartOfRequest() {
        let image=sheet(),replacement=sheet()
        let still=PetIconStillRequest(sheet:image,rows:3,row:0,column:7)
        let a=PetIconFrameRequest(sheet:image,rows:3,row:1,columns:[0,1],still:still)
        XCTAssertFalse(a.matches(PetIconFrameRequest(sheet:image,rows:2,row:1,columns:[0,1],still:still)))
        XCTAssertFalse(a.matches(PetIconFrameRequest(sheet:replacement,rows:3,row:1,columns:[0,1],still:still)))
        let changed=PetIconFrameRequest(sheet:image,rows:3,row:2,columns:[0,1],still:still)
        XCTAssertEqual(changed.still?.row,0);XCTAssertEqual(changed.still?.column,7)
        XCTAssertTrue(changed.still?.sheet === image)
    }

}
