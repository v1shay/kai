import AppKit

/// A click-open, keyboard-operated picker shared by the notch's composer and
/// slash commands. It only emits existing owner-validated bridge actions.
@MainActor final class NativeAgentPicker:NSObject {
    enum Scope {case all,providers,models}
    private(set) var scope:Scope = .all
    private(set) var column=1
    private(set) var cursors=[0,0,0]
    private var state:KaiState?
    private var sourceProject="",sourceThread:String?,pendingAgent:String?
    private var selectedModel:String?
    private var panel:NotchPanel?,glass:NSVisualEffectView?,monitor:Any?,globalMonitor:Any?
    private var columnIDs=[Int]()
    private var columns=[NSView](),scrolls=[NSScrollView](),buttons=[[NSButton]](),effortRows=[EffortTileRow]()
    private var previousWindow:NSWindow?,previousResponder:NSResponder?
    var onNinja:((String,String)->Void)?
    private var ninjaQuery=""
    private var ninjaSearch:NSSearchField?
    var onAgent:((String)->Void)?,onModel:((String)->Void)?,onEffort:((String,String)->Void)?
    var visible:Bool{panel?.isVisible == true}
    private var providers:[KaiAgent]{state?.agents?.filter(\.available) ?? []}
    private var models:[KaiModel]{
        guard let state else{return []};let catalog=state.models ?? []
        let id=state.effectiveModel ?? state.providerDefaultModel
        let resolved=catalog.first{($0.resolvedModel ?? $0.model)==id}
        let label=(state.threadId == nil ? "Use ":"Continue · ")+(id ?? state.selectedAgent ?? "Agent")
        if state.selectedAgent=="ninja"{return catalog.filter{ninjaQuery.isEmpty || $0.displayName.localizedCaseInsensitiveContains(ninjaQuery)}}
        return [KaiModel(resolvedModel:id,model:"",displayName:label,supportedReasoningEfforts:resolved?.supportedReasoningEfforts ?? [])]+catalog
    }
    private var choice:KaiModel?{models.first{$0.model==selectedModel} ?? models.first{($0.resolvedModel ?? $0.model)==(state?.effectiveModel ?? state?.selectedModel)}}
    private var levels:[String]{choice?.supportedReasoningEfforts.map(\.reasoningEffort) ?? []}
    func dismiss(restoreFocus:Bool=false){
        let old=panel;panel=nil;glass=nil
        if let monitor{NSEvent.removeMonitor(monitor)};if let globalMonitor{NSEvent.removeMonitor(globalMonitor)};monitor=nil;globalMonitor=nil
        effortRows.forEach{$0.setPreview(false)};effortRows=[];columns=[];columnIDs=[];scrolls=[];buttons=[]
        old?.orderOut(nil)
        if restoreFocus,let window=previousWindow{window.makeKey();if let responder=previousResponder{window.makeFirstResponder(responder)}}
        previousWindow=nil;previousResponder=nil;pendingAgent=nil
    }
    func show(scope:Scope,state:KaiState,at anchor:CGPoint){
        if visible,self.scope==scope{dismiss(restoreFocus:true);return}
        dismiss();ninjaQuery="";if state.selectedAgent=="ninja"{onNinja?("search","")};self.scope=scope;self.state=state;sourceProject=state.project;sourceThread=state.threadId;selectedModel=state.selectedModel ?? ""
        column=scope == .providers ? 0:1
        cursors=[providers.firstIndex{$0.id==state.selectedAgent} ?? 0,models.firstIndex{$0.model==state.selectedModel} ?? 0,levels.firstIndex{$0==(state.reasoningEffort ?? state.effectiveReasoningEffort)} ?? 0]
        previousWindow=NSApp.keyWindow;previousResponder=previousWindow?.firstResponder
        let panel=NotchPanel(contentRect:CGRect(x:anchor.x,y:anchor.y-310,width:620,height:310),styleMask:[.borderless,.nonactivatingPanel],backing:.buffered,defer:false)
        panel.isOpaque=false;panel.backgroundColor = .clear;panel.hasShadow=true;panel.level = .popUpMenu;panel.collectionBehavior=[.canJoinAllSpaces,.fullScreenAuxiliary];panel.isReleasedWhenClosed=false
        let glass=NSVisualEffectView();glass.material = .popover;glass.blendingMode = .behindWindow;glass.state = .active;glass.wantsLayer=true;glass.layer?.cornerRadius=13;glass.layer?.masksToBounds=true;glass.layer?.borderWidth=0.7;glass.layer?.borderColor=NSColor.white.withAlphaComponent(0.16).cgColor
        self.panel=panel;self.glass=glass;panel.contentView=glass
        rebuild(anchor:anchor);panel.alphaValue=0;panel.makeKeyAndOrderFront(nil)
        NSAnimationContext.runAnimationGroup{c in c.duration=NSWorkspace.shared.accessibilityDisplayShouldReduceMotion ? 0:0.16;panel.animator().alphaValue=1}
        monitor=NSEvent.addLocalMonitorForEvents(matching:[.keyDown,.leftMouseDown,.rightMouseDown]){[weak self,weak panel] event in
            guard let self,let panel,self.panel === panel else{return event}
            if event.type == .keyDown,NSApp.keyWindow === panel,self.handleKey(event.keyCode,modifiers:event.modifierFlags){return nil}
            if event.type != .keyDown,event.window !== panel{DispatchQueue.main.async{[weak self,weak panel] in guard let self,let panel,self.panel === panel else{return};self.dismiss()}}
            return event
        }
        globalMonitor=NSEvent.addGlobalMonitorForEvents(matching:[.leftMouseDown,.rightMouseDown]){[weak self] _ in Task{@MainActor in self?.dismiss()}}
    }
    func update(_ state:KaiState){
        guard visible else{return}
        guard state.project==sourceProject,state.threadId==sourceThread else{dismiss();return}
        if self.state?.ownerToken != state.ownerToken,pendingAgent != state.selectedAgent{dismiss();return}
        if pendingAgent != nil,state.completedRequestId != self.state?.completedRequestId,state.completedRequestError != nil{dismiss();return}
        let changed=state.ninja != self.state?.ninja || self.state?.selectedAgent != state.selectedAgent || self.state?.selectedModel != state.selectedModel || self.state?.effectiveModel != state.effectiveModel || self.state?.reasoningEffort != state.reasoningEffort || self.state?.effectiveReasoningEffort != state.effectiveReasoningEffort || self.state?.horizontalAgentPicker != state.horizontalAgentPicker || self.state?.models?.map{$0.model+":"+$0.supportedReasoningEfforts.map(\.reasoningEffort).joined(separator:",")} != state.models?.map{$0.model+":"+$0.supportedReasoningEfforts.map(\.reasoningEffort).joined(separator:",")}
        let modelChanged=self.state?.selectedModel != state.selectedModel
        self.state=state
        if modelChanged{selectedModel=state.selectedModel ?? "";cursors[1]=models.firstIndex{$0.model==selectedModel} ?? 0}
        if let pendingAgent,pendingAgent != state.selectedAgent{return}
        if pendingAgent != nil{pendingAgent=nil;selectedModel=state.selectedModel ?? "";cursors[1]=models.firstIndex{$0.model==selectedModel} ?? 0}
        if changed{rebuild()}
    }
    private func rebuild(anchor:CGPoint?=nil){
        guard let panel,let glass,let state else{return}
        let restoreSearch=ninjaSearch?.currentEditor() != nil && panel.firstResponder === ninjaSearch?.currentEditor()
        let searchSelection=(ninjaSearch?.currentEditor() as? NSTextView)?.selectedRange()
        effortRows.forEach{$0.setPreview(false)};effortRows=[];glass.subviews.forEach{$0.removeFromSuperview()};columns=[];scrolls=[];buttons=Array(repeating:[],count:3)
        let screen=NSScreen.screens.first{$0.frame.intersects(panel.frame)}?.visibleFrame ?? NSScreen.main?.visibleFrame ?? CGRect(x:0,y:0,width:1200,height:800)
        let horizontal=state.horizontalAgentPicker != false && screen.width>=640
        let ids=scope == .providers ? [0]:scope == .models ? [1,2]:[0,1,2]
        let visibleIDs=ids.filter{$0 != 2 || !levels.isEmpty};columnIDs=visibleIDs
        let widths:[CGFloat]=[166,254,170]
        let baseWidth:CGFloat=horizontal ? visibleIDs.reduce(16){$0+widths[$1]}+CGFloat(max(0,visibleIDs.count-1))*4:280
        let ninjaSide=state.ninja != nil && scope != .providers && horizontal && screen.width>=baseWidth+244
        let width=baseWidth+(ninjaSide ? 244:0)
        let agents=providers,catalog=models,efforts=levels
        let counts=[agents.count,catalog.count,efforts.count]
        let desiredHeights=visibleIDs.map{CGFloat(min(counts[$0],8))*34+30}
        let fit=horizontal ? 1:min(1,(screen.height-32)/max(1,desiredHeights.reduce(0,+)))
        let heights=desiredHeights.map{$0*fit}
        let ninjaHeight:CGFloat=(state.ninja == nil || scope == .providers || ninjaSide) ? 0:144
        let height=min(screen.height-16,max(ninjaSide ? 224:0,(horizontal ? (heights.max() ?? 90)+16:heights.reduce(16,+))+ninjaHeight))
        let top=anchor ?? CGPoint(x:panel.frame.minX+30,y:panel.frame.maxY+4)
        let frame=CGRect(x:min(max(screen.minX+8,top.x-30),screen.maxX-width-8),y:max(screen.minY+8,min(top.y-height-4,screen.maxY-height-8)),width:width,height:height)
        panel.setFrame(frame,display:false);glass.frame=CGRect(origin:.zero,size:frame.size)
        if scope != .providers,let ninja=state.ninja{let extra=NSView(frame:CGRect(x:ninjaSide ? baseWidth:0,y:0,width:ninjaSide ? 244:width,height:ninjaSide ? height:ninjaHeight));glass.addSubview(extra);buildNinja(ninja,in:extra,width:extra.frame.width,top:extra.frame.height-8)}
        var x:CGFloat=8,y=height-8
        for (order,id) in visibleIDs.enumerated(){
            let h=horizontal ? height-16-ninjaHeight:heights[order],w=horizontal ? widths[id]:baseWidth-16
            let host=NSView(frame:CGRect(x:x,y:y-h,width:w,height:h));glass.addSubview(host);columns.append(host)
            let title=NSTextField(labelWithString:["Provider","Model","Thinking intensity"][id]);title.font=AppTypography.ui(ofSize:11,weight:.medium);title.textColor=column==id ? .controlAccentColor:.secondaryLabelColor;title.frame=CGRect(x:8,y:h-23,width:w-16,height:18);host.addSubview(title)
            let scroll=NSScrollView(frame:CGRect(x:0,y:0,width:w,height:h-29));scroll.drawsBackground=false;scroll.hasVerticalScroller=true;scroll.autohidesScrollers=true;host.addSubview(scroll);scrolls.append(scroll)
            let document=NSView(frame:CGRect(x:0,y:0,width:w,height:CGFloat(counts[id])*34));scroll.documentView=document
            for i in 0..<counts[id]{
                let rect=CGRect(x:0,y:document.frame.height-CGFloat(i+1)*34,width:w-3,height:32)
                if id==2{
                    let value=efforts[i];let row=EffortTileRow(value:value,title:EffortAppearance.title(value),index:i,count:efforts.count,selected:value==(state.reasoningEffort ?? state.effectiveReasoningEffort));row.frame=rect
                    row.layoutSubtreeIfNeeded();row.commit={[weak self] value in self?.commitEffort(value)};document.addSubview(row);effortRows.append(row)
                    row.setPreview(column==2 ? cursors[2]==i:row.selected)
                    if column==2 && cursors[2]==i{row.layer?.backgroundColor=NSColor.white.withAlphaComponent(0.09).cgColor}
                }else{
                    let label=id==0 ? agents[i].name:ModelIdentity.title(catalog[i].displayName)
                    let button=NSButton(title:label,target:self,action:#selector(selectRow(_:)));button.tag=id*10000+i;button.isBordered=false;button.alignment = .left;button.font=AppTypography.ui(ofSize:12,weight:.medium);button.lineBreakMode = .byTruncatingTail;button.frame=rect;button.wantsLayer=true;button.layer?.cornerRadius=6
                    let selected=id==0 ? agents[i].id==state.selectedAgent:catalog[i].model==selectedModel
                    button.layer?.backgroundColor=NSColor.white.withAlphaComponent(column==id && cursors[id]==i ? 0.11:selected ? 0.06:0).cgColor
                    button.image=id==0 ? ProviderIcons.image(agents[i].id):ModelIdentity.image(catalog[i].resolvedModel ?? catalog[i].model);button.image?.size=CGSize(width:16,height:16);button.imagePosition = .imageLeading
                    button.isEnabled=pendingAgent==nil || id==0;button.setAccessibilityLabel(label);document.addSubview(button);buttons[id].append(button)
                }
            }
            let cursor=min(cursors[id],max(0,counts[id]-1));document.scrollToVisible(CGRect(x:0,y:document.frame.height-CGFloat(cursor+1)*34,width:w,height:34))
            if horizontal{x+=w+4}else{y-=h}
        }
        if restoreSearch,let search=ninjaSearch{panel.makeFirstResponder(search);if let selection=searchSelection{(search.currentEditor() as? NSTextView)?.setSelectedRange(selection)}}
    }

    private func buildNinja(_ ninja:KaiNinja,in host:NSView,width:CGFloat,top:CGFloat){
        let logo=NSImageView(frame:CGRect(x:12,y:top-18,width:14,height:14));logo.image=ProviderIcons.mark("tailscale");host.addSubview(logo)
        let title=NSTextField(labelWithString:"Inference");title.font=AppTypography.ui(ofSize:11,weight:.medium);title.textColor = .secondaryLabelColor;title.frame=CGRect(x:34,y:top-18,width:width-46,height:18);host.addSubview(title)
        let values:[(String,[(String,String)],String)]=[("machine",ninja.machines.map{($0.id,$0.name+($0.online ? "":" · Offline"))},ninja.selection.machine),("engine",[("ollama","Ollama"),("llama_cpp","llama.cpp")],ninja.selection.engine),("harness",[("codex","Codex"),("claude","Claude Code")],ninja.selection.harness)]
        let narrow=width<400,extra:CGFloat=width<400 ? 32:0
        let widths:[CGFloat]=narrow ? [width-20,(width-26)/2,(width-26)/2]:[width-248,100,112];var x:CGFloat=10
        for (i,entry) in values.enumerated(){if narrow && i==1{x=10}
            let popup=NSPopUpButton(frame:CGRect(x:x,y:top-49-(narrow && i>0 ? 32:0),width:widths[i],height:26),pullsDown:false);popup.identifier=NSUserInterfaceItemIdentifier(entry.0);popup.target=self;popup.action=#selector(selectNinja(_:));popup.font=AppTypography.ui(ofSize:11)
            for (value,label) in entry.1{popup.addItem(withTitle:label);popup.lastItem?.representedObject=value;if i==0{let name=label.lowercased();let symbol=name.contains("book")||name.contains("laptop") ? "laptopcomputer":name.contains("studio") ? "macstudio":name.contains("mini") ? "macmini":name.contains("mac") ? "desktopcomputer":"server.rack";popup.lastItem?.image=NSImage(systemSymbolName:symbol,accessibilityDescription:label)}else{popup.lastItem?.image=ProviderIcons.mark(value=="ollama" ? "lab-ollama":value=="llama_cpp" ? "engine-llamacpp":value)};popup.lastItem?.image?.size=CGSize(width:14,height:14);if value==entry.2{popup.select(popup.lastItem)}}
            popup.isEnabled=ninja.progress==nil;host.addSubview(popup);x+=widths[i]+6}
        let search=ninjaSearch ?? NSSearchField();ninjaSearch=search;search.frame=CGRect(x:12,y:top-80-extra,width:width-24,height:24);search.placeholderString="Search installed models, labs, or Ollama registry";search.stringValue=ninjaQuery;search.target=self;search.action=#selector(searchNinja(_:));search.sendsSearchStringImmediately=true;host.addSubview(search)
        let status=NSTextField(labelWithString:ninja.progress ?? ninja.error ?? (ninja.loading ? "Loading model inventory…":"Installed models first · choose a family to see download tags"));status.font=AppTypography.ui(ofSize:10);status.textColor = .secondaryLabelColor;status.lineBreakMode = .byTruncatingTail;status.frame=CGRect(x:12,y:top-102-extra,width:width-(ninja.canCancel == true ? 90:24),height:17);status.toolTip=status.stringValue;host.addSubview(status)
        if ninja.canCancel == true{let stop=NSButton(title:"Stop",target:self,action:#selector(cancelNinja));stop.isBordered=false;stop.font=AppTypography.ui(ofSize:11);stop.frame=CGRect(x:width-66,y:top-105-extra,width:54,height:22);host.addSubview(stop)}
    }
    @objc private func selectNinja(_ sender:NSPopUpButton){guard let field=sender.identifier?.rawValue,let value=sender.selectedItem?.representedObject as? String else{return};onNinja?(field,value)}
    @objc private func searchNinja(_ sender:NSSearchField){ninjaQuery=sender.stringValue;cursors[1]=0;onNinja?("search",ninjaQuery);rebuild()}
    @objc private func cancelNinja(){onNinja?("cancel","")}
    private func updateHighlights(){
        guard let state else{return}
        let agents=providers,catalog=models
        for (order,id) in columnIDs.enumerated(){
            (columns[order].subviews.first as? NSTextField)?.textColor=column==id ? .controlAccentColor:.secondaryLabelColor
            if id==2{for(i,row)in effortRows.enumerated(){row.layer?.backgroundColor=NSColor.white.withAlphaComponent(column==id && cursors[id]==i ? 0.11:row.selected ? 0.06:0).cgColor;row.setPreview(column==id ? cursors[id]==i:row.selected)}}
            else{for(i,button)in buttons[id].enumerated(){let selected=id==0 ? agents[i].id==state.selectedAgent:catalog[i].model==selectedModel;button.layer?.backgroundColor=NSColor.white.withAlphaComponent(column==id && cursors[id]==i ? 0.11:selected ? 0.06:0).cgColor}}
            if let document=scrolls[order].documentView{document.scrollToVisible(CGRect(x:0,y:document.frame.height-CGFloat(cursors[id]+1)*34,width:document.frame.width,height:34))}
        }
    }
    @objc private func selectRow(_ button:NSButton){column=button.tag/10000;cursors[column]=button.tag%10000;commitColumn()}
    @discardableResult func handleKey(_ code:UInt16,modifiers:NSEvent.ModifierFlags=[])->Bool{
        guard visible,modifiers.intersection([.command,.control,.option,.shift]).isEmpty else{return false}
        switch code{
        case 53:dismiss(restoreFocus:true)
        case 126,125:
            let count=column==0 ? providers.count:column==1 ? models.count:levels.count
            cursors[column]=min(max(0,cursors[column]+(code==126 ? -1:1)),max(0,count-1));updateHighlights()
        case 123:column=column==2 ? 1:scope == .all ? 0:column;updateHighlights()
        case 124:if column != 2{commitColumn()}
        case 36,76:commitColumn()
        default:return false
        }
        return true
    }
    private func commitColumn(){
        guard let state else{return}
        switch column{
        case 0:
            guard providers.indices.contains(cursors[0]) else{return}
            let id=providers[cursors[0]].id
            if id != state.selectedAgent{ninjaQuery="";pendingAgent=id;onAgent?(id)}
            if scope == .providers{dismiss(restoreFocus:true)}else{column=1;rebuild()}
        case 1:
            guard pendingAgent==nil,models.indices.contains(cursors[1]) else{return}
            let model=models[cursors[1]];selectedModel=model.model;if state.selectedAgent=="ninja",!model.model.contains(":"){ninjaQuery=model.model+":";onNinja?("search",ninjaQuery)};onModel?(model.model)
            if model.supportedReasoningEfforts.isEmpty{if state.selectedAgent != "ninja"{dismiss(restoreFocus:true)}}else{column=2;cursors[2]=min(cursors[2],model.supportedReasoningEfforts.count-1);rebuild()}
        default:guard levels.indices.contains(cursors[2]) else{return};commitEffort(levels[cursors[2]])
        }
    }
    private func commitEffort(_ effort:String){guard pendingAgent==nil,levels.contains(effort),let model=choice else{return};onEffort?(model.model,effort);dismiss(restoreFocus:true)}
}
