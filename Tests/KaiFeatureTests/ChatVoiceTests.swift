import XCTest
import AppKit
@testable import NotchPrototype

@MainActor private final class TestVoiceMedia:VoiceMedia {
    var onOffer:((String)->Void)?,onConnected:(()->Void)?,onError:((String)->Void)?,onLevels:(([CGFloat])->Void)?,onActivity:((VoiceActivity)->Void)?
    var starts=0,stops=0,activations=0,answers=[String](),capture=false
    var prompts=[Data?]()
    func start(sessionID:String,captureMicrophone:Bool,initialPrompt:Data?){starts+=1;capture=captureMicrophone;prompts.append(initialPrompt)}
    func acceptAnswer(_ sdp:String){answers.append(sdp)}
    func activate(){activations+=1}
    func stop(){stops+=1}
}
final class ChatVoiceTests:XCTestCase {
    @MainActor func testExpandedApprovalControlsRemainClickableDuringVoice()throws {
        let object:[String:Any] = ["threadId":"chat","project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadName":"Chat","history":[],"readOnly":false,"approval":true,"activity":["scene":"permission","text":"Approve command"],"activeTasks":[],"notice":"Approval needed","attachments":[]]
        let state=try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:object))
        func labels(in layer:CALayer)->[CATextLayer] {
            (layer.sublayers ?? []).flatMap{child in (child as? CATextLayer).map{[$0]} ?? labels(in:child)}
        }
        for size in [CGSize(width:354,height:257),CGSize(width:700,height:500)] {
            let ui=MiniAppInterface();ui.layout(in:CGRect(origin:.zero,size:size))
            ui.setState(state);ui.setVoiceActive(true);ui.setVisible(false,duration:0);ui.setVisible(true,duration:0)
            var decisions=[String]();ui.onApprove={decisions.append("approve")};ui.onReject={decisions.append("reject")}
            for title in ["Approve","Reject"] {
                let label=try XCTUnwrap(labels(in:ui.layer).first{$0.string as? String == title})
                let point=label.convert(CGPoint(x:label.bounds.midX,y:label.bounds.midY),to:ui.layer)
                XCTAssertTrue(ui.handleClick(point));ui.cancelPending()
            }
            XCTAssertEqual(decisions,["approve","reject"])
        }
    }
    private func state(thread:String="chat",session:String?=nil,phase:String="starting",error:String?=nil,ack:String?=nil)throws->KaiState {
        var value:[String:Any] = ["threadId":thread,"project":"/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":false,"threadName":"Chat","history":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":""],"activeTasks":[],"notice":"Ready","attachments":[]]
        if let session{value["voice"]=["sessionId":session,"threadId":thread,"phase":phase,"error":error as Any? ?? NSNull()]}
        if let ack{value["completedRequestId"]=ack;value["completedRequestError"]=error as Any? ?? NSNull()}
        return try JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:value))
    }
    @MainActor func testWebRTCUsesExistingChatAndWaitsForBackendAndMedia()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var commands=[(String,[String:Any])]();voice.onCommand={commands.append(($0,$1))}
        voice.start(try state());let id=try XCTUnwrap(voice.sessionID)
        XCTAssertEqual(media.starts,1);XCTAssertTrue(media.capture)
        XCTAssertTrue(commands.isEmpty)
        media.onOffer?("v=0\r\nOFFER")
        XCTAssertEqual(commands.first?.0,"voice_start")
        XCTAssertEqual(commands.first?.1["sdp"] as? String,"v=0\r\nOFFER")
        voice.receive(try state(session:id,phase:"live"))
        XCTAssertFalse(voice.listening);XCTAssertEqual(media.activations,0)
        voice.receiveSDP(id,"v=0\r\nANSWER");XCTAssertEqual(media.answers,["v=0\r\nANSWER"])
        media.onConnected?();XCTAssertTrue(voice.listening);XCTAssertEqual(media.activations,1)
        voice.stop();XCTAssertEqual(commands.last?.0,"voice_stop")
    }
    @MainActor func testMediaCanConnectBeforeBackendAndStaleSessionCannotApplySDP()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        voice.start(try state());let id=try XCTUnwrap(voice.sessionID)
        media.onOffer?("v=0\r\nOFFER");media.onConnected?()
        XCTAssertFalse(voice.listening)
        voice.receiveSDP("old","v=0\r\nLATE");XCTAssertTrue(media.answers.isEmpty)
        voice.receive(try state(session:id,phase:"live"));XCTAssertTrue(voice.listening)
        voice.stop();voice.receiveSDP(id,"v=0\r\nOLD");XCTAssertTrue(media.answers.isEmpty)
    }
    @MainActor func testMenuSelectionIsBoundAtStartAndChangesOnlyAffectFutureCalls()throws {
        _=NSApplication.shared
        let suite="KaiVoiceSessionTests-\(UUID().uuidString)",defaults=try XCTUnwrap(UserDefaults(suiteName:suite))
        defer{defaults.removePersistentDomain(forName:suite)}
        let picker=RealtimeVoiceMenu(defaults:defaults)
        func pick(_ choice:RealtimeVoiceChoice)throws {
            let index=try XCTUnwrap(picker.menu.items.firstIndex{$0.representedObject as? String == choice.rawValue})
            picker.menu.performActionForItem(at:index)
            XCTAssertEqual(picker.selection,choice)
        }
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var commands=[(String,[String:Any])]();voice.onCommand={commands.append(($0,$1))}
        try pick(.ember)
        voice.start(try state(),voice:picker.selection)
        let id=try XCTUnwrap(voice.sessionID)
        try pick(.maple)
        media.onOffer?("v=0\r\nOFFER")
        XCTAssertEqual(commands.last?.1["voice"] as? String,"ember")
        voice.receive(try state(session:id,phase:"live"));media.onConnected?()
        let stops=media.stops,commandCount=commands.count
        try pick(.vale)
        XCTAssertEqual(voice.sessionID,id);XCTAssertTrue(voice.listening)
        XCTAssertEqual(media.stops,stops);XCTAssertEqual(commands.count,commandCount)
        voice.stop();voice.start(try state(),voice:picker.selection);media.onOffer?("v=0\r\nOFFER")
        XCTAssertEqual(commands.last?.1["voice"] as? String,"vale")
        voice.stop();try pick(.codexDefault)
        voice.start(try state(),voice:picker.selection);media.onOffer?("v=0\r\nOFFER")
        XCTAssertNil(commands.last?.1["voice"])
        voice.stop()
    }
    @MainActor func testAuthErrorsNeverActivateAppleFallback()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        voice.start(try state());let id=try XCTUnwrap(voice.sessionID);media.onOffer?("v=0\r\nOFFER")
        voice.receive(try state(session:id,phase:"failed",error:"Authentication failed"))
        XCTAssertFalse(voice.active);XCTAssertEqual(voice.status,"Authentication failed")
        XCTAssertEqual(media.starts,1);XCTAssertEqual(media.activations,0)
    }
    @MainActor func testStopDuringPermissionPromptPreventsLateCapture()throws {
        var permission:((Bool)->Void)?
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{permission=$0})
        voice.start(try state());voice.stop();permission?(true)
        XCTAssertFalse(voice.active);XCTAssertEqual(media.starts,0)
    }
    @MainActor func testCodeChatDoesNotCreateAnotherThreadOrStartFallback()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var commands=[String]();voice.onCommand={action,_ in commands.append(action)}
        voice.start(try state(thread:"code:one"))
        XCTAssertFalse(voice.active);XCTAssertEqual(media.starts,0);XCTAssertTrue(commands.isEmpty)
    }
    @MainActor func testAppshotIsBoundToVoiceOfferAndClearedForNextSession()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var commands=[[String:Any]]();voice.onCommand={action,fields in if action == "voice_start" {commands.append(fields)}}
        voice.start(try state(),appshot:ApplicationContext(text:"Application: Editor",imagePath:"/tmp/appshot.png"))
        XCTAssertTrue(commands.isEmpty)
        media.onOffer?("v=0\r\nOFFER")
        XCTAssertEqual(commands.first?["targetThreadId"] as? String,"chat")
        let context=try XCTUnwrap(commands.first?["appshot"] as? [String:Any])
        XCTAssertEqual(context["text"] as? String,"Application: Editor")
        XCTAssertEqual(context["paths"] as? [String],["/tmp/appshot.png"])
        voice.stop();voice.start(try state());media.onOffer?("v=0\r\nOFFER")
        XCTAssertNil(commands.last?["appshot"])
    }
    @MainActor func testCancelledMicrophonePromptNeverSendsAppshot()throws {
        var permission:((Bool)->Void)?
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{permission=$0})
        var commands=[String]();voice.onCommand={action,_ in commands.append(action)}
        voice.start(try state(),appshot:ApplicationContext(text:"Application: Editor",imagePath:nil))
        voice.stop();permission?(true);media.onOffer?("v=0\r\nLATE")
        XCTAssertTrue(commands.isEmpty);XCTAssertEqual(media.starts,0)
    }
    @MainActor func testHeldAudioIsPassedOnceToNativeMediaAfterPermissionAndActivityDrivesCompactUI()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        let prompt=Data([82,73,70,70])
        voice.start(try state(),initialPrompt:prompt)
        XCTAssertEqual(media.prompts.count,1);XCTAssertEqual(media.prompts.first!,prompt)
        media.onOffer?("v=0\r\nOFFER");let session=try XCTUnwrap(voice.sessionID)
        voice.receive(try state(session:session,phase:"live"));media.onConnected?()
        media.onActivity?(.speaking);XCTAssertEqual(voice.activity,.speaking)
        media.onActivity?(.thinking);XCTAssertEqual(voice.activity,.thinking)
        voice.stop();voice.start(try state());XCTAssertNil(media.prompts.last!)
    }
    @MainActor func testTypedFirstPromptWaitsForMediaAndIsAcknowledgedBeforeClearingDraft()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var sends=[[String:Any]](),accepted=[String]()
        voice.onCommand={action,fields in if action == "send"{sends.append(fields)}}
        voice.onInitialTextAccepted={accepted.append($0)}
        voice.start(try state(),initialText:"First question")
        media.onOffer?("v=0\r\nOFFER");let id=try XCTUnwrap(voice.sessionID)
        voice.receive(try state(session:id,phase:"live"));XCTAssertTrue(sends.isEmpty)
        media.onConnected?();XCTAssertEqual(sends.count,1);XCTAssertTrue(accepted.isEmpty)
        let request=try XCTUnwrap(sends[0]["requestId"] as? String)
        XCTAssertEqual(sends[0]["targetThreadId"] as? String,"chat")
        voice.receive(try state(session:id,phase:"live",ack:request))
        XCTAssertEqual(accepted,["First question"]);XCTAssertEqual(sends.count,1)
        voice.stop()
    }
    @MainActor func testHeldAudioNewChatPipelineDoesNotConnectOrSendUntilReleaseAndNewChatAck()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)}),prep=ShortcutPreparation()
        let recorder=PipelinePromptRecorder(),held=HoldToVoiceController(recorder:recorder,minimumHold:0)
        var commands=[(String,[String:Any])](),audio:Data?
        held.capture={_,done in done(ApplicationContext(text:"Editor appshot",imagePath:nil))}
        held.onReady={action,recorded,context in
            audio=recorded
            prep.start(action:action,trigger:.function,selected:"old",target:"old",lightweight:false,context:context)
        }
        prep.onCommand={commands.append(($0,$1))}
        prep.onReady={_,target,context in voice.start(try! self.state(thread:target),appshot:context,initialPrompt:audio)}
        voice.onCommand={commands.append(($0,$1))}
        held.begin(.realtimeVoiceNewChatWithAppshot,trigger:.function,lightweight:false)
        XCTAssertEqual(recorder.starts,1);XCTAssertTrue(commands.isEmpty);XCTAssertEqual(media.starts,0)
        held.release(.function)
        XCTAssertEqual(commands.map{$0.0},["new"]);XCTAssertEqual(media.starts,0)
        let request=try XCTUnwrap(commands.first?.1["requestId"] as? String)
        prep.receive(thread:"fresh",request:request,error:nil)
        XCTAssertEqual(media.prompts.first!,Data([1,2,3]));XCTAssertEqual(media.starts,1)
        media.onOffer?("v=0\r\nOFFER")
        XCTAssertEqual(commands.last?.0,"voice_start")
        XCTAssertEqual(commands.last?.1["targetThreadId"] as? String,"fresh")
        XCTAssertEqual((commands.last?.1["appshot"] as? [String:Any])?["text"] as? String,"Editor appshot")
        voice.stop()
    }
    @MainActor func testChatSwitchAndMediaFailureStopCaptureAndBackend()throws {
        let media=TestVoiceMedia(),voice=ChatVoiceController(media:media,authorizeMicrophone:{$0(true)})
        var commands=[String]();voice.onCommand={action,_ in commands.append(action)}
        voice.start(try state());media.onOffer?("v=0\r\nOFFER")
        voice.receive(try state(thread:"other"));XCTAssertFalse(voice.active);XCTAssertEqual(commands.last,"voice_stop")
        voice.start(try state());media.onOffer?("v=0\r\nOFFER");media.onError?("Connection failed")
        XCTAssertFalse(voice.active);XCTAssertEqual(voice.status,"Connection failed");XCTAssertEqual(commands.last,"voice_stop")
    }
    func testStreamDecoderPreservesFragmentedSDPAndRejectsUnknownEnvelopes(){
        let line="{\"type\":\"voiceSDP\",\"sessionId\":\"session\",\"sdp\":\"v=0\\r\\nANSWER\"}\n"
        let decoder=KaiStateStreamDecoder();var events=[KaiBridgeEvent]()
        for byte in line.utf8{events+=decoder.receiveEvents(Data([byte]))}
        XCTAssertEqual(events.count,1)
        if case .sdp(let id,let sdp)=events[0]{XCTAssertEqual(id,"session");XCTAssertEqual(sdp,"v=0\r\nANSWER")}else{XCTFail()}
        XCTAssertTrue(decoder.receiveEvents(Data("{\"type\":\"unknown\"}\n".utf8)).isEmpty)
    }
}

@MainActor private final class PipelinePromptRecorder:VoicePromptRecording {
    var onLevels:(([CGFloat])->Void)?,onError:((String)->Void)?
    var starts=0
    func start(){starts+=1}
    func stop()->Data?{Data([1,2,3])}
    func cancel(){}
}
