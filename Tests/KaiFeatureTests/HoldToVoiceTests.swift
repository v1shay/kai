import XCTest
import AVFoundation
@testable import NotchPrototype

@MainActor private final class PromptRecorderStub:VoicePromptRecording {
    var onLevels:(([CGFloat])->Void)?,onError:((String)->Void)?
    var starts=0,stops=0,cancels=0,result:Data?=Data([1,2,3])
    func start(){starts+=1}
    func stop()->Data?{stops+=1;return result}
    func cancel(){cancels+=1}
}

@MainActor final class HoldToVoiceTests:XCTestCase {
    func testAdjustedMinimumHoldStillRecordsImmediatelyAndSnapshotsEachGesture() {
        var now:TimeInterval=0,starts=0,discarded=0
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,clock:{now})
        hold.onReady={_,_,_ in starts+=1};hold.onDiscard={discarded+=1}
        hold.begin(.realtimeVoice,trigger:.command,lightweight:false,minimumHold:2)
        XCTAssertEqual(recorder.starts,1)
        now=1;hold.release(.command)
        XCTAssertEqual(discarded,1);XCTAssertEqual(starts,0)
        hold.begin(.realtimeVoice,trigger:.command,lightweight:false,minimumHold:0.1)
        now=1.2;hold.release(.command)
        XCTAssertEqual(starts,1)
        hold.begin(.realtimeVoice,trigger:.command,lightweight:false)
        now=1.3;hold.release(.command)
        XCTAssertEqual(discarded,2);XCTAssertEqual(starts,1)
    }
    func testRecordsBeforeReleaseAndNeverConnectsUntilReleased() {
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var ready=[(ShortcutAction,Data)]();hold.onReady={ready.append(($0,$1));_=$2}
        hold.begin(.realtimeVoiceNewChat,trigger:.function,lightweight:false)
        XCTAssertEqual(recorder.starts,1);XCTAssertEqual(hold.phase,.recording);XCTAssertTrue(ready.isEmpty)
        hold.release(.option);XCTAssertEqual(recorder.stops,0)
        hold.release(.function)
        XCTAssertEqual(recorder.stops,1);XCTAssertEqual(ready.first?.0,.realtimeVoiceNewChat)
        XCTAssertEqual(ready.first?.1,Data([1,2,3]));XCTAssertFalse(hold.active)
        hold.release(.function);XCTAssertEqual(ready.count,1)
    }
    func testReleaseWaitsForAppshotWhileKeepingRecordedPrompt() {
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var captured:((ApplicationContext)->Void)?,ready=[ApplicationContext?]()
        hold.capture={_,done in captured=done};hold.onReady={_,_,context in ready.append(context)}
        hold.begin(.realtimeVoiceNewChatWithAppshot,trigger:.option,lightweight:false)
        hold.release(.option);XCTAssertEqual(hold.phase,.finishing);XCTAssertTrue(ready.isEmpty)
        captured?(ApplicationContext(text:"Editor",imagePath:nil))
        XCTAssertEqual(ready.compactMap{$0}.first?.text,"Editor");XCTAssertFalse(hold.active)
    }
    func testShortTapDiscardsAudioAndDoesNotStartACall() {
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0.25,clock:{0})
        var starts=0,discards=0,captures=0
        hold.onReady={_,_,_ in starts+=1};hold.onDiscard={discards+=1};hold.capture={_,_ in captures+=1}
        hold.begin(.realtimeVoiceWithAppshot,trigger:.command,lightweight:false);hold.release(.command)
        XCTAssertEqual(starts,0);XCTAssertEqual(discards,1);XCTAssertEqual(captures,0);XCTAssertFalse(hold.active)
    }
    func testCancelStopsCaptureAndDiscardsLateScreenshot() throws {
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var captured:((ApplicationContext)->Void)?,starts=0
        hold.capture={_,done in captured=done};hold.onReady={_,_,_ in starts+=1}
        hold.begin(.realtimeVoiceWithAppshot,trigger:.command,lightweight:false);hold.release(.command);hold.cancel()
        let path=FileManager.default.temporaryDirectory.appendingPathComponent("kai-held-test-\(UUID().uuidString).png")
        try Data([1]).write(to:path);defer{try? FileManager.default.removeItem(at:path)}
        captured?(ApplicationContext(text:"Private",imagePath:path.path))
        XCTAssertEqual(starts,0);XCTAssertFalse(FileManager.default.fileExists(atPath:path.path))
    }
    func testSilentOrEmptyRecordingIsDiscardedWithoutAnErrorOrVoiceStart() {
        let recorder=PromptRecorderStub();recorder.result=nil
        let hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var errors=[String](),starts=0,discards=0;hold.onReady={_,_,_ in starts+=1};hold.onError={errors.append($0)};hold.onDiscard={discards+=1}
        hold.begin(.realtimeVoice,trigger:.command,lightweight:false);hold.release(.command)
        XCTAssertEqual(starts,0);XCTAssertTrue(errors.isEmpty);XCTAssertEqual(discards,1);XCTAssertFalse(hold.active)
        recorder.onError?("late error");XCTAssertTrue(errors.isEmpty)
    }
    func testCancelledGestureCannotDeliverLevelsOrAudioIntoAnotherGesture() {
        let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var levels=0,ready=0;hold.onLevels={_ in levels+=1};hold.onReady={_,_,_ in ready+=1}
        hold.begin(.realtimeVoice,trigger:.command,lightweight:false);recorder.onLevels?([0.5])
        hold.cancel();recorder.onLevels?([0.8]);hold.release(.command)
        XCTAssertEqual(levels,1);XCTAssertEqual(ready,0)
    }
    func testAllVoiceActionsRecordWhileHeldAndPreserveTheirDestination() {
        for action in ShortcutAction.allCases where action.isVoice {
            let recorder=PromptRecorderStub(),hold=HoldToVoiceController(recorder:recorder,minimumHold:0)
            var selected:ShortcutAction?;hold.capture={_,done in done(ApplicationContext(text:"App",imagePath:nil))}
            hold.onReady={action,_,_ in selected=action}
            hold.begin(action,trigger:.function,lightweight:false)
            XCTAssertNil(selected);hold.release(.function);XCTAssertEqual(selected,action)
        }
    }
}

