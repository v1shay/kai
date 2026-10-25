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
    func present(_ review:KaiNativeReview,from anchor:NSView,send:@escaping(String,[String:Any])->Void) {
        popover.close();self.review=review;self.send=send;pending=nil
        fields.removeAll();options.removeAll();buttons=[];choiceIDs=[:]
        let controller=NSViewController(),effect=NSVisualEffectView()
        effect.material = .popover;effect.blendingMode = .behindWindow;effect.state = .active
        controller.view=effect
        let stack=NSStackView();stack.orientation = .vertical;stack.alignment = .leading;stack.spacing=12;stack.edgeInsets=NSEdgeInsets(top:16,left:16,bottom:16,right:16)
        let heading=NSTextField(wrappingLabelWithString:review.title);heading.font = .systemFont(ofSize:14,weight:.semibold);stack.addArrangedSubview(heading)
        let detail=NSTextField(wrappingLabelWithString:review.detail);detail.font = .monospacedSystemFont(ofSize:11,weight:.regular);detail.isSelectable=true;stack.addArrangedSubview(detail)
        for label in [heading,detail]{label.preferredMaxLayoutWidth=358;label.widthAnchor.constraint(equalToConstant:358).isActive=true}
        for question in review.questions {
            let label=NSTextField(wrappingLabelWithString:question.question);label.font = .systemFont(ofSize:12,weight:.medium);label.preferredMaxLayoutWidth=358;label.widthAnchor.constraint(equalToConstant:358).isActive=true;stack.addArrangedSubview(label)
            for option in question.options {
                let button=NSButton(checkboxWithTitle:option.label,target:self,action:#selector(toggleOption(_:)))
                button.toolTip=option.label;button.widthAnchor.constraint(lessThanOrEqualToConstant:358).isActive=true
                button.identifier=NSUserInterfaceItemIdentifier(question.id);options[question.id,default:[]].append((option.id,button));stack.addArrangedSubview(button)
            }
            let field:NSTextField=question.secret ? NSSecureTextField():NSTextField()
            field.placeholderString=question.options.isEmpty ? "Your answer":"Or type an answer"
            field.setAccessibilityLabel(question.header.isEmpty ? question.question:question.header)
            field.widthAnchor.constraint(equalToConstant:358).isActive=true
            field.font = .systemFont(ofSize:12);fields[question.id]=field;stack.addArrangedSubview(field)
        }
        for choice in review.choices {
            let button=NSButton(title:choice.label,target:self,action:#selector(choose(_:)));button.bezelStyle = .rounded
            choiceIDs[ObjectIdentifier(button)]=choice.id;buttons.append(button);stack.addArrangedSubview(button)
        }
        if !review.questions.isEmpty {
            let button=NSButton(title:"Submit answers",target:self,action:#selector(submit));button.bezelStyle = .rounded;buttons.append(button);stack.addArrangedSubview(button)
        }
        status.stringValue="";status.font = .systemFont(ofSize:11);status.textColor = .secondaryLabelColor;stack.addArrangedSubview(status)
        let scroll=NSScrollView();scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;scroll.documentView=stack
        scroll.translatesAutoresizingMaskIntoConstraints=false;effect.addSubview(scroll)
        stack.translatesAutoresizingMaskIntoConstraints=false
        NSLayoutConstraint.activate([scroll.leadingAnchor.constraint(equalTo:effect.leadingAnchor),scroll.trailingAnchor.constraint(equalTo:effect.trailingAnchor),scroll.topAnchor.constraint(equalTo:effect.topAnchor),scroll.bottomAnchor.constraint(equalTo:effect.bottomAnchor),stack.widthAnchor.constraint(equalToConstant:390)])
        stack.layoutSubtreeIfNeeded();let height=min(480,max(160,stack.fittingSize.height))
        stack.frame=CGRect(x:0,y:0,width:390,height:max(height,stack.fittingSize.height));effect.frame=CGRect(x:0,y:0,width:390,height:height)
        popover.contentViewController=controller;popover.contentSize=effect.frame.size;popover.behavior = .transient
        scroll.contentView.scroll(to:CGPoint(x:0,y:stack.isFlipped ? 0:max(0,stack.frame.height-height)));scroll.reflectScrolledClipView(scroll.contentView)
        popover.show(relativeTo:anchor.bounds,of:anchor,preferredEdge:.minY)
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
            if question.required && values.isEmpty{status.stringValue="Answer \(question.header.isEmpty ? question.question:question.header)";return}
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
    func update(_ state:KaiState){
        guard let review else{return}
        if state.nativeReview?.token != review.token{popover.close();self.review=nil;return}
        if let pending,state.completedRequestId == pending {
            if let error=state.completedRequestError{self.pending=nil;buttons.forEach{$0.isEnabled=true};fields.values.forEach{$0.isEnabled=true};options.values.flatMap{$0}.forEach{$0.1.isEnabled=true};status.stringValue=error}
            else{popover.close();self.review=nil}
        }
    }
}
