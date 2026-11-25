import AVFoundation
import Foundation

@MainActor protocol VoicePromptRecording:AnyObject {
    var onLevels:(([CGFloat])->Void)? {get set}
    var onError:((String)->Void)? {get set}
    func start()
    func stop()->Data?
    func cancel()
}

/// Bounded, memory-only PCM recording. No speech recognizer or transcript service.
final class VoicePromptBuffer: @unchecked Sendable {
    private let lock=NSLock()
    private var pcm=Data(),rate=0,maxSeconds:Double
    private var audibleSamples=0
    init(maxSeconds:Double=60){self.maxSeconds=maxSeconds}
    func append(_ samples:UnsafeBufferPointer<Float>,sampleRate:Double)->Bool {
        lock.lock();defer{lock.unlock()}
        guard sampleRate.isFinite,(8000...192000).contains(sampleRate) else{return false}
        let hz=Int(sampleRate.rounded())
        if rate == 0 {rate=hz}
        guard rate == hz,pcm.count + samples.count*2 <= Int(Double(rate)*maxSeconds)*2 else{return false}
        var bytes=[UInt8]();bytes.reserveCapacity(samples.count*2)
        var energy:Double=0
        for value in samples {
            let finite=value.isFinite ? max(-1,min(1,value)):0
            energy += Double(finite)*Double(finite)
            let sample=UInt16(bitPattern:Int16((finite*32767).rounded()))
            bytes.append(UInt8(sample & 255));bytes.append(UInt8(sample >> 8))
        }
        // Require sustained microphone activity, rather than treating a WAV
        // full of silence (or a single click) as a spoken first prompt.
        if !samples.isEmpty,sqrt(energy/Double(samples.count)) >= 0.003 {
            audibleSamples += samples.count
        }
        pcm.append(contentsOf:bytes);return true
    }
    func wave()->Data? {
        lock.lock();defer{lock.unlock()}
        guard rate>0,pcm.count>=rate/5,audibleSamples>=Int(Double(rate)*0.08) else{return nil}
        var result=Data()
        func word(_ value:UInt16){result.append(UInt8(value & 255));result.append(UInt8(value >> 8))}
        func dword(_ value:UInt32){for shift in stride(from:0,to:32,by:8){result.append(UInt8((value >> shift) & 255))}}
        result.append(Data("RIFF".utf8));dword(UInt32(pcm.count+36));result.append(Data("WAVEfmt ".utf8))
        dword(16);word(1);word(1);dword(UInt32(rate));dword(UInt32(rate*2));word(2);word(16)
        result.append(Data("data".utf8));dword(UInt32(pcm.count));result.append(pcm)
        return result
    }
}

@MainActor final class VoicePromptRecorder:VoicePromptRecording {
    var onLevels:(([CGFloat])->Void)?,onError:((String)->Void)?
    private var engine:AVAudioEngine?,observer:NSObjectProtocol?,buffer:VoicePromptBuffer?
    private var generation=UUID(),active=false,tapInstalled=false
    func start(){
        cancel();active=true;let token=generation
        switch AVCaptureDevice.authorizationStatus(for:.audio) {
        case .authorized:startEngine(token)
        case .notDetermined:AVCaptureDevice.requestAccess(for:.audio){[weak self] allowed in DispatchQueue.main.async{
            guard let self,self.active,self.generation == token else{return}
            if allowed{self.startEngine(token)}else{self.fail("Microphone permission is unavailable")}
        }}
        default:fail("Microphone permission is unavailable")
        }
    }
    private func startEngine(_ token:UUID){
        guard active,generation == token else{return}
        let engine=AVAudioEngine(),buffer=VoicePromptBuffer();self.engine=engine;self.buffer=buffer
        let input=engine.inputNode,format=input.inputFormat(forBus:0)
        guard format.sampleRate>0,format.channelCount>0 else{fail("No microphone input is available");return}
        input.installTap(onBus:0,bufferSize:1024,format:nil){[weak self] audio,_ in
            guard let samples=audio.floatChannelData?[0],audio.frameLength>0 else{return}
            let count=Int(audio.frameLength),values=UnsafeBufferPointer(start:samples,count:count)
            let accepted=buffer.append(values,sampleRate:audio.format.sampleRate)
            var sum:Float=0
            for value in values where value.isFinite{sum+=value*value}
            let level=min(1,CGFloat(sqrt(sum/Float(count)))*13)
            DispatchQueue.main.async{[weak self] in
                guard let self,self.active,self.generation == token else{return}
                if !accepted{self.fail("Voice prompt reached the recording limit; hold and try a shorter prompt");return}
                self.onLevels?([level*0.82,level*0.92,level,level*0.88])
            }
        };tapInstalled=true
        observer=NotificationCenter.default.addObserver(forName:.AVAudioEngineConfigurationChange,object:engine,queue:.main){[weak self] _ in
            DispatchQueue.main.async{[weak self] in
                guard let self,self.active,self.generation == token else{return}
                self.fail("Microphone changed; hold the shortcut to try again")
            }
        }
        do{engine.prepare();try engine.start()}catch{fail(error.localizedDescription)}
    }
    func stop()->Data? {
        let recorded=buffer
        cancel()
        return recorded?.wave()
    }
    func cancel(){
        generation=UUID();active=false
        if let observer{NotificationCenter.default.removeObserver(observer);self.observer=nil}
        engine?.stop()
        if tapInstalled{engine?.inputNode.removeTap(onBus:0);tapInstalled=false}
        engine=nil;buffer=nil
    }
    private func fail(_ message:String){cancel();onError?(message)}
}
