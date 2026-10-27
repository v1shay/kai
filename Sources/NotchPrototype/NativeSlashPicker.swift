import AppKit

struct KaiSlashCommand:Decodable,Equatable {let name:String;let description:String}
struct NativeSlashToken:Equatable {
    let range:NSRange,query:String
    static func at(_ text:String,selection:NSRange)->NativeSlashToken?{
        let value=text as NSString
        guard selection.length == 0,selection.location<=value.length else{return nil}
        var caret=selection.location
        if caret>0,value.substring(with:NSRange(location:caret-1,length:1)) == " "{caret-=1}
        let prefix=value.substring(to:caret),start=(prefix as NSString).rangeOfCharacter(from:.whitespacesAndNewlines,options:.backwards).location
        let beginning=start == NSNotFound ? 0:start+1
        let suffix=value.substring(from:caret) as NSString,end=suffix.rangeOfCharacter(from:.whitespacesAndNewlines).location
        let finish=end == NSNotFound ? value.length:caret+end
        let range=NSRange(location:beginning,length:finish-beginning),token=value.substring(with:range)
        guard caret>beginning,token.hasPrefix("/"),token.utf8.count<=65,token.dropFirst().utf8.allSatisfy({(65...90).contains($0) || (97...122).contains($0) || (48...57).contains($0) || [45,95,58].contains($0)}) else{return nil}
        return NativeSlashToken(range:range,query:value.substring(with:NSRange(location:beginning+1,length:caret-beginning-1)))
    }
}

/// Shares Code's provider-scoped catalog. Lives in the existing notch window,
/// so opening completion never changes focus or creates a second chat surface.
@MainActor final class NativeSlashPicker:NSVisualEffectView {
    var onAccept:((String)->Void)?
    private(set) var matches=[KaiSlashCommand](),selected=0
    private var rows=[NSButton](),signature="",suppressed:String?
    override init(frame:NSRect){
        super.init(frame:frame);material = .popover;state = .active;blendingMode = .withinWindow
        wantsLayer=true;layer?.cornerRadius=10;layer?.masksToBounds=true;isHidden=true
    }
    required init?(coder:NSCoder){fatalError()}
    func update(text:String,selection:NSRange,commands:[KaiSlashCommand],anchor:NSRect){
        guard let token=NativeSlashToken.at(text,selection:selection) else{hide();suppressed=nil;return}
        let signature="\(token.range.location):\(token.query)"
        guard suppressed != signature else{hide();return}
        let filter=token.query.lowercased()
        let values=commands.filter{filter.isEmpty || $0.name.lowercased().contains(filter)}.sorted{a,b in
            let ap=a.name.lowercased().hasPrefix(filter),bp=b.name.lowercased().hasPrefix(filter)
            return ap != bp ? ap:a.name<b.name
        }
        guard !values.isEmpty else{hide();return}
        if self.signature != signature || values != matches {
            self.signature=signature;matches=values;selected=0;render()
        }
        frame=CGRect(x:anchor.minX,y:anchor.maxY+6,width:anchor.width,height:CGFloat(min(matches.count,6))*25+8)
        let opening=isHidden;isHidden=false
        if opening,!NSWorkspace.shared.accessibilityDisplayShouldReduceMotion{
            layer?.add(CABasicAnimation(keyPath:"opacity").configured(from:0,to:1,duration:0.16),forKey:"slashReveal")
        }
    }
    var active:Bool{!isHidden && !matches.isEmpty}
    func move(_ delta:Int){guard active else{return};selected=(selected+delta+matches.count)%matches.count;render()}
    func accept(){guard active else{return};onAccept?(matches[selected].name);hide()}
    func dismiss(){suppressed=signature;hide()}
    func hide(){isHidden=true}
    private func render(){
        rows.forEach{$0.removeFromSuperview()};rows=[]
        let start=min(max(0,selected-5),max(0,matches.count-6))
        for index in start..<min(matches.count,start+6){
            let command=matches[index],button=NSButton(title:"/\(command.name)   \(command.description)",target:self,action:#selector(click(_:)))
            button.tag=index;button.isBordered=false;button.alignment = .left;button.font = .systemFont(ofSize:11);button.lineBreakMode = .byTruncatingTail
            button.toolTip=command.description;button.wantsLayer=true;button.layer?.cornerRadius=5
            button.layer?.backgroundColor=index == selected ? NSColor.controlAccentColor.withAlphaComponent(0.24).cgColor:nil
            addSubview(button);rows.append(button)
        }
        needsLayout=true
    }
    override func layout(){super.layout();for (i,row) in rows.enumerated(){row.frame=CGRect(x:5,y:bounds.height-4-CGFloat(i+1)*25,width:bounds.width-10,height:25)}}
    @objc private func click(_ sender:NSButton){selected=sender.tag;accept()}
}

private extension CABasicAnimation {
    func configured(from:Float,to:Float,duration:Double)->CABasicAnimation{fromValue=from;toValue=to;self.duration=duration;return self}
}
