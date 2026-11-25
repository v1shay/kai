import AppKit
import AVFoundation

struct KaiVoiceState:Decodable {
    let threadId:String?
    let sessionId:String?
    let phase:String
    let error:String?
    let historyError:String?
}
struct KaiVoiceAudio:Decodable {
    let data:String
    let sampleRate:Int
    let numChannels:Int
    let samplesPerChannel:Int?
    let itemId:String?
    func pcm() throws -> Data {
        guard (8000...96000).contains(sampleRate),(1...2).contains(numChannels),data.utf8.count <= 256000,
              let bytes=Data(base64Encoded:data),!bytes.isEmpty,bytes.count % (2*numChannels) == 0,
              bytes.count/(2*numChannels) <= sampleRate,
              samplesPerChannel == nil || samplesPerChannel == bytes.count/(2*numChannels) else {
            throw RealtimeAudioError.invalidPacket
        }
        return bytes
    }
}
enum RealtimeAudioError:LocalizedError {
    case invalidPacket, noMicrophone, backlog
    var errorDescription:String? {
        switch self {
        case .invalidPacket:return "Voice received an invalid audio packet"
        case .noMicrophone:return "No microphone input is available"
        case .backlog:return "Voice playback fell behind; start voice again"
        }
    }
}
