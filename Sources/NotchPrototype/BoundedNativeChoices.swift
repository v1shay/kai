import AppKit

struct NativeMenuChoice {let id,title:String}
struct NativeMenuChoiceGroup {let title:String,choices:[NativeMenuChoice]}

/// Metadata remains complete; only one currently tracked category/page owns
/// custom row views. Small categories retain their existing direct row layout.
/// Every submenu shares this one delegate; no per-page delegate accumulates.
@MainActor final class BoundedNativeChoices:NSObject,NSMenuDelegate {
    private let groups:[NativeMenuChoiceGroup],pageSize:Int
    private let makeItem:(NativeMenuChoice)->NSMenuItem
    private let didRetire:()->Void
    private var choicesByMenu=[ObjectIdentifier:[NativeMenuChoice]]()
    private weak var activeMenu:NSMenu?
    private var activeTracking=false,revision=0
    init(groups:[NativeMenuChoiceGroup],pageSize:Int=40,makeItem:@escaping(NativeMenuChoice)->NSMenuItem,didRetire:@escaping()->Void){
        self.groups=groups;self.pageSize=max(1,pageSize);self.makeItem=makeItem;self.didRetire=didRetire
        super.init()
    }
    func install(in menu:NSMenu,at position:Int){
        for (offset,group) in groups.enumerated(){
            let item=NSMenuItem(title:group.title,action:nil,keyEquivalent:"")
            item.identifier=NSUserInterfaceItemIdentifier("pet-category")
            item.submenu=submenu(group.choices)
            menu.insertItem(item,at:position+offset)
        }
    }
    private func submenu(_ choices:[NativeMenuChoice])->NSMenu{
        let menu=NSMenu();choicesByMenu[ObjectIdentifier(menu)]=choices;menu.delegate=self;return menu
    }
    func menuNeedsUpdate(_ menu:NSMenu){
        guard let choices=choicesByMenu[ObjectIdentifier(menu)] else{return}
        if choices.count>pageSize{
            if !activeTracking{retire()}
            guard menu.items.isEmpty else{return}
            for start in stride(from:0,to:choices.count,by:pageSize){
                let end=min(choices.count,start+pageSize)
                let item=NSMenuItem(title:"Pets \(start+1)–\(end)",action:nil,keyEquivalent:"")
                item.submenu=submenu(Array(choices[start..<end]));menu.addItem(item)
            }
            return
        }
        if activeMenu === menu{return}
        retire()
        activeMenu=menu;revision+=1
        autoreleasepool{for choice in choices{menu.addItem(makeItem(choice))}}
    }
    func menuWillOpen(_ menu:NSMenu){if activeMenu === menu{activeTracking=true}}
    func menuDidClose(_ menu:NSMenu){
        guard activeMenu === menu else{return}
        activeTracking=false
        let revision=self.revision
        // Keep the selected row alive through AppKit's current event dispatch;
        // a sibling page opening retires it synchronously before allocating rows.
        DispatchQueue.main.async{[weak self,weak menu] in
            guard let self,let menu,!self.activeTracking,self.revision==revision,self.activeMenu === menu else{return}
            self.retire()
        }
    }
    func retire(){
        revision+=1
        autoreleasepool{
            didRetire()
            // AppKit may cache removed NSMenuItems. Explicitly release their
            // custom views and callback-bearing objects before removing them.
            for item in activeMenu?.items ?? []{item.view=nil;item.target=nil;item.representedObject=nil}
            activeMenu?.removeAllItems();activeMenu=nil;activeTracking=false
        }
    }
    func invalidate(){retire();choicesByMenu.removeAll()}
}
