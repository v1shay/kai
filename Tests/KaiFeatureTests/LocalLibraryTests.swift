import AppKit
import XCTest
@testable import NotchPrototype

final class LocalLibraryTests:XCTestCase {
    func testHangEvidenceIsRateLimitedAndRequiresARealStall(){
        var gate=HangSampleGate();XCTAssertFalse(gate.shouldSample(now:4,heartbeat:0));XCTAssertTrue(gate.shouldSample(now:6,heartbeat:0));XCTAssertFalse(gate.shouldSample(now:20,heartbeat:0));XCTAssertTrue(gate.shouldSample(now:130,heartbeat:0));XCTAssertTrue(gate.shouldSample(now:260,heartbeat:0));XCTAssertFalse(gate.shouldSample(now:400,heartbeat:0));XCTAssertEqual(gate.samples,3)
    }
    @MainActor func testInstalledFamiliesResolveAndDisabledSpeechDoesNotCreateAudioUnits()throws{
        let oldUI=AppTypography.interfaceFamily,oldCode=AppTypography.codeFamily
        defer{_ = AppTypography.apply(interface:oldUI,code:oldCode)}
        let families=AppTypography.families();XCTAssertFalse(families.isEmpty)
        let family=try XCTUnwrap(families.first)
        _=AppTypography.apply(interface:family,code:family)
        XCTAssertEqual(AppTypography.ui(ofSize:12).familyName,family)
        XCTAssertEqual(AppTypography.code(ofSize:12).familyName,family)
        let speech=SpeechPlayback();speech.configure(enabled:false,model:"");speech.cancelAudio();speech.stop()
        XCTAssertFalse(speech.playbackInitialized,"Simply opening Kai must not instantiate the audio player that crashed at startup")
        if ProcessInfo.processInfo.environment["KAI_LOCAL_LIBRARY_VERIFY"] == "1" {
            for name in ["Google Sans","Geist"]{XCTAssertTrue(families.contains(name),"Installed family missing: \(name)");_ = AppTypography.apply(interface:name,code:name);XCTAssertEqual(AppTypography.ui(ofSize:12).familyName,name)}
            let pets=LocalPetLibrary.discover(in:LocalPetLibrary.roots());XCTAssertTrue(pets.contains{$0.id == "ultronite"});XCTAssertTrue(pets.allSatisfy{$0.sourceURL != nil})
            print("Local installed fonts: Google Sans and Geist; discovered \(pets.count) user pet manifests")
        }
    }
    @MainActor func testLocalPetsLoadWithoutCatalogEditsAndRejectBrokenOrEscapingAssets()throws{
        let root=FileManager.default.temporaryDirectory.appendingPathComponent("kai-library-test-"+UUID().uuidString)
        try FileManager.default.createDirectory(at:root,withIntermediateDirectories:true);defer{try? FileManager.default.removeItem(at:root)}
        let context=try XCTUnwrap(CGContext(data:nil,width:192,height:234,bitsPerComponent:8,bytesPerRow:192*4,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGImageAlphaInfo.premultipliedLast.rawValue))
        context.setFillColor(NSColor.blue.cgColor);context.fill(CGRect(x:0,y:0,width:192,height:234))
        let image=NSBitmapImageRep(cgImage:try XCTUnwrap(context.makeImage())),png=try XCTUnwrap(image.representation(using:.png,properties:[:]))
        func manifest(_ folder:String,_ id:String,_ path:String)throws{
            let directory=root.appendingPathComponent(folder);try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
            try png.write(to:directory.appendingPathComponent("sprite.png"))
            try JSONSerialization.data(withJSONObject:["id":id,"displayName":"Local 🦀","spritesheetPath":path]).write(to:directory.appendingPathComponent("pet.json"))
        }
        try manifest("valid","my-new-pet","sprite.png");try manifest("escaping","escape","../valid/sprite.png");try manifest("broken","broken","missing.webp")
        let pets=LocalPetLibrary.discover(in:[root]);XCTAssertEqual(pets.map(\.id),["my-new-pet"]);XCTAssertEqual(pets[0].grid.rows,9)
        XCTAssertEqual(pets[0].sourceURL,root.appendingPathComponent("valid/sprite.png"))
        let catalog=Catalog(pets:[],stateAnimations:[:]);XCTAssertEqual(LocalPetLibrary.merge(pets,into:catalog).pets.count,1)
        XCTAssertNotNil(LocalPetLibrary.profiles(pets)["my-new-pet"])
        let legacy=Pet(id:"old-id",displayName:"Old copy",spritesheet:"installed_custom/valid/sprite.png",grid:Grid(rows:9),animationOverrides:nil,kind:"installed_custom")
        let old=Catalog(pets:[legacy],stateAnimations:[:]);XCTAssertTrue(LocalPetLibrary.merge([],into:old).pets.isEmpty,"Removing a local pet must not resurrect a packaged copy")
        XCTAssertEqual(LocalPetLibrary.migratedID("old-id",in:old,local:pets),"my-new-pet")
    }
}
