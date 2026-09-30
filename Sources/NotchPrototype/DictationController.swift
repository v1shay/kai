import AppKit
import AVFoundation
import Speech

@MainActor final class DictationController {
    var onText:((String)->Void)?,onLevels:(([CGFloat])->Void)?
    private let recognizer=SFSpeechRecognizer(locale:.current),engine=AVAudioEngine()
    private var request:SFSpeechAudioBufferRecognitionRequest?,task:SFSpeechRecognitionTask?,latest="",finishWork:DispatchWorkItem?,tapInstalled=false
    private var session=0,active=false

    func start() {
        stopNow();latest="";active=true
        let token=session
        SFSpeechRecognizer.requestAuthorization { [weak self] status in DispatchQueue.main.async {
            guard let self,self.active,self.session == token,status == .authorized else{return}
            self.requestMic(token)
        } }
    }

    func stop(_ completion:@escaping(String)->Void) {
        active=false
        guard request != nil else{let result=latest;stopNow();completion(result);return}
        engine.stop();if tapInstalled{engine.inputNode.removeTap(onBus:0);tapInstalled=false};request?.endAudio();finishWork?.cancel()
        let token=session
        let work=DispatchWorkItem{[weak self] in guard let self,self.session == token else{return};let result=self.latest;self.stopNow();completion(result)};finishWork=work;DispatchQueue.main.asyncAfter(deadline:.now()+0.42,execute:work)
    }

    private func requestMic(_ token:Int){guard active,session == token else{return};switch AVCaptureDevice.authorizationStatus(for:.audio){case .authorized:startEngine(token);case .notDetermined:AVCaptureDevice.requestAccess(for:.audio){[weak self] ok in DispatchQueue.main.async{guard let self,self.active,self.session == token,ok else{return};self.startEngine(token)}};default:break}}

    private func startEngine(_ token:Int){
        guard active,session == token else{return}
        let request=SFSpeechAudioBufferRecognitionRequest();request.shouldReportPartialResults=true;self.request=request
        let input=engine.inputNode,format=input.outputFormat(forBus:0);guard format.sampleRate>0 else{return}
        input.installTap(onBus:0,bufferSize:1024,format:format){[weak self] buffer,_ in self?.request?.append(buffer);guard let samples=buffer.floatChannelData?[0] else{return};let count=Int(buffer.frameLength);guard count>0 else{return};var sum:Float=0;for i in 0..<count{sum+=samples[i]*samples[i]};let level=min(1,CGFloat(sqrt(sum/Float(count)))*13);DispatchQueue.main.async{self?.onLevels?([level*0.82,level*0.92,level,level*0.88])}};tapInstalled=true
        engine.prepare();try? engine.start()
        task=recognizer?.recognitionTask(with:request){[weak self] result,error in DispatchQueue.main.async{guard let self,self.session == token else{return};if let result{self.latest=result.bestTranscription.formattedString;self.onText?(self.latest)};if result?.isFinal==true || error != nil{self.finishWork?.perform()} }}
    }

    private func stopNow(){active=false;session += 1;finishWork?.cancel();finishWork=nil;if engine.isRunning{engine.stop()};if tapInstalled{engine.inputNode.removeTap(onBus:0);tapInstalled=false};task?.cancel();task=nil;request=nil}
}
