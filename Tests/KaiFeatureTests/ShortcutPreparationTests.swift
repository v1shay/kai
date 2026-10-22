import XCTest
@testable import NotchPrototype

@MainActor final class ShortcutPreparationTests: XCTestCase {
    @MainActor private final class Harness {
        let input=ShortcutPreparation()
        var commands=[(String,[String:Any])](),dictation=[DictationTrigger](),ready=[(ShortcutAction,String,ApplicationContext?)](),errors=[String]()
        var captureCompletion:((ApplicationContext)->Void)?,captures=0,retained=0
        init() {
            input.onCommand={ [unowned self] in commands.append(($0,$1)) }
            input.onDictation={ [unowned self] key,_ in dictation.append(key) }
            input.onReady={ [unowned self] in ready.append(($0,$1,$2)) }
            input.onFailure={ [unowned self] in errors.append($0) }
            input.capture={ [unowned self] _,done in captures+=1;captureCompletion=done }
            input.onRetainContext={ [unowned self] _ in retained+=1 }
        }
        func start(_ action:ShortcutAction,selected:String?="old",target:String?="old") {
            input.start(action:action,trigger:.command,selected:selected,target:target,lightweight:false)
        }
        func ack(_ thread:String?,error:String?=nil) {
            input.receive(thread:thread,request:commands.last?.1["requestId"] as? String,error:error)
        }
        func captured() {captureCompletion?(ApplicationContext(text:"Application: Editor",imagePath:nil))}
    }

