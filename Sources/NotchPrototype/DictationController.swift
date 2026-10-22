import AppKit
import AVFoundation
import Speech

@MainActor final class DictationController {
    var onText:((String)->Void)?,onLevels:(([CGFloat])->Void)?,onError:((String)->Void)?
    private let recognizer=SFSpeechRecognizer(locale:.current)
    private var engine:AVAudioEngine?,configurationObserver:NSObjectProtocol?
    private var request:SFSpeechAudioBufferRecognitionRequest?,task:SFSpeechRecognitionTask?,latest="",finishWork:DispatchWorkItem?,tapInstalled=false
    private var session=0,active=false

    func start() {
        stopNow();latest="";active=true
        let token=session
        SFSpeechRecognizer.requestAuthorization { [weak self] status in DispatchQueue.main.async {
            guard let self,self.active,self.session == token else{return}
            guard status == .authorized else{self.fail("Speech recognition permission is unavailable");return}
            self.requestMic(token)
        } }
    }

    func cancel(){stopNow()}

    func stop(_ completion:@escaping(String)->Void) {
        active=false
        guard request != nil else{let result=latest;stopNow();completion(result);return}
        stopCapture();request?.endAudio();finishWork?.cancel()
        let token=session
        let work=DispatchWorkItem{[weak self] in guard let self,self.session == token else{return};let result=self.latest;self.stopNow();completion(result)}
        finishWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.42,execute:work)
    }

    private func requestMic(_ token:Int){
        guard active,session == token else{return}
        switch AVCaptureDevice.authorizationStatus(for:.audio) {
        case .authorized:startEngine(token)
        case .notDetermined:AVCaptureDevice.requestAccess(for:.audio){[weak self] ok in DispatchQueue.main.async{
            guard let self,self.active,self.session == token else{return}
            if ok{self.startEngine(token)}else{self.fail("Microphone permission is unavailable")}
        }}
        default:fail("Microphone permission is unavailable")
        }
    }

    private func startEngine(_ token:Int){
        guard active,session == token else{return}
        guard let recognizer,recognizer.isAvailable else{fail("Speech recognition is unavailable");return}
        // Recreate the graph after each session; the previous route's format can be stale.
        let engine=AVAudioEngine();self.engine=engine
        let input=engine.inputNode,hardware=input.inputFormat(forBus:0)
        guard hardware.sampleRate.isFinite,hardware.sampleRate > 0,hardware.channelCount > 0 else{fail("No microphone input is available");return}
        let request=SFSpeechAudioBufferRecognitionRequest();request.shouldReportPartialResults=true;self.request=request
        // Let Core Audio choose its native format instead of forcing a cached output rate.
        input.installTap(onBus:0,bufferSize:1024,format:nil){[weak self,request] buffer,_ in
            request.append(buffer)
            guard let samples=buffer.floatChannelData?[0] else{return}
            let count=Int(buffer.frameLength);guard count>0 else{return}
            var sum:Float=0;for i in 0..<count{let value=samples[i];if value.isFinite{sum+=value*value}}
            let level=min(1,CGFloat(sqrt(sum/Float(count)))*13)
            DispatchQueue.main.async{[weak self] in guard let self,self.active,self.session == token else{return};self.onLevels?([level*0.82,level*0.92,level,level*0.88])}
        };tapInstalled=true
        configurationObserver=NotificationCenter.default.addObserver(forName:.AVAudioEngineConfigurationChange,object:engine,queue:.main){[weak self] _ in
            DispatchQueue.main.async{[weak self] in
                guard let self,self.active,self.session == token else{return};self.fail("Microphone changed; start dictation again")
            }
        }
        do{engine.prepare();try engine.start()}catch{fail(error.localizedDescription);return}
        task=recognizer.recognitionTask(with:request){[weak self] result,error in DispatchQueue.main.async{
            guard let self,self.session == token else{return}
            if let result{self.latest=result.bestTranscription.formattedString;self.onText?(self.latest)}
            if result?.isFinal == true || error != nil {
                if let work=self.finishWork{work.perform()}
                else if let error{self.fail(error.localizedDescription)}
            }
        }}
    }

    private func fail(_ message:String){stopNow();onError?(message)}
    private func stopCapture(){
        if let observer=configurationObserver{NotificationCenter.default.removeObserver(observer);configurationObserver=nil}
        engine?.stop()
        if tapInstalled{engine?.inputNode.removeTap(onBus:0);tapInstalled=false}
    }
    private func stopNow(){active=false;session += 1;finishWork?.cancel();finishWork=nil;stopCapture();task?.cancel();task=nil;request=nil;engine=nil}
}