final class VoicePromptBufferTests:XCTestCase {
    func testSilenceQuietNoiseAndSingleClickDoNotBecomePromptsButSpeechIsRetained()throws {
        for kind in 0..<4 {
            let buffer=VoicePromptBuffer()
            for block in 0..<50 {
                let amplitude:Float=kind == 0 ? 0:kind == 1 ? 0.0004:kind == 2 ? (block == 10 ? 0.4:0):(block >= 10 && block<20 ? 0.015:0)
                let samples=(0..<960).map{amplitude*Float(sin(Double($0)*0.1))}
                XCTAssertTrue(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:48000)})
            }
            if kind == 3 {
                let wave=try XCTUnwrap(buffer.wave())
                // Keep the entire held prompt, including quiet lead-in and tail.
                XCTAssertEqual(wave.count,44+48000*2)
            }else{XCTAssertNil(buffer.wave())}
        }
    }
    func testWaveEncodingIsReadablePCMWithCorrectRateAndDuration() throws {
        let buffer=VoicePromptBuffer(),samples=(0..<4800).map{Float(sin(Double($0)/10))}
        XCTAssertTrue(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:48000)})
        let wave=try XCTUnwrap(buffer.wave()),url=FileManager.default.temporaryDirectory.appendingPathComponent("kai-synthetic-audio-\(UUID().uuidString).wav")
        try wave.write(to:url);defer{try? FileManager.default.removeItem(at:url)}
        let file=try AVAudioFile(forReading:url)
        XCTAssertEqual(file.length,4800);XCTAssertEqual(file.processingFormat.sampleRate,48000)
        XCTAssertEqual(file.processingFormat.channelCount,1);XCTAssertEqual(wave.count,44+4800*2)
    }
    func testRecordingIsBoundedRejectsRouteChangesAndSanitizesNonfiniteSamples() {
        let buffer=VoicePromptBuffer(maxSeconds:0.1),samples=[Float](repeating:0.1,count:800)
        XCTAssertTrue(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:8000)})
        XCTAssertFalse(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:8000)})
        XCTAssertFalse(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:16000)})
        XCTAssertFalse(samples.withUnsafeBufferPointer{buffer.append($0,sampleRate:.nan)})
        let safe=VoicePromptBuffer(),unsafe=[Float](repeating:.nan,count:800)
        XCTAssertTrue(unsafe.withUnsafeBufferPointer{safe.append($0,sampleRate:8000)})
        XCTAssertNil(safe.wave())
    }
}

final class CompactVoicePresentationTests:XCTestCase {
    func testVoiceDefaultsToCompactForAutomaticOpenOrCloseRequests() {
        for open in [true,false] {for expanded in [true,false] {
            let layout=CompactVoicePresentation.layout(active:true,open:open,expanded:expanded)
            XCTAssertTrue(layout.open);XCTAssertFalse(layout.expanded)
        }}
        XCTAssertTrue(CompactVoicePresentation.layout(active:false,open:true,expanded:true).expanded)
    }
    func testManualExpansionSurvivesAutomaticOpenAndCloseRequests() {
        for open in [true,false] {for expanded in [true,false] {
            let layout=CompactVoicePresentation.layout(active:true,open:open,expanded:expanded,manualExpansion:true)
            XCTAssertTrue(layout.open);XCTAssertTrue(layout.expanded)
        }}
        let collapsed=CompactVoicePresentation.layout(active:true,open:true,expanded:true,manualExpansion:false)
        XCTAssertTrue(collapsed.open);XCTAssertFalse(collapsed.expanded)
    }
    func testNormalLayoutResumesAfterVoiceEnds() {
        for open in [true,false] {for expanded in [true,false] {
            let layout=CompactVoicePresentation.layout(active:false,open:open,expanded:expanded,manualExpansion:true)
            XCTAssertEqual(layout.open,open);XCTAssertEqual(layout.expanded,expanded)
        }}
    }
    func testCompactBandAcceptsExpansionClickWithoutCapturingDesktop() {
        XCTAssertTrue(NotchInputRegion.compactAcceptsInput(open:true,expanded:false,web:false,working:false,voice:true))
        XCTAssertFalse(NotchInputRegion.mainAcceptsInput(expanded:false,web:false))
        XCTAssertFalse(NotchInputRegion.compactAcceptsInput(open:true,expanded:true,web:false,working:false,voice:true))
        XCTAssertTrue(NotchInputRegion.mainAcceptsInput(expanded:true,web:false))
    }
    func testRecordingThinkingSpeakingAndApprovalIndicators() {
        XCTAssertEqual(CompactVoicePresentation.activity(recording:true,connecting:true,backendScene:"off",voice:.thinking),.listening)
        XCTAssertEqual(CompactVoicePresentation.activity(recording:false,connecting:true,backendScene:"off",voice:.listening),.thinking)
        XCTAssertEqual(CompactVoicePresentation.activity(recording:false,connecting:false,backendScene:"thinking",voice:.speaking),.speaking)
        XCTAssertEqual(CompactVoicePresentation.activity(recording:false,connecting:false,backendScene:"permission",voice:.listening),.attention)
        XCTAssertEqual(CompactVoicePresentation.activity(recording:false,connecting:false,backendScene:"off",voice:.listening),.listening)
    }
}
