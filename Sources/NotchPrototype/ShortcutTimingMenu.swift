import AppKit

@MainActor final class ShortcutTimingMenu:NSObject {
    let menu=NSMenu()
    private let preferences:ShortcutPreferences
    private var sliders=[DictationTrigger:NSSlider](),labels=[DictationTrigger:NSTextField]()
    var onChange:(()->Void)?
    init(defaults:UserDefaults = .standard){
        preferences=ShortcutPreferences(defaults:defaults);super.init()
        for trigger in DictationTrigger.allCases {
            let view=NSView(frame:CGRect(x:0,y:0,width:250,height:46))
            let label=NSTextField(labelWithString:"");label.frame=CGRect(x:12,y:25,width:226,height:16)
            let slider=NSSlider(value:preferences.holdDuration(for:trigger),minValue:0.1,maxValue:5,target:self,action:#selector(change(_:)))
            slider.identifier=NSUserInterfaceItemIdentifier(trigger.rawValue);slider.frame=CGRect(x:12,y:3,width:226,height:20);slider.isContinuous=true
            slider.setAccessibilityLabel("\(trigger.title) hold duration in seconds")
            view.addSubview(label);view.addSubview(slider);labels[trigger]=label;sliders[trigger]=slider
            let item=NSMenuItem();item.view=view;menu.addItem(item)
        }
        menu.addItem(.separator())
        let reset=menu.addItem(withTitle:"Restore original hold timings",action:#selector(reset),keyEquivalent:"");reset.target=self
        refresh()
    }
    func refresh(){
        for trigger in DictationTrigger.allCases {
            let value=preferences.holdDuration(for:trigger)
            sliders[trigger]?.doubleValue=value
            let name=trigger == .function ? "Fn":trigger.title
            labels[trigger]?.stringValue=String(format:"Hold %@ · %.2f s",name,value)
        }
    }
    @objc private func change(_ slider:NSSlider){
        guard let raw=slider.identifier?.rawValue,let trigger=DictationTrigger(rawValue:raw) else{return}
        preferences.setHoldDuration(slider.doubleValue,for:trigger);refresh();onChange?()
    }
    @objc private func reset(){preferences.resetHoldDurations();refresh();onChange?()}
}
