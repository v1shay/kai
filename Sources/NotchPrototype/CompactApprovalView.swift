import AppKit

struct CompactApprovalRequest:Equatable {
    let id:String,thread:String,summary:String
    var review:KaiNativeReview?
    var count=1
    var preview=false
    init?(state:KaiState){
        guard state.approval,let id=state.approvalRequestId,let thread=state.approvalThreadId else{return nil}
        self.count=state.approvalCount ?? 1;self.review=state.nativeReview;self.id=id;self.thread=thread;summary=state.approvalSummary ?? state.activity.text
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
    var displayReview:KaiNativeReview{review ?? KaiNativeReview(owner:thread,token:id,title:"Approval request",detail:summary,choices:[],questions:[],kind:nil)}
    var isQuestion:Bool{review?.isQuestion == true}
    var booleanQuestion:KaiReviewQuestion?{
        guard let questions=review?.questions,questions.count == 1,let question=questions.first,!question.multiple,Set(question.options.map(\.id)) == Set(["true","false"]) else{return nil}
        return question
    }
    /// A form is never an approval. Preserve its exact question ID and answer type.
    func response(_ action:String)->(String,[String:Any])?{
        guard isQuestion,action != "approval_stop" else{return (action,[:])}
        if let question=booleanQuestion{return ("native_answer",["answers":[question.id:[action == "approve" ? "true":"false"]]])}
        return action == "approve" ? nil:("native_cancel",[:])
    }
    static func height(notch:CGFloat)->CGFloat{notch+112}
}

@MainActor final class CompactApprovalView:NSView {
    var onCycle:(()->Void)?
    private let nextButton=ApprovalButton(action:"next",label:"Next approval request",scene:"agents")
    var onReview:((KaiNativeReview)->Void)?
    var onReviewAction:((String,[String:Any],CompactApprovalRequest)->Void)?
    var onPreferredHeightChange:(()->Void)?
    var onDecision:((String,CompactApprovalRequest)->Void)?
    private(set) var request:CompactApprovalRequest?
    private(set) var pendingID:String?
    private(set) var reviewExpanded=false,reviewPinned=false
    var maximumReviewHeight:CGFloat=480
    private let heading=NSTextField(labelWithString:""),scroll=NSScrollView(),command=NSTextView()
    private let buttons=[("approve","Approve","success"),("reject","Don't approve","failure"),("approval_stop","Stop chat","pause")].map{ApprovalButton(action:$0.0,label:$0.1,scene:$0.2)}
    private let reviewButton=ApprovalButton(action:"review",label:"Review request",scene:"review"),inlineReview=NativeRequestReview()
    private var reviewControls=[ApprovalButton]()
    private var actionRow:[ApprovalButton]{
        guard reviewExpanded,request?.booleanQuestion == nil,!reviewControls.isEmpty else{return buttons+[reviewButton]}
        let extras=request?.isQuestion == true ? Array(buttons.dropFirst()):reviewControls.contains{$0.scene == "pause"} ? []:[buttons[2]]
        return reviewControls+extras+[reviewButton]
    }
    private func updateActionRow(){
        let visible=actionRow
        for button in buttons+reviewControls+[reviewButton]{button.isHidden = !visible.contains{$0 === button}}
        needsLayout=true
    }
    private var visibilityRevision=0
    private var inlineBody:NSView?,timeout:DispatchWorkItem?,hoverClose:DispatchWorkItem?,hoverArea:NSTrackingArea?,localMonitor:Any?,globalMonitor:Any?
    var preferredHeight:CGFloat{reviewExpanded ? max(88,min(maximumReviewHeight,inlineReview.preferredHeight)+44):88}
    override init(frame:NSRect){
        super.init(frame:frame);wantsLayer=true;isHidden=true
        heading.font = .systemFont(ofSize:10,weight:.medium);heading.textColor=NSColor.white.withAlphaComponent(0.6);heading.isHidden=true;addSubview(heading)
        scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;scroll.scrollerStyle = .overlay;scroll.borderType = .noBorder
        command.isEditable=false;command.isSelectable=true;command.drawsBackground=false;command.font = .monospacedSystemFont(ofSize:11,weight:.regular);command.textColor=NSColor.white.withAlphaComponent(0.92);command.textContainerInset = .zero
        command.isVerticallyResizable=true;command.autoresizingMask = .width;command.textContainer?.widthTracksTextView=true;command.setAccessibilityLabel("Requested approval command")
        scroll.documentView=command;addSubview(scroll)
        nextButton.target=self;nextButton.action=#selector(cycle)
        reviewButton.target=self;reviewButton.action=#selector(openReview);reviewButton.isHidden=true
        reviewButton.onHover={[weak self] active in guard let self else{return};if active{self.hoverClose?.cancel();self.setReviewExpanded(true,pinned:self.reviewPinned)}else{self.scheduleHoverClose()}}
        inlineReview.onResolution={[weak self] in self?.setReviewExpanded(false)}
        for button in buttons{button.target=self;button.action=#selector(decide(_:));addSubview(button)}
    }
    required init?(coder:NSCoder){fatalError()}
    override func mouseDown(with event:NSEvent){}
    override func updateTrackingAreas(){super.updateTrackingAreas();if let hoverArea{removeTrackingArea(hoverArea)};let area=NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self);addTrackingArea(area);hoverArea=area}
    override func mouseEntered(with event:NSEvent){hoverClose?.cancel()}
    override func mouseExited(with event:NSEvent){scheduleHoverClose()}
    private func scheduleHoverClose(){
        hoverClose?.cancel();guard reviewExpanded,!reviewPinned else{return}
        let close=DispatchWorkItem{[weak self] in guard let self,self.reviewExpanded,!self.reviewPinned else{return};if let window=self.window,self.bounds.contains(self.convert(window.mouseLocationOutsideOfEventStream,from:nil)){return};self.setReviewExpanded(false)}
        hoverClose=close;DispatchQueue.main.asyncAfter(deadline:.now()+0.22,execute:close)
    }
    private func monitorOutsideClicks(){
        removeMonitors()
        localMonitor=NSEvent.addLocalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown]){[weak self] event in
            guard let self,self.reviewExpanded else{return event}
            if event.window !== self.window || !self.bounds.contains(self.convert(event.locationInWindow,from:nil)){self.setReviewExpanded(false)}
            else if self.convert(event.locationInWindow,from:nil).y>36{self.reviewPinned=true}
            return event
        }
        globalMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown]){[weak self] _ in Task{@MainActor in self?.setReviewExpanded(false)}}
    }
    private func removeMonitors(){if let localMonitor{NSEvent.removeMonitor(localMonitor)};if let globalMonitor{NSEvent.removeMonitor(globalMonitor)};localMonitor=nil;globalMonitor=nil}
    override func layout(){
        super.layout();heading.frame=CGRect(x:0,y:bounds.height-16,width:bounds.width,height:14)
        let body=CGRect(x:0,y:36,width:bounds.width,height:max(20,bounds.height-36-(heading.isHidden ? 0:20)))
        scroll.frame=body
        if let inlineBody{inlineBody.frame=body}
        command.setFrameSize(CGSize(width:scroll.contentSize.width,height:scroll.contentSize.height));command.textContainer?.containerSize=CGSize(width:scroll.contentSize.width,height:.greatestFiniteMagnitude)
        if let container=command.textContainer{command.layoutManager?.ensureLayout(for:container);command.setFrameSize(CGSize(width:scroll.contentSize.width,height:max(scroll.contentSize.height,(command.layoutManager?.usedRect(for:container).height ?? 0)+2)))}
        nextButton.frame=CGRect(x:0,y:0,width:32,height:28)
        let visible=actionRow,step=min(42,max(32,(bounds.width-(nextButton.isHidden ? 0:36))/CGFloat(max(1,visible.count)))),width=CGFloat(visible.count)*step-(step-32),start=(bounds.width-width)/2+(nextButton.isHidden ? 0:18)
        for (index,button) in visible.enumerated(){button.frame=CGRect(x:start+CGFloat(index)*step,y:0,width:32,height:28)}
    }
    private func prepareInline(){
        guard let request else{return};let review=request.displayReview
        let body=inlineReview.inlineContent(review,width:max(190,bounds.width),externalActions:true){[weak self] action,fields in
            guard let self,let current=self.request,current.id==request.id,current.thread==request.thread,self.pendingID==nil else{return}
            if let id=fields["requestId"] as? String{self.begin(id)};self.setReviewExpanded(false);self.onReviewAction?(action,fields,current)
        }
        if inlineBody !== body{inlineBody?.removeFromSuperview();inlineBody=body;addSubview(body,positioned:.below,relativeTo:heading)}
        let controls=inlineReview.actionControls
        for old in reviewControls where !controls.contains(where:{$0 === old}){old.removeFromSuperview()}
        reviewControls=controls
        for button in controls where button.superview !== self{addSubview(button)}
        updateActionRow()
    }
    func setReviewExpanded(_ expanded:Bool,pinned:Bool=false){
        guard !expanded || (request != nil && pendingID==nil) else{return}
        hoverClose?.cancel();reviewPinned=expanded && pinned
        guard reviewExpanded != expanded else{return}
        reviewExpanded=expanded
        if expanded{prepareInline();monitorOutsideClicks();reviewButton.animate(looping:true);if let review=request?.displayReview{onReview?(review)}}else{removeMonitors();reviewButton.animate(looping:false)}
        updateActionRow()
        visibilityRevision+=1;let revision=visibilityRevision,duration:TimeInterval=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0.1:0.28
        scroll.isHidden=false;inlineBody?.isHidden=false
        if expanded{inlineBody?.alphaValue=0}else{scroll.alphaValue=0}
        needsLayout=true;onPreferredHeightChange?()
        NSAnimationContext.runAnimationGroup{context in context.duration=duration;context.timingFunction=CAMediaTimingFunction(controlPoints:0.16,0.85,0.25,1);self.scroll.animator().alphaValue=expanded ? 0:1;self.inlineBody?.animator().alphaValue=expanded ? 1:0}
        DispatchQueue.main.asyncAfter(deadline:.now()+duration){[weak self] in guard let self,self.visibilityRevision==revision else{return};self.scroll.isHidden=self.reviewExpanded;self.inlineBody?.isHidden = !self.reviewExpanded}

    }
    func show(_ request:CompactApprovalRequest){
        guard self.request != request else{return}
        let changed=self.request?.id != request.id || self.request?.thread != request.thread
        if changed{setReviewExpanded(false);reviewControls.forEach{$0.removeFromSuperview()};reviewControls=[];inlineReview.dismiss();inlineBody?.removeFromSuperview();inlineBody=nil}
        self.request=request;if changed{timeout?.cancel();pendingID=nil}
        if reviewButton.superview == nil{addSubview(reviewButton)}
        reviewButton.isHidden=false
        if request.count>1{if nextButton.superview == nil{addSubview(nextButton)}}else{nextButton.removeFromSuperview()}
        nextButton.isHidden=request.count<2;nextButton.toolTip="Next approval · \(request.count) pending";nextButton.setAccessibilityLabel(nextButton.toolTip);if request.count>1{nextButton.animate()}
        command.string=request.command;command.toolTip=request.summary;scroll.contentView.scroll(to:.zero)
        if pendingID == nil{heading.stringValue="";heading.toolTip=nil;heading.isHidden=true}
        let labels=request.isQuestion ? [request.booleanQuestion == nil ? "Answer question":"Yes",request.booleanQuestion == nil ? "Cancel question":"No","Stop chat"]:["Approve","Don't approve","Stop chat"]
        for (button,label) in zip(buttons,labels){button.toolTip=label;button.setAccessibilityLabel(label)}
        buttons.forEach{$0.isEnabled=pendingID == nil;$0.alphaValue=pendingID == nil ? 1:0.45;$0.animate()};reviewButton.isEnabled=pendingID == nil;reviewButton.alphaValue=pendingID == nil ? 1:0.45;reviewButton.animate(looping:reviewExpanded);nextButton.isEnabled=pendingID == nil
        if reviewExpanded{prepareInline()};updateActionRow()
    }
    func receive(_ state:KaiState){inlineReview.update(state);finish(state.completedRequestId,error:state.completedRequestError)}
    func setProfile(_ profile:PetGradientProfile){(buttons+[reviewButton,nextButton]).forEach{$0.glyph.setProfile(profile,wave:false)};inlineReview.setProfile(profile)}
    func reset(){visibilityRevision+=1;timeout?.cancel();timeout=nil;hoverClose?.cancel();removeMonitors();reviewControls.forEach{$0.glyph.hide();$0.removeFromSuperview()};reviewControls=[];inlineReview.dismiss();inlineBody?.removeFromSuperview();inlineBody=nil;reviewExpanded=false;reviewPinned=false;request=nil;pendingID=nil;scroll.layer?.removeAllAnimations();scroll.isHidden=false;scroll.alphaValue=1;heading.isHidden=true;(buttons+[reviewButton]).forEach{$0.glyph.hide();$0.isEnabled=true;$0.alphaValue=1};nextButton.isEnabled=true;isHidden=true}
    func begin(_ id:String){
        pendingID=id;(buttons+[reviewButton]).forEach{$0.isEnabled=false;$0.alphaValue=0.45};nextButton.isEnabled=false;heading.stringValue="Waiting for confirmation…";heading.isHidden=false;needsLayout=true
        let work=DispatchWorkItem{[weak self] in guard let self,self.pendingID == id else{return};self.finish(id,error:"No confirmation yet · check the chat before retrying")};timeout=work;DispatchQueue.main.asyncAfter(deadline:.now()+12,execute:work)
    }
    func finish(_ id:String?,error:String?){
        guard let id,pendingID == id else{return};timeout?.cancel();pendingID=nil
        heading.stringValue=error ?? "";heading.toolTip=error;heading.isHidden=error == nil;(buttons+[reviewButton]).forEach{$0.isEnabled=true;$0.alphaValue=1};nextButton.isEnabled=true;needsLayout=true
    }
    @objc private func cycle(){guard pendingID == nil else{return};onCycle?()}
    @objc private func openReview(){guard pendingID==nil else{return};if reviewExpanded && reviewPinned{setReviewExpanded(false)}else{setReviewExpanded(true,pinned:true)}}
    @objc private func decide(_ sender:ApprovalButton){guard pendingID == nil,let request else{return};if request.response(sender.decision) == nil{setReviewExpanded(true,pinned:true)}else{setReviewExpanded(false);onDecision?(sender.decision,request)}}
}

