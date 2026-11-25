import AppKit
import XCTest
@testable import NotchPrototype

final class PetIconOrchestrationTests:XCTestCase {
    @MainActor final class HeldExecutor {
        var jobs:[(PetIconFrameRequest,(PetIconFrameResult)->Void)]=[]
        func execute(_ request:PetIconFrameRequest,_ done:@escaping(PetIconFrameResult)->Void){jobs.append((request,done))}
        func finish(_ index:Int){let job=jobs[index];job.1(PetIconFramePipeline.render(job.0))}
    }
    func sheet(_ red:CGFloat)->CGImage {
        let c=CGContext(data:nil,width:88,height:39,bitsPerComponent:8,bytesPerRow:352,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue)!
        for row in 0..<3 {for col in 0..<8 {
            c.setFillColor(CGColor(red:red,green:CGFloat(row)/3,blue:CGFloat(col)/8,alpha:1))
            c.fill(CGRect(x:col*11,y:row*13,width:11,height:13))
        }}
        return c.makeImage()!
    }
    func bytes(_ image:NSImage?)->Data? {
        guard let image,let cg=image.cgImage(forProposedRect:nil,context:nil,hints:nil),let c=CGContext(data:nil,width:1,height:1,bitsPerComponent:8,bytesPerRow:4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue) else{return nil}
        c.draw(cg,in:CGRect(x:-cg.width/2,y:-cg.height/2,width:cg.width,height:cg.height))
        return Data(bytes:c.data!,count:4)
    }
    @MainActor func play(_ icon:PetAppIcon,_ id:String,_ image:CGImage,row:Int=0,loops:Bool=true,durations:[Int]=[10000,10000]){icon.play(petID:id,sheet:image,rows:3,row:row,columns:[0,1],durationsMs:durations,loops:loops)}
    @MainActor func testActualRepeatedPlayStopAndResubmit()throws {
        let app=NSApplication.shared,oldView=NSApp.dockTile.contentView,oldIcon=NSApp.applicationIconImage
        defer{app.applicationIconImage=oldIcon;app.dockTile.contentView=oldView}
        let held=HeldExecutor(),icon=PetAppIcon(execute:held.execute);defer{icon.stop()}
        let first=sheet(0.2),second=sheet(0.8)
        play(icon,"a",first);play(icon,"a",first)
        XCTAssertEqual(held.jobs.count,1)
        let before=bytes(app.applicationIconImage)
        let ownedView=app.dockTile.contentView
        icon.stop();held.finish(0)
        XCTAssertNil(app.dockTile.contentView);XCTAssertEqual(bytes(app.applicationIconImage),before)
        play(icon,"b",second);XCTAssertEqual(held.jobs.count,2);held.finish(1)
        XCTAssertEqual(bytes(app.applicationIconImage),bytes(PetAppIcon.frame(sheet:second,rows:3,row:0,column:0,size:512)))
        XCTAssertNotNil((app.dockTile.contentView as? NSImageView)?.image)
        XCTAssertTrue(app.dockTile.contentView === ownedView,"Accepted still publication must restore this active instance view")
    }
    @MainActor func testActualPetSwitchRejectsOldAndRowChangePreservesInitialStill()throws {
        let app=NSApplication.shared,oldView=NSApp.dockTile.contentView,oldIcon=NSApp.applicationIconImage
        defer{app.applicationIconImage=oldIcon;app.dockTile.contentView=oldView}
        let held=HeldExecutor(),icon=PetAppIcon(execute:held.execute);defer{icon.stop()}
        let first=sheet(0.2),second=sheet(0.8)
        let before=bytes(app.applicationIconImage)
        play(icon,"a",first);let ownedView=app.dockTile.contentView
        play(icon,"b",second,row:1);play(icon,"b",second,row:2)
        held.finish(0);XCTAssertEqual(bytes(app.applicationIconImage),before);XCTAssertEqual(held.jobs.count,2)
        XCTAssertEqual(held.jobs[1].0.row,2);XCTAssertEqual(held.jobs[1].0.still?.row,1)
        held.finish(1)
        XCTAssertTrue(app.dockTile.contentView === ownedView,"Still setter may replace Dock view; restore the active instance only")
        XCTAssertEqual(bytes(app.applicationIconImage),bytes(PetAppIcon.frame(sheet:second,rows:3,row:1,column:0,size:512)))
        XCTAssertEqual(bytes((app.dockTile.contentView as? NSImageView)?.image),bytes(PetAppIcon.frame(sheet:second,rows:3,row:2,column:0,size:128)))
    }
    @MainActor func testActualPendingStillRebindsOnSamePetLibraryReplacement()throws {
        let app=NSApplication.shared,oldView=NSApp.dockTile.contentView,oldIcon=NSApp.applicationIconImage
        defer{app.applicationIconImage=oldIcon;app.dockTile.contentView=oldView}
        let held=HeldExecutor(),icon=PetAppIcon(execute:held.execute);defer{icon.stop()}
        let first=sheet(0.2),replacement=sheet(0.8)
        play(icon,"a",first,row:0);play(icon,"a",replacement,row:2)
        let before=bytes(app.applicationIconImage);held.finish(0)
        XCTAssertEqual(bytes(app.applicationIconImage),before);XCTAssertEqual(held.jobs.count,2)
        XCTAssertTrue(held.jobs[1].0.still?.sheet === replacement,"Pending512 still must not retain an obsolete library sheet")
        XCTAssertEqual(held.jobs[1].0.still?.row,0,"Row-only change keeps the selected initial still cell")
        held.finish(1)
        XCTAssertEqual(bytes(app.applicationIconImage),bytes(PetAppIcon.frame(sheet:replacement,rows:3,row:0,column:0,size:512)))
    }
    @MainActor func testActualLateCompletionUsesExistingPlaybackClock()async throws {
        try XCTSkipIf(NSWorkspace.shared.accessibilityDisplayShouldReduceMotion,"User reduce-motion setting intentionally selects first frame; no setting change")
        let app=NSApplication.shared,oldView=NSApp.dockTile.contentView,oldIcon=NSApp.applicationIconImage
        defer{app.applicationIconImage=oldIcon;app.dockTile.contentView=oldView}
        let held=HeldExecutor(),icon=PetAppIcon(execute:held.execute);defer{icon.stop()}
        let image=sheet(0.6);var completions=0;icon.onCompletion={completions += 1}
        play(icon,"a",image,row:1,loops:false,durations:[16,16])
        try await Task.sleep(nanoseconds:120_000_000)
        XCTAssertEqual(completions,1)
        held.finish(0)
        XCTAssertEqual(bytes((app.dockTile.contentView as? NSImageView)?.image),bytes(PetAppIcon.frame(sheet:image,rows:3,row:1,column:1,size:128)),"Completion must select elapsed final frame, not restart column0")
        XCTAssertEqual(completions,1)
    }
}
