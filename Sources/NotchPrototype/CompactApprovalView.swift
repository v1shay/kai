import AppKit

struct CompactApprovalRequest:Equatable {
    let id:String,thread:String,summary:String
    var preview=false
    init?(state:KaiState){
        guard state.approval,let id=state.approvalRequestId,let thread=state.approvalThreadId else{return nil}
        self.id=id;self.thread=thread;summary=state.approvalSummary ?? state.activity.text
    }
    init(preview:Bool){id="preview";thread="preview";summary="swift test --filter CompactApprovalTests";self.preview=preview}
    var command:String {
        guard let data=summary.data(using:.utf8),let object=try? JSONSerialization.jsonObject(with:data) as? [String:Any] else{return summary}
        if let command=object["command"] as? String{return command}
        // Array arguments stay JSON-quoted; joining them could conceal spaces.
        if let command=object["command"] as? [String],let data=try? JSONSerialization.data(withJSONObject:command,options:[.prettyPrinted,.withoutEscapingSlashes]),let text=String(data:data,encoding:.utf8){return text}
        return summary
    }
    func matches(_ state:KaiState)->Bool{!preview && state.approval && state.approvalRequestId == id && state.approvalThreadId == thread}
    static func height(notch:CGFloat)->CGFloat{notch+112}
}

@MainActor final class CompactApprovalView:NSView {
    var onDecision:((String,CompactApprovalRequest)->Void)?
    private(set) var request:CompactApprovalRequest?
    private(set) var pendingID:String?
    private let heading=NSTextField(labelWithString:""),scroll=NSScrollView(),command=NSTextView()
    private let buttons=[("approve","Approve","success"),("reject","Don't approve","failure"),("approval_stop","Stop chat","pause")].map{ApprovalButton(action:$0.0,label:$0.1,scene:$0.2)}
    private var timeout:DispatchWorkItem?
    override init(frame:NSRect){
        super.init(frame:frame);wantsLayer=true;isHidden=true
        heading.font = .systemFont(ofSize:10,weight:.medium);heading.textColor=NSColor.white.withAlphaComponent(0.6);heading.isHidden=true;addSubview(heading)
        scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;scroll.borderType = .noBorder
        command.isEditable=false;command.isSelectable=true;command.drawsBackground=false;command.font = .monospacedSystemFont(ofSize:11,weight:.regular);command.textColor=NSColor.white.withAlphaComponent(0.92);command.textContainerInset = .zero
        command.isVerticallyResizable=true;command.autoresizingMask = .width;command.textContainer?.widthTracksTextView=true;command.setAccessibilityLabel("Requested approval command")
        scroll.documentView=command;addSubview(scroll)
        for button in buttons{button.target=self;button.action=#selector(decide(_:));addSubview(button)}
    }
    required init?(coder:NSCoder){fatalError()}
    override func mouseDown(with event:NSEvent){}
    override func layout(){
        super.layout();heading.frame=CGRect(x:0,y:bounds.height-14,width:bounds.width,height:12)
        scroll.frame=CGRect(x:0,y:36,width:bounds.width,height:max(20,bounds.height-55))
        command.setFrameSize(CGSize(width:scroll.contentSize.width,height:scroll.contentSize.height));command.textContainer?.containerSize=CGSize(width:scroll.contentSize.width,height:.greatestFiniteMagnitude)
        if let container=command.textContainer{command.layoutManager?.ensureLayout(for:container);command.setFrameSize(CGSize(width:scroll.contentSize.width,height:max(scroll.contentSize.height,(command.layoutManager?.usedRect(for:container).height ?? 0)+2)))}
        for (index,button) in buttons.enumerated(){button.frame=CGRect(x:bounds.midX-58+CGFloat(index)*42,y:0,width:32,height:28)}
    }
    func show(_ request:CompactApprovalRequest){
        guard self.request != request else{return};self.request=request;timeout?.cancel();pendingID=nil
        command.string=request.command;command.toolTip=request.summary;scroll.contentView.scroll(to:.zero)
        heading.stringValue="";heading.toolTip=nil;heading.isHidden=true
        buttons.forEach{$0.isEnabled=true;$0.alphaValue=1;$0.animate()};needsLayout=true
    }
    func setProfile(_ profile:PetGradientProfile){buttons.forEach{$0.glyph.setProfile(profile,wave:false)}}
    func reset(){timeout?.cancel();timeout=nil;request=nil;pendingID=nil;buttons.forEach{$0.glyph.hide()};isHidden=true}
    func begin(_ id:String){
        pendingID=id;buttons.forEach{$0.isEnabled=false;$0.alphaValue=0.45};heading.stringValue="Waiting for confirmation…";heading.isHidden=false
        let work=DispatchWorkItem{[weak self] in guard let self,self.pendingID == id else{return};self.finish(id,error:"No confirmation yet · check the chat before retrying")};timeout=work;DispatchQueue.main.asyncAfter(deadline:.now()+12,execute:work)
    }
    func finish(_ id:String?,error:String?){
        guard let id,pendingID == id else{return};timeout?.cancel();pendingID=nil
        heading.stringValue=error ?? "";heading.toolTip=error;heading.isHidden=error == nil;buttons.forEach{$0.isEnabled=true;$0.alphaValue=1}
    }
    @objc private func decide(_ sender:ApprovalButton){guard pendingID == nil,let request else{return};onDecision?(sender.decision,request)}
}

@MainActor private final class ApprovalButton:NSButton {
    let glyph=NotchIndicatorEngine(),decision:String,scene:String
    override var isFlipped:Bool{false}
    init(action:String,label:String,scene:String){
        decision=action;self.scene=scene;super.init(frame:.zero);title="";isBordered=false;wantsLayer=true;layer?.cornerRadius=8
        toolTip=label;setAccessibilityLabel(label);layer?.addSublayer(glyph.layer)
    }
    required init?(coder:NSCoder){fatalError()}
    override func layout(){super.layout();glyph.layer.frame=CGRect(x:2,y:5,width:28,height:18);glyph.layout()}
    override func updateTrackingAreas(){super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea);addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self))}
    func animate(){glyph.showScene(scene,looping:false)}
    override func mouseEntered(with event:NSEvent){guard isEnabled else{return};layer?.backgroundColor=NSColor.white.withAlphaComponent(0.09).cgColor;animate()}
    override func mouseExited(with event:NSEvent){layer?.backgroundColor=nil}
}
