import Foundation

/// Coordinates capture, navigation and context attachment before input is sent.
/// Dictation can run while a new chat loads, but sending waits for its exact ack.
@MainActor final class ShortcutPreparation {
    var onCommand: ((String, [String: Any]) -> Void)?
    var onDictation: ((DictationTrigger, Bool) -> Void)?
    var onReady: ((ShortcutAction, String, ApplicationContext?) -> Void)?
    var onFailure: ((String) -> Void)?
    var onRetainContext: ((ApplicationContext) -> Void)?
    var capture: ((Bool, @escaping (ApplicationContext) -> Void) -> Void)?
    private enum Phase { case capture, navigation, context }
    private struct Job {
        let id: UUID, action: ShortcutAction, trigger: DictationTrigger, source: String?
        var target: String?, context: ApplicationContext?, request: String?, phase: Phase
    }
    private var job: Job?, timeout: DispatchWorkItem?
    var busy: Bool { job != nil }

    static func target(selected: String?, running: String?, lastOpened: String?) -> String? {
        selected ?? running ?? lastOpened
    }

    func start(action: ShortcutAction, trigger: DictationTrigger, selected: String?, target: String?, lightweight: Bool, context:ApplicationContext?=nil) {
        cancel()
        guard action.createsChat || target != nil else { onFailure?("Open a chat before using this shortcut"); return }
        if action.isVoice, !action.createsChat, target?.hasPrefix("code:") == true {
            onFailure?("Codex voice requires a Codex chat; select one in the sidebar");return
        }
        let id=UUID()
        job=Job(id:id, action:action, trigger:trigger, source:selected, target:target, context:nil, request:nil, phase:.capture)
        let work=DispatchWorkItem { [weak self] in
            guard let self,self.job?.id == id else{return}
            self.fail("Chat preparation timed out; your input was not sent")
        }
        timeout=work;DispatchQueue.main.asyncAfter(deadline:.now()+60,execute:work)
        if action.capturesContext {
            if let context {job?.context=context;onRetainContext?(context);navigate();return}
            guard let capture else { fail("Appshot capture is unavailable");return }
            capture(lightweight) { [weak self] context in
                guard let self,self.job?.id == id else {
                    if let path=context.imagePath {try? FileManager.default.removeItem(atPath:path)}
                    return
                }
                self.job?.context=context;self.onRetainContext?(context);self.navigate()
            }
        } else { navigate() }
    }

    private func navigate() {
        guard var current=job else{return}
        let command: String? = current.action.createsChat ? "new" : current.target != current.source ? "chat" : nil
        if let command {
            let request=UUID().uuidString
            current.request=request;current.phase = .navigation;job=current
            if !current.action.isVoice {onDictation?(current.trigger,current.action.createsChat)}
            guard job?.id == current.id else{return}
            var fields: [String:Any] = ["requestId":request]
            if command == "chat",let target=current.target {fields["id"]=target}
            onCommand?(command,fields)
        } else {
            if !current.action.isVoice {onDictation?(current.trigger,false)}
            guard job?.id == current.id else{return}
            attachContextOrFinish()
        }
    }

    private func attachContextOrFinish() {
        guard var current=job,let target=current.target else {fail("No chat was selected; your input was not sent");return}
        if !current.action.isVoice,let context=current.context {
            let request=UUID().uuidString
            current.phase = .context;current.request=request;job=current
            onCommand?("context",["requestId":request,"targetThreadId":target,"threadId":target,"text":context.text,"paths":context.imagePath.map{[$0]} ?? []])
        } else { finish() }
    }

    func receive(thread: String?, request: String?, error: String?) {
        guard var current=job else{return}
        if current.phase == .capture {
            if thread != current.source {fail("Chat changed while capturing appshot; your input was not sent")}
            return
        }
        if request == current.request {
            if let error {fail(error);return}
            guard let thread, !current.action.createsChat || current.phase != .navigation || thread != current.source else {
                fail("Could not create a new chat; your input was not sent");return
            }
            if current.phase == .navigation {
                guard current.action.createsChat || thread == current.target else {fail("Chat selection changed; your input was not sent");return}
                current.target=thread;job=current;attachContextOrFinish()
            } else if thread == current.target {finish()}
            else {fail("Context belongs to a different chat; your input was not sent")}
        } else if current.phase == .context, thread != current.target {
            fail("Chat changed while attaching appshot; your input was not sent")
        }
    }

    func release(_ trigger: DictationTrigger) {
        if let current=job,current.trigger == trigger,!current.action.isVoice,current.phase == .capture {cancel()}
    }
    func cancel() { job=nil;timeout?.cancel();timeout=nil }
    private func fail(_ message: String) {cancel();onFailure?(message)}
    private func finish() {
        guard let current=job,let target=current.target else{return}
        cancel();onReady?(current.action,target,current.context)
    }
}