    func testCurrentChatDoesNotCreateChatOrCaptureContext() {
        let h=Harness();h.start(.sendActiveChat)
        XCTAssertEqual(h.dictation,[.command]);XCTAssertEqual(h.ready.first?.1,"old")
        XCTAssertEqual(h.captures,0);XCTAssertTrue(h.commands.isEmpty);XCTAssertFalse(h.input.busy)
    }
    func testNewChatWaitsForExactAckWhileAllowingDictationAndRelease() {
        let h=Harness();h.start(.sendNewChat)
        XCTAssertEqual(h.dictation,[.command]);XCTAssertEqual(h.commands.first?.0,"new");XCTAssertTrue(h.input.busy)
        h.input.release(.command)
        h.input.receive(thread:"old",request:"another-request",error:nil)
        XCTAssertTrue(h.ready.isEmpty);XCTAssertTrue(h.input.busy)
        h.ack("new");XCTAssertEqual(h.ready.first?.1,"new");XCTAssertFalse(h.input.busy)
        h.ack("new");XCTAssertEqual(h.ready.count,1)
    }
    func testAppshotNewChatOrdersCaptureNavigationAndAttachmentBeforeReady() {
        let h=Harness();h.start(.sendNewChatWithAppshot)
        XCTAssertEqual(h.captures,1);XCTAssertTrue(h.dictation.isEmpty);XCTAssertTrue(h.commands.isEmpty)
        h.captured();XCTAssertEqual(h.retained,1);XCTAssertEqual(h.commands.last?.0,"new")
        h.ack("new");XCTAssertEqual(h.commands.last?.0,"context");XCTAssertTrue(h.ready.isEmpty)
        XCTAssertEqual(h.commands.last?.1["threadId"] as? String,"new")
        XCTAssertEqual(h.commands.last?.1["targetThreadId"] as? String,"new")
        h.input.release(.command);h.ack("new")
        XCTAssertEqual(h.ready.first?.0,.sendNewChatWithAppshot);XCTAssertEqual(h.ready.first?.1,"new")
    }
    func testReleaseDuringCaptureCancelsDictationAndDeletesLateScreenshot() throws {
        let h=Harness();h.start(.sendNewChatWithAppshot);h.input.release(.command)
        let image=FileManager.default.temporaryDirectory.appendingPathComponent("kai-shortcut-test-\(UUID().uuidString).png")
        defer {try? FileManager.default.removeItem(at:image)}
        try Data([1]).write(to:image)
        h.captureCompletion?(ApplicationContext(text:"Editor",imagePath:image.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath:image.path))
        XCTAssertTrue(h.commands.isEmpty);XCTAssertTrue(h.dictation.isEmpty);XCTAssertEqual(h.retained,0)
    }
    func testVoiceWithAppshotSurvivesReleaseAndPassesContextDirectlyToVoice() {
        let h=Harness();h.start(.realtimeVoiceWithAppshot);h.input.release(.command);h.captured()
        XCTAssertTrue(h.commands.isEmpty);XCTAssertTrue(h.dictation.isEmpty)
        XCTAssertEqual(h.ready.first?.0,.realtimeVoiceWithAppshot)
        XCTAssertEqual(h.ready.first?.2?.text,"Application: Editor")
    }
    func testVoiceWithoutAppshotDoesNotCaptureOrDictate() {
        let h=Harness();h.start(.realtimeVoice)
        XCTAssertEqual(h.captures,0);XCTAssertTrue(h.commands.isEmpty);XCTAssertTrue(h.dictation.isEmpty)
        XCTAssertEqual(h.ready.first?.0,.realtimeVoice);XCTAssertNil(h.ready.first?.2)
    }
    func testLastOpenedChatMustBeSelectedBeforeVoiceCanStart() {
        let h=Harness();h.start(.realtimeVoice,selected:nil,target:"last")
        XCTAssertEqual(h.commands.last?.0,"chat");XCTAssertEqual(h.commands.last?.1["id"] as? String,"last")
        XCTAssertTrue(h.ready.isEmpty);h.ack("last");XCTAssertEqual(h.ready.first?.1,"last")
    }
    func testNewChatVoiceWaitsForCreationWithoutAnExistingChat() {
        let h=Harness();h.start(.realtimeVoiceNewChat,selected:nil,target:nil)
        XCTAssertEqual(h.commands.last?.0,"new");XCTAssertTrue(h.dictation.isEmpty)
        XCTAssertEqual(h.captures,0);XCTAssertTrue(h.ready.isEmpty)
        h.input.release(.command)
        h.input.receive(thread:nil,request:"unrelated",error:nil)
        XCTAssertTrue(h.input.busy);XCTAssertTrue(h.ready.isEmpty)
        h.ack("fresh")
        XCTAssertEqual(h.ready.first?.0,.realtimeVoiceNewChat)
        XCTAssertEqual(h.ready.first?.1,"fresh");XCTAssertNil(h.ready.first?.2)
    }
    func testNewChatVoiceWithAppshotCapturesBeforeCreationAndKeepsContextForFreshChat() {
        let h=Harness();h.start(.realtimeVoiceNewChatWithAppshot)
        XCTAssertEqual(h.captures,1);XCTAssertTrue(h.commands.isEmpty)
        h.input.release(.command);h.captured()
        XCTAssertEqual(h.commands.last?.0,"new");XCTAssertTrue(h.ready.isEmpty)
        XCTAssertTrue(h.dictation.isEmpty)
        h.input.release(.command);h.ack("fresh")
        XCTAssertEqual(h.ready.first?.0,.realtimeVoiceNewChatWithAppshot)
        XCTAssertEqual(h.ready.first?.1,"fresh")
        XCTAssertEqual(h.ready.first?.2?.text,"Application: Editor")
        XCTAssertEqual(h.commands.count,1)
    }
    func testNewChatVoiceCanLeaveCodeInsteadOfAttachingVoiceToCode() {
        for action in [ShortcutAction.realtimeVoiceNewChat,.realtimeVoiceNewChatWithAppshot] {
            let h=Harness();h.start(action,selected:"code:one",target:"code:one")
            if action.capturesContext {h.captured()}
            XCTAssertTrue(h.errors.isEmpty);XCTAssertEqual(h.commands.last?.0,"new")
            h.ack("fresh-codex")
            XCTAssertEqual(h.ready.first?.1,"fresh-codex")
        }
    }
    func testFailedOrCancelledNewChatVoiceNeverStartsInOriginalChat() {
        for action in [ShortcutAction.realtimeVoiceNewChat,.realtimeVoiceNewChatWithAppshot] {
            let failed=Harness();failed.start(action)
            if action.capturesContext {failed.captured()}
            failed.ack("old",error:"Creation failed")
            XCTAssertTrue(failed.ready.isEmpty);XCTAssertEqual(failed.errors,["Creation failed"])
            let cancelled=Harness();cancelled.start(action)
            if action.capturesContext {cancelled.captured()}
            cancelled.input.cancel();cancelled.ack("fresh")
            XCTAssertTrue(cancelled.ready.isEmpty)
        }
    }
    func testFailuresAndWrongDestinationNeverBecomeReady() {
        for error in [nil,"Connection lost"] as [String?] {
            let h=Harness();h.start(.sendNewChat);h.ack("old",error:error)
            XCTAssertFalse(h.input.busy);XCTAssertTrue(h.ready.isEmpty);XCTAssertEqual(h.errors.count,1)
        }
        let h=Harness();h.start(.sendActiveChat,selected:nil,target:"last");h.ack("other")
        XCTAssertTrue(h.ready.isEmpty);XCTAssertEqual(h.errors.count,1)
    }
    func testChatSwitchDuringCaptureOrContextCancelsInput() {
        let h=Harness();h.start(.realtimeVoiceWithAppshot)
        h.input.receive(thread:"other",request:nil,error:nil);h.captured()
        XCTAssertTrue(h.ready.isEmpty);XCTAssertTrue(h.commands.isEmpty);XCTAssertEqual(h.errors.count,1)
        let d=Harness();d.start(.sendNewChatWithAppshot);d.captured();d.ack("new")
        d.input.receive(thread:"other",request:nil,error:nil)
        XCTAssertTrue(d.ready.isEmpty);XCTAssertEqual(d.errors.count,1)
    }
    func testCancellationIgnoresLateNavigationAndContextAcks() {
        let h=Harness();h.start(.sendNewChatWithAppshot);h.captured();h.ack("new")
        h.input.cancel();h.ack("new")
        XCTAssertTrue(h.ready.isEmpty);XCTAssertFalse(h.input.busy)
    }
    func testDictationFailureDuringPreparationPreventsCreatingAnotherChat() {
        for action in [ShortcutAction.sendNewChat,.sendActiveChat] {
            let h=Harness()
            h.input.onDictation={_,_ in h.input.cancel()}
            h.start(action)
            XCTAssertTrue(h.commands.isEmpty);XCTAssertTrue(h.ready.isEmpty);XCTAssertFalse(h.input.busy)
            XCTAssertTrue(h.errors.isEmpty)
        }
    }
    func testMissingChatAndCodeVoiceFailBeforeCapture() {
        let h=Harness();h.start(.sendActiveChat,selected:nil,target:nil)
        XCTAssertEqual(h.errors.count,1);XCTAssertTrue(h.dictation.isEmpty)
        let v=Harness();v.start(.realtimeVoiceWithAppshot,selected:"code:one",target:"code:one")
        XCTAssertEqual(v.errors.count,1);XCTAssertEqual(v.captures,0)
    }
    func testTargetPrefersOpenChatThenRunningThenLastOpened() {
        XCTAssertEqual(ShortcutPreparation.target(selected:"open",running:"running",lastOpened:"last"),"open")
        XCTAssertEqual(ShortcutPreparation.target(selected:nil,running:"running",lastOpened:"last"),"running")
        XCTAssertEqual(ShortcutPreparation.target(selected:nil,running:nil,lastOpened:"last"),"last")
        XCTAssertNil(ShortcutPreparation.target(selected:nil,running:nil,lastOpened:nil))
    }
}
