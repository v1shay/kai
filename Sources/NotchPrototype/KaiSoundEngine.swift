import AVFoundation

@MainActor final class KaiSoundEngine {
    private var player:AVAudioPlayer?,listening=false,playToken=0
    private let sampleRate=48_000.0
    var enabled=true { didSet {if !enabled{stopAll()} } }

    func startListening(){
        guard enabled,!listening else{return};listening=true;play(.listening)
    }
    func stopListening(){listening=false}
    func playScene(_ scene:String){
        guard enabled else{return}
        if scene != "listening"{stopListening()}
        switch scene {
        case "listening":startListening()
        case "thinking","waiting":play(.thinking)
        case "success":play(.success)
        case "failure","declined":play(.failure)
        case "warning":play(.warning)
        case "permission","question":play(.permission)
        case "off":break
        default:play(.working)
        }
    }
    func stopAll(){playToken += 1;listening=false;player?.stop();player=nil}

    private enum Cue {case listening,thinking,working,success,failure,warning,permission}
    private func play(_ sound:Cue){
        guard enabled else{return};playToken += 1;let token=playToken;player?.stop();let audio:(Data,Double)
        switch sound {
        case .listening:audio=(boops(0.25,[(185,0,16,0.68)]),0.25)
        case .thinking:audio=(boops(0.3,[(205,0,24,0.78)]),0.3)
        case .working:audio=(boops(0.22,[(190,0,10,0.72)]),0.22)
        case .success:audio=(boops(0.42,[(215,0,16,0.7),(275,0.13,12,0.62)]),0.42)
        case .failure:audio=(boops(0.34,[(185,0,-38,0.78)]),0.34)
        case .warning:audio=(boops(0.4,[(175,0,0,0.68),(175,0.15,0,0.5)]),0.4)
        case .permission:audio=(boops(0.42,[(200,0,12,0.66),(235,0.16,15,0.54)]),0.42)
        };guard let next=try? AVAudioPlayer(data:audio.0) else{return};player=next;next.volume=0.72;next.prepareToPlay();next.play();DispatchQueue.main.asyncAfter(deadline:.now()+audio.1+0.08){[weak self] in guard let self,self.playToken==token else{return};self.player=nil}
    }
    private func boops(_ duration:Double,_ notes:[(frequency:Double,delay:Double,bend:Double,gain:Double)])->Data {
        wav(duration){t in var value=0.0;for note in notes {let x=t-note.delay;guard x>=0 else{continue};let attack=1-exp(-55*x),decay=exp(-13*x),phase=Double.pi*2*(note.frequency*x+note.bend*x*x/2),body=sin(phase)+0.1*sin(phase*2);value += body*attack*decay*note.gain};return Float(value*min(1,(duration-t)/0.035)*0.085)}
    }
    private func wav(_ duration:Double,_ sample:(Double)->Float)->Data {
        let frames=Int(duration*sampleRate),dataSize=frames*4;var data=Data()
        func bytes<T:FixedWidthInteger>(_ value:T)->Data {var x=value.littleEndian;return Data(bytes:&x,count:MemoryLayout<T>.size)}
        data.append("RIFF".data(using:.ascii)!);data.append(bytes(UInt32(36+dataSize)));data.append("WAVEfmt ".data(using:.ascii)!);data.append(bytes(UInt32(16)));data.append(bytes(UInt16(1)));data.append(bytes(UInt16(2)));data.append(bytes(UInt32(sampleRate)));data.append(bytes(UInt32(sampleRate*4)));data.append(bytes(UInt16(4)));data.append(bytes(UInt16(16)));data.append("data".data(using:.ascii)!);data.append(bytes(UInt32(dataSize)))
        for i in 0..<frames {let value=Int16(max(-1,min(1,sample(Double(i)/sampleRate)))*Float(Int16.max));let encoded=bytes(value);data.append(encoded);data.append(encoded)};return data
    }
}