@MainActor final class ApprovalButton:NSButton {
    let glyph=NotchIndicatorEngine(),decision:String,scene:String
    var onHover:((Bool)->Void)?
    override var isFlipped:Bool{false}
    init(action:String,label:String,scene:String){
        decision=action;self.scene=scene;super.init(frame:.zero);title="";isBordered=false;wantsLayer=true;layer?.cornerRadius=8
        toolTip=label;setAccessibilityLabel(label);layer?.addSublayer(glyph.layer)
    }
    required init?(coder:NSCoder){fatalError()}
    override func layout(){super.layout();glyph.layer.frame=CGRect(x:2,y:5,width:28,height:18);glyph.layout()}
    override func updateTrackingAreas(){super.updateTrackingAreas();trackingAreas.forEach(removeTrackingArea);addTrackingArea(NSTrackingArea(rect:bounds,options:[.mouseEnteredAndExited,.activeAlways],owner:self))}
    func animate(looping:Bool=false){glyph.showScene(scene,looping:looping)}
    override func mouseEntered(with event:NSEvent){guard isEnabled else{return};layer?.backgroundColor=NSColor.white.withAlphaComponent(0.09).cgColor;animate();onHover?(true)}
    override func mouseExited(with event:NSEvent){layer?.backgroundColor=nil;onHover?(false)}
}
