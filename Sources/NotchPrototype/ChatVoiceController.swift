import AppKit
import AVFoundation

/// The same Codex thread supplies voice, transcripts, delegated work and approvals.
/// Authentication and token refresh stay inside the headless App Server.
@MainActor final class ChatVoiceController {
    var onCommand:((String,[String:Any])->Void)?
    var onStatus:((String)->Void)?
    var onLevels:(([CGFloat])->Void)?
    var onChange:(()->Void)?
    var onInitialTextAccepted:((String)->Void)?
    private let media:VoiceMedia
    private let authorizeMicrophone:(@escaping(Bool)->Void)->Void
    private(set) var sessionID:String?,threadID:String?,status="Voice off"
    private(set) var mode=Mode.off
    enum Mode {case off,connecting,realtime}
    var active:Bool {mode != .off}
    var listening:Bool {mode == .realtime}
    private var generation=UUID(),timeout:DispatchWorkItem?
    private var backendStarted=false,mediaConnected=false,startIssued=false
    private var appshot:ApplicationContext?
    private var initialPrompt:Data?
    private var initialText:String?,initialTextRequest:String?
    private var voiceChoice=RealtimeVoiceChoice.codexDefault
    private(set) var activity=VoiceActivity.listening

    init(media:VoiceMedia?=nil,authorizeMicrophone:((@escaping(Bool)->Void)->Void)?=nil){
        self.media=media ?? WebRTCVoiceMedia()
        self.authorizeMicrophone=authorizeMicrophone ?? { completion in
            switch AVCaptureDevice.authorizationStatus(for:.audio) {
            case .authorized:completion(true)
            case .notDetermined:AVCaptureDevice.requestAccess(for:.audio){allowed in DispatchQueue.main.async{completion(allowed)}}
            default:completion(false)
            }
        }
        self.media.onOffer={ [weak self] sdp in
            guard let self,self.mode == .connecting,!self.startIssued,let session=self.sessionID else{return}
            self.startIssued=true
            var fields:[String:Any] = ["sessionId":session,"requestId":session,"sdp":sdp]
            fields["targetThreadId"]=self.threadID
            if let voice=self.voiceChoice.apiValue {fields["voice"]=voice}
            if let context=self.appshot {fields["appshot"]=["text":String(context.text.prefix(12000)),"paths":context.imagePath.map{[$0]} ?? []]}
            self.onCommand?("voice_start",fields)
        }
        self.media.onConnected={ [weak self] in self?.mediaConnected=true;self?.activateIfReady() }
        self.media.onLevels={ [weak self] in self?.onLevels?($0) }
        self.media.onActivity={ [weak self] activity in
            guard let self,self.active else{return}
            self.activity=activity;self.onChange?()
        }
        self.media.onError={ [weak self] in self?.fail($0) }
    }
    func attachMedia(to host:NSView?){(media as? WebRTCVoiceMedia)?.hostView=host}
    private func report(_ value:String){status=value;onStatus?(value);onChange?()}

    func start(_ state:KaiState,appshot:ApplicationContext?=nil,initialPrompt:Data?=nil,initialText:String?=nil,voice:RealtimeVoiceChoice = .codexDefault){
        stop()
        guard let thread=state.threadId else{report("Select a Codex chat to start voice");return}
        guard !thread.hasPrefix("code:") else{report("Codex voice requires a Codex chat; select one in the sidebar");return}
        guard !state.readOnly else{report("This chat is read-only; select a writable Codex chat");return}
        if appshot != nil,state.activeTurnId != nil {report("Wait for this turn to finish before starting voice with appshot");return}
        self.appshot=appshot
        self.voiceChoice=voice
        self.initialPrompt=initialPrompt;activity = .thinking
        self.initialText=initialText?.trimmingCharacters(in:.whitespacesAndNewlines)
        threadID=thread;sessionID=UUID().uuidString;generation=UUID()
        mode = .connecting;report("Connecting Codex voice…")
        let token=generation
        authorizeMicrophone{ [weak self] allowed in
            guard let self,self.generation == token,self.mode == .connecting,let session=self.sessionID else{return}
            guard allowed else{self.fail("Microphone permission is unavailable");return}
            let work=DispatchWorkItem{[weak self] in
                guard let self,self.generation == token,self.mode == .connecting else{return}
                self.fail("Codex voice connection timed out")
            };self.timeout=work;DispatchQueue.main.asyncAfter(deadline:.now()+45,execute:work)
            self.media.start(sessionID:session,captureMicrophone:true,initialPrompt:self.initialPrompt)
            self.initialPrompt=nil
        }
    }
    func receive(_ state:KaiState){
        guard active else{return}
        guard state.threadId == threadID else{stop();return}
        if let request=initialTextRequest,state.completedRequestId == request {
            if let error=state.completedRequestError{fail(error);return}
            let accepted=initialText;initialText=nil;initialTextRequest=nil
            if let accepted{onInitialTextAccepted?(accepted)}
        }
        guard let voice=state.voice,voice.sessionId == sessionID else{
            if startIssued,state.completedRequestId == sessionID{fail(state.completedRequestError ?? state.notice)}
            return
        }
        switch voice.phase {
        case "live":backendStarted=true;activateIfReady()
        case "failed":fail(voice.error ?? "Codex voice failed")
        case "off":if let error=voice.error{fail(error)}else{stop()}
        case "stopping":media.stop()
        default:break
        }
    }
    func receiveSDP(_ session:String,_ sdp:String){
        guard active,startIssued,session == sessionID else{return}
        media.acceptAnswer(sdp)
    }
    private func activateIfReady(){
        guard mode == .connecting,backendStarted,mediaConnected else{return}
        timeout?.cancel();timeout=nil;media.activate();mode = .realtime
        report("Codex voice · listening")
        if let text=initialText,!text.isEmpty,let thread=threadID,let session=sessionID {
            let request=session+":prompt";initialTextRequest=request
            onCommand?("send",["text":text,"requestId":request,"targetThreadId":thread])
        }
    }
    func stopSpeaking(){if active{stop()}}
    func stop(){
        let prior=sessionID,issued=startIssued
        generation=UUID();timeout?.cancel();timeout=nil
        mode = .off;sessionID=nil;threadID=nil;startIssued=false
        backendStarted=false;mediaConnected=false;media.stop()
        appshot=nil
        voiceChoice = .codexDefault
        initialPrompt=nil;activity = .listening
        initialText=nil;initialTextRequest=nil
        if issued,let prior{onCommand?("voice_stop",["sessionId":prior])}
        report("Voice off")
    }
    private func fail(_ message:String){stop();report(message)}
}
