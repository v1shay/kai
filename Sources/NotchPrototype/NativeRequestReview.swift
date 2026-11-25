import AppKit

struct KaiReviewOption:Decodable,Equatable {let id:String;let label:String}
struct KaiReviewQuestion:Decodable,Equatable {
    let id:String,header:String,question:String
    let options:[KaiReviewOption]
    let secret:Bool,multiple:Bool,required:Bool
}
struct KaiNativeReview:Decodable,Equatable {
    let owner:String,token:String,title:String,detail:String
    let choices:[KaiReviewOption]
    let questions:[KaiReviewQuestion]
    let kind:String?
    var isQuestion:Bool{kind == "questions" || !questions.isEmpty}
}

/// Native provider choices and forms anchored to the existing approval popout.
/// IDs stay opaque. A click is sent once, and only the owner can acknowledge it.
@MainActor final class NativeRequestReview:NSObject {
    private let popover=NSPopover()
    private var review:KaiNativeReview?
    private var fields=[String:NSTextField](),options=[String:[(String,NSButton)]]()
    private var buttons=[NSButton](),choiceIDs=[ObjectIdentifier:String]()
    private var pending:String?
    private let status=NSTextField(wrappingLabelWithString:"")
    private var send:((String,[String:Any])->Void)?
    private var content:NSView?,contentStack:NSStackView?,contentScroll:NSScrollView?
    private var embedded=false,contentWidth:CGFloat=390
    private var externalActions=false
    var actionControls:[ApprovalButton]{buttons.compactMap{$0 as? ApprovalButton}}
    private var profile:PetGradientProfile?
    func setProfile(_ profile:PetGradientProfile){self.profile=profile;buttons.compactMap{$0 as? ApprovalButton}.forEach{$0.glyph.setProfile(profile,wave:false)}}
    var onResolution:(()->Void)?
    var preferredHeight:CGFloat{min(480,max(80,contentStack?.fittingSize.height ?? 80))}
    func present(_ review:KaiNativeReview,from anchor:NSView,send:@escaping(String,[String:Any])->Void) {
        if !embedded,self.review?.token == review.token,self.review?.owner == review.owner,popover.contentViewController != nil{
            self.send=send;popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.minY);return
        }
        let view=build(review,width:390,embedded:false,send:send),controller=NSViewController();controller.view=view
        popover.contentViewController=controller;popover.contentSize=view.frame.size;popover.behavior = .transient
        popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.minY)
    }
    func inlineContent(_ review:KaiNativeReview,width:CGFloat,externalActions:Bool=false,send:@escaping(String,[String:Any])->Void)->NSView {
        if embedded,self.review==review,self.externalActions==externalActions,abs(contentWidth-width)<0.5,let content{self.send=send;return content}
        return build(review,width:width,embedded:true,externalActions:externalActions,send:send)
    }
    private func build(_ review:KaiNativeReview,width:CGFloat,embedded:Bool,externalActions:Bool=false,send:@escaping(String,[String:Any])->Void)->NSView {
        popover.close();popover.contentViewController=nil;self.review=review;self.send=send;pending=nil;self.embedded=embedded;self.externalActions=externalActions;contentWidth=width
        fields.removeAll();options.removeAll();buttons=[];choiceIDs=[:]
        let root:NSView
        if embedded{root=NSView();root.appearance=NSAppearance(named:.darkAqua)}else{let effect=NSVisualEffectView();effect.material = .popover;effect.blendingMode = .behindWindow;effect.state = .active;root=effect}
        root.frame=CGRect(x:0,y:0,width:width,height:480)
        let textWidth=max(100,width-24),stack=NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing=10;stack.edgeInsets=NSEdgeInsets(top:10,left:12,bottom:10,right:12)
        let heading=NSTextField(wrappingLabelWithString:review.title);heading.font = .systemFont(ofSize:12,weight:.semibold);stack.addArrangedSubview(heading)
        let detail=NSTextField(wrappingLabelWithString:review.detail);detail.font = .monospacedSystemFont(ofSize:11,weight:.regular);detail.isSelectable=true;stack.addArrangedSubview(detail)
        for label in [heading,detail]{label.preferredMaxLayoutWidth=textWidth;label.widthAnchor.constraint(equalToConstant:textWidth).isActive=true;label.textColor = .labelColor}
        for question in review.questions {
            let label=NSTextField(wrappingLabelWithString:question.question);label.font = .systemFont(ofSize:12,weight:.medium);label.preferredMaxLayoutWidth=textWidth;label.widthAnchor.constraint(equalToConstant:textWidth).isActive=true;stack.addArrangedSubview(label)
            for option in question.options {
                let button=NSButton(checkboxWithTitle:option.label,target:self,action:#selector(toggleOption(_:)))
                button.toolTip=option.label;button.widthAnchor.constraint(lessThanOrEqualToConstant:textWidth).isActive=true
                button.identifier=NSUserInterfaceItemIdentifier(question.id);options[question.id,default:[]].append((option.id,button));stack.addArrangedSubview(button)
            }
            let field:NSTextField=question.secret ? NSSecureTextField():NSTextField()
            field.placeholderString=question.options.isEmpty ? "Your answer":"Or type an answer"
            field.setAccessibilityLabel(question.header.isEmpty ? question.question:question.header)
            field.widthAnchor.constraint(equalToConstant:textWidth).isActive=true
            field.font = .systemFont(ofSize:12);fields[question.id]=field;stack.addArrangedSubview(field)
        }
        let choices=NSStackView();choices.orientation = .horizontal;choices.spacing=12
        for choice in review.choices {
            let name=(choice.id+" "+choice.label).lowercased()
            let scene=name.contains("stop") || name.contains("cancel") ? "pause":name.contains("decline") || name.contains("reject") ? "failure":name.contains("matching") || name.contains("always") || name.contains("session") ? "permission":"success"
            let button=ApprovalButton(action:choice.id,label:choice.label,scene:scene);button.target=self;button.action=#selector(choose(_:));if !externalActions{button.widthAnchor.constraint(equalToConstant:32).isActive=true;button.heightAnchor.constraint(equalToConstant:28).isActive=true}
            if let profile{button.glyph.setProfile(profile,wave:false)};button.animate()
            choiceIDs[ObjectIdentifier(button)]=choice.id;buttons.append(button);if !externalActions{choices.addArrangedSubview(button)}
        }
        if !externalActions && !review.choices.isEmpty{stack.addArrangedSubview(choices)}
        if review.isQuestion {
            let button=ApprovalButton(action:"submit",label:review.questions.isEmpty ? "Continue":"Submit answers",scene:"success");button.target=self;button.action=#selector(submit);if !externalActions{button.widthAnchor.constraint(equalToConstant:32).isActive=true;button.heightAnchor.constraint(equalToConstant:28).isActive=true};if let profile{button.glyph.setProfile(profile,wave:false)};button.animate();buttons.append(button);if !externalActions{stack.addArrangedSubview(button)}
        }
        status.constraints.filter{$0.firstAttribute == .width && $0.secondItem == nil}.forEach{$0.isActive=false};status.stringValue="";status.font = .systemFont(ofSize:11);status.textColor = .secondaryLabelColor;status.preferredMaxLayoutWidth=textWidth;status.widthAnchor.constraint(equalToConstant:textWidth).isActive=true;stack.addArrangedSubview(status)
        let scroll=NSScrollView();scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;scroll.scrollerStyle = .overlay;scroll.documentView=stack
        scroll.translatesAutoresizingMaskIntoConstraints=false;root.addSubview(scroll)
        stack.translatesAutoresizingMaskIntoConstraints=false
        NSLayoutConstraint.activate([scroll.leadingAnchor.constraint(equalTo:root.leadingAnchor),scroll.trailingAnchor.constraint(equalTo:root.trailingAnchor),scroll.topAnchor.constraint(equalTo:root.topAnchor),scroll.bottomAnchor.constraint(equalTo:root.bottomAnchor),stack.widthAnchor.constraint(equalToConstant:width)])
        content=root;contentStack=stack;contentScroll=scroll
        refreshLayout(resetScroll:true);return root
    }
    private func refreshLayout(resetScroll:Bool=false){
        guard let content,let stack=contentStack,let scroll=contentScroll else{return}
        stack.layoutSubtreeIfNeeded();let actual=max(80,stack.fittingSize.height),height=min(480,actual)
        stack.frame=CGRect(x:0,y:0,width:contentWidth,height:actual);content.frame.size=CGSize(width:contentWidth,height:height);content.layoutSubtreeIfNeeded()
        if resetScroll{scroll.contentView.scroll(to:CGPoint(x:0,y:stack.isFlipped ? 0:max(0,actual-height)));scroll.reflectScrolledClipView(scroll.contentView)}
    }
    @objc private func toggleOption(_ sender:NSButton){
        guard let question=review?.questions.first(where:{$0.id == sender.identifier?.rawValue}),!question.multiple else{return}
        for (_,button) in options[question.id] ?? [] where button !== sender{button.state = .off}
    }
    @objc private func choose(_ button:NSButton){guard let id=choiceIDs[ObjectIdentifier(button)] else{return};issue(["choice":id])}
    @objc private func submit(){
        guard let review else{return};var answers=[String:[String]]()
        for question in review.questions {
            var values=(options[question.id] ?? []).filter{$0.1.state == .on}.map{$0.0}
            let text=fields[question.id]?.stringValue.trimmingCharacters(in:.whitespacesAndNewlines) ?? ""
            if !text.isEmpty{if !question.multiple{values=[]};values.append(text)}
            if question.required && values.isEmpty{status.stringValue="Answer \(question.header.isEmpty ? question.question:question.header)";refreshLayout();return}
            answers[question.id]=values
        }
        issue(["answers":answers])
    }
    private func issue(_ answer:[String:Any]){
        guard pending == nil,let review else{return};let id=UUID().uuidString;pending=id
        buttons.forEach{$0.isEnabled=false};fields.values.forEach{$0.isEnabled=false};options.values.flatMap{$0}.forEach{$0.1.isEnabled=false};status.stringValue="Sending to agent…"
        var fields=answer;fields["requestId"]=id;fields["threadId"]=review.owner;fields["approvalRequestId"]=review.token
        send?("native_answer",fields)
    }
    func dismiss(){popover.close();popover.contentViewController=nil;review=nil;pending=nil;send=nil;content?.removeFromSuperview();content=nil;contentStack=nil;contentScroll=nil}
    func update(_ state:KaiState){
        guard let review else{return}
        if state.nativeReview?.token != review.token || state.nativeReview?.owner != review.owner{dismiss();onResolution?();return}
        if let pending,let result=state.requestResult(pending) {
            if let error=result.error{self.pending=nil;buttons.forEach{$0.isEnabled=true};fields.values.forEach{$0.isEnabled=true};options.values.flatMap{$0}.forEach{$0.1.isEnabled=true};status.stringValue=error;refreshLayout()}
            else{dismiss();onResolution?()}
        }
    }
}
