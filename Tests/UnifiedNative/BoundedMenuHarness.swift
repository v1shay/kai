import AppKit
@MainActor private final class ChoiceRow:NSView{var onSelect:(()->Void)?}
@main struct BoundedMenuHarness{
 @MainActor static func main(){
  setbuf(stdout,nil)
  _=NSApplication.shared;NSApp.setActivationPolicy(.accessory);NSApp.finishLaunching()
  var rows=[String:ChoiceRow](),made=0,peak=0,selected="pet-0",clicked=[String]()
  func owner(_ count:Int)->(BoundedNativeChoices,NSMenu){
   let choices=(0..<count).map{NativeMenuChoice(id:"pet-\($0)",title:"Pet \($0)")},menu=NSMenu()
   let owner=BoundedNativeChoices(groups:[NativeMenuChoiceGroup(title:"Originals (\(count))",choices:choices)],makeItem:{choice in
    let item=NSMenuItem(title:choice.title,action:nil,keyEquivalent:"");let row=ChoiceRow()
    row.onSelect={clicked.append(choice.id)};item.view=row;item.state=choice.id==selected ? .on:.off
    rows[choice.id]=row;made+=1;peak=max(peak,rows.count);return item
   },didRetire:{rows.values.forEach{$0.onSelect=nil};rows.removeAll()})
   owner.install(in:menu,at:0);return(owner,menu)
  }
  func prepare(_ menu:NSMenu){menu.delegate?.menuNeedsUpdate?(menu)}
  func open(_ menu:NSMenu){prepare(menu);menu.delegate?.menuWillOpen?(menu)}
  func close(_ menu:NSMenu){menu.delegate?.menuDidClose?(menu)}
  func flush(){RunLoop.main.run(until:Date().addingTimeInterval(0.02))}
  let (small,smallRoot)=owner(30);let direct=smallRoot.items[0].submenu!
  precondition(made==0 && direct.items.isEmpty)
  open(direct);precondition(direct.items.count==30 && rows.count==30)
  precondition(direct.items.map(\.title)==(0..<30).map{"Pet \($0)"},"Small category layout changed")
  let originalIDs=direct.items.map(ObjectIdentifier.init);prepare(direct)
  precondition(direct.items.map(ObjectIdentifier.init)==originalIDs && made==30)
  rows["pet-0"]?.onSelect?();precondition(clicked==["pet-0"])
  weak var released=rows["pet-0"]
  close(direct);precondition(rows.count==30,"Released event receiver synchronously")
  flush();print("CLOSE_STATE rows=\(rows.count) items=\(direct.items.count) released=\(released == nil)");precondition(rows.isEmpty && direct.items.isEmpty && released==nil)
  selected="pet-7";open(direct);precondition(direct.items[7].state == .on)
  small.retire();precondition(rows.isEmpty)
  let (large,largeRoot)=owner(1003);let category=largeRoot.items[0].submenu!
  let before=made;open(category)
  precondition(category.items.count==26 && made==before && rows.isEmpty)
  precondition(category.items[0].title=="Pets 1–40" && category.items[25].title=="Pets 1001–1003")
  let first=category.items[0].submenu!,second=category.items[1].submenu!,last=category.items[25].submenu!
  open(first);precondition(rows.count==40 && first.items.count==40)
  let held=rows["pet-0"]!;weak var retired=rows["pet-1"]
  close(first);open(second)
  precondition(first.items.isEmpty && rows.count==40 && retired==nil && held.onSelect==nil,"Sibling did not retire callbacks/rows")
  flush();precondition(rows.count==40,"Old close callback retired the new page")
  close(second);open(last);precondition(rows.count==3 && last.items.count==3)
  close(last);flush();precondition(rows.isEmpty && last.items.isEmpty)
  open(first);large.invalidate();precondition(rows.isEmpty && first.items.isEmpty)
  let created=made;prepare(first);precondition(rows.isEmpty && made==created,"Invalidated delegate recreated obsolete rows")
  precondition(peak<=40,"Live custom rows exceeded page bound")
  withExtendedLifetime((small,large,smallRoot,largeRoot)){}
  print("BOUNDED_MENU_PASS normal30/direct_layout/latest_selection/1003_metadata/26_page_shells/peak_rows=\(peak)/reopen/reentrant_close/sibling/callback_retirement")
 }
}
