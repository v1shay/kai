import Foundation

/// Records first, connects after release. Capture and audio callbacks are scoped
/// to one gesture so neither can start a cancelled or different conversation.
@MainActor final class HoldToVoiceController {
    enum Phase {case off,recording,finishing}
    var onReady:((ShortcutAction,Data,ApplicationContext?)->Void)?
    var onChange:(()->Void)?,onError:((String)->Void)?,onLevels:(([CGFloat])->Void)?
    var onDiscard:(()->Void)?
    var capture:((Bool,@escaping(ApplicationContext)->Void)->Void)?
    private(set) var phase=Phase.off
    var active:Bool {phase != .off}
    private let recorder:VoicePromptRecording,clock:()->TimeInterval,defaultMinimumHold:TimeInterval
    private var minimumHold:TimeInterval=0.25
    private var generation=UUID(),startedAt:TimeInterval=0,trigger:DictationTrigger?,action:ShortcutAction?
    private var context:ApplicationContext?,audio:Data?,captureDone=false,armWork:DispatchWorkItem?,timeout:DispatchWorkItem?
    init(recorder:VoicePromptRecording?=nil,minimumHold:TimeInterval=0.25,clock:@escaping()->TimeInterval={ProcessInfo.processInfo.systemUptime}){
        self.recorder=recorder ?? VoicePromptRecorder();self.defaultMinimumHold=minimumHold;self.clock=clock
        self.recorder.onLevels={ [weak self] levels in guard let self,self.phase == .recording else{return};self.onLevels?(levels) }
        self.recorder.onError={ [weak self] error in guard let self,self.active else{return};self.fail(error) }
    }
    func begin(_ action:ShortcutAction,trigger:DictationTrigger,lightweight:Bool,minimumHold:TimeInterval?=nil){
        cancel();guard action.isVoice else{return}
        self.minimumHold=minimumHold.flatMap{$0.isFinite ? min(5,max(0,$0)):nil} ?? defaultMinimumHold
        self.action=action;self.trigger=trigger;startedAt=clock();phase = .recording
        captureDone = !action.capturesContext
        let token=generation
        let arm=DispatchWorkItem{[weak self] in
            guard let self,self.generation == token,self.active else{return}
            guard action.capturesContext else{return}
            guard let capture=self.capture else{self.fail("Appshot capture is unavailable");return}
            capture(lightweight){[weak self] context in
                guard let self,self.generation == token,self.active else{
                    if let path=context.imagePath{try? FileManager.default.removeItem(atPath:path)};return
                }
                self.context=context;self.captureDone=true;self.finishIfReady()
            }
        }
        armWork=arm
        if self.minimumHold == 0 {arm.perform()}else{DispatchQueue.main.asyncAfter(deadline:.now()+self.minimumHold,execute:arm)}
        let timeout=DispatchWorkItem{[weak self] in
            guard let self,self.generation == token,self.active else{return}
            self.fail("Voice recording timed out; hold the shortcut to try again")
        };self.timeout=timeout;DispatchQueue.main.asyncAfter(deadline:.now()+65,execute:timeout)
        recorder.start();if active{onChange?()}
    }
    func release(_ trigger:DictationTrigger){
        guard self.trigger == trigger,phase == .recording else{return}
        guard clock()-startedAt >= minimumHold else{cancel();onDiscard?();return}
        phase = .finishing
        audio=recorder.stop()
        guard audio != nil else{cancel();onDiscard?();return}
        onChange?();finishIfReady()
    }
    func cancel(){
        generation=UUID();armWork?.cancel();armWork=nil;timeout?.cancel();timeout=nil
        recorder.cancel();phase = .off;trigger=nil;action=nil;audio=nil;captureDone=false
        if let path=context?.imagePath{try? FileManager.default.removeItem(atPath:path)};context=nil
        onChange?()
    }
    private func finishIfReady(){
        guard phase == .finishing,captureDone,let action,let audio else{return}
        let retained=context;context=nil
        cancel();onReady?(action,audio,retained)
    }
    private func fail(_ error:String){cancel();onError?(error)}
}
