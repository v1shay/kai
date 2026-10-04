import XCTest
@testable import NotchPrototype

final class MediaRegressionTests:XCTestCase {
    func testMeterRespondsToSpeakerAmplitudeAndSilence() {
        let silent=[Float](repeating:0,count:128)
        XCTAssertEqual(silent.withUnsafeBufferPointer(PlaybackMeter.rmsBands),[0,0,0,0])
        let samples=[Float](repeating:0.01,count:32)+[Float](repeating:0.02,count:32)+[Float](repeating:0.1,count:32)+[Float](repeating:0.2,count:32)
        let bands=samples.withUnsafeBufferPointer(PlaybackMeter.rmsBands)
        for (actual,expected) in zip(bands,[0.05,0.1,0.5,1.0]){XCTAssertEqual(Double(actual),expected,accuracy:0.001)}
        let invalid=[Float](repeating:.nan,count:32)
        XCTAssertEqual(invalid.withUnsafeBufferPointer(PlaybackMeter.rmsBands),[0,0,0,0])
    }
    func testMediaMustYieldBeforeCodexPanelIsPresented() async throws {
        let idle=try state()
        let localTurn=try state(turn:"new-turn")
        let externalTask=try state(tasks:[["id":"external","name":"Codex","scene":"thinking","text":"Working"]])
        let approval=try state(approval:true)
        let thinkingBeforeTurnID=try state(scene:"thinking")
        await MainActor.run {
            XCTAssertFalse(MediaCompanion.codexNeedsNotch(idle))
            for state in [localTurn,externalTask,approval,thinkingBeforeTurnID] {XCTAssertTrue(MediaCompanion.codexNeedsNotch(state))}
        }
    }
    private func state(turn:String?=nil,tasks:[[String:String]]=[],approval:Bool=false,scene:String="off") throws->KaiState {
        var obj:[String:Any]=["project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadName":"","history":[],"readOnly":false,"approval":approval,"activity":["scene":scene,"text":""],"activeTasks":tasks,"notice":"","attachments":[]]
        if let turn{obj["activeTurnId"]=turn}
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:obj))
    }
}
