import AppKit
private func noop(_ bytes:UnsafePointer<UInt8>?,_ count:Int)->Bool{true}
private func unwrap(_ value:Any)->Any{let m=Mirror(reflecting:value);if m.displayStyle == .optional,let c=m.children.first{return unwrap(c.value)};return value}
private func member(_ object:Any,_ name:String)->Any?{Mirror(reflecting:object).children.first{$0.label==name}.map{unwrap($0.value)}}
@MainActor private final class TrackingDelegate:NSObject,NSMenuDelegate{
 let forward:NSMenuDelegate
 init(_ value:NSMenuDelegate){forward=value}
 func menuNeedsUpdate(_ menu:NSMenu){forward.menuNeedsUpdate?(menu);menu.cancelTracking()}
}
@main struct DeferredMenusHarness{
 @MainActor static func main(){
  setbuf(stdout,nil)
  DispatchQueue.global().asyncAfter(deadline:.now()+15){fputs("DEFERRED_MENUS_TIMEOUT\n",stderr);exit(2)}
  _=NSApplication.shared;NSApp.setActivationPolicy(.accessory);NSApp.finishLaunching()
  let defaults=UserDefaults.standard
  defaults.set(true,forKey:"unifiedPreferencesSeeded");defaults.set(false,forKey:"soundsEnabled")
  let window=NSWindow(contentRect:CGRect(x:100,y:100,width:900,height:600),styleMask:[.titled],backing:.buffered,defer:false)
  window.isReleasedWhenClosed=false
  let runtime=EmbeddedCodeRuntime.shared;runtime.start(window:window,command:noop,presentInitialNotch:false)
  guard let app=member(runtime,"app") as? KaiPetApp,let root=member(app,"statusContextMenu") as? NSMenu,let pets=member(app,"workspacePetMenu") as? NSMenu else{fatalError("Owner missing")}
  defer{app.applicationWillTerminate(Notification(name:NSApplication.willTerminateNotification));NSApp.windows.forEach{$0.orderOut(nil)}}
  func find(_ title:String,in menu:NSMenu)->NSMenu?{for item in menu.items{if item.title==title{return item.submenu};if let child=item.submenu,let value=find(title,in:child){return value}};return nil}
  guard let glass=find("Bottom appearance",in:root),let sizing=find("Default notch size",in:root) else{fatalError("Menus missing")}
  precondition(glass.items.isEmpty && sizing.items.isEmpty)
  precondition((member(app,"petMenuRows") as? [String:Any])?.isEmpty==true)
  func track(_ menu:NSMenu,_ name:String){
   guard let original=menu.delegate else{fatalError("Deferred delegate missing")}
   let closer=TrackingDelegate(original);menu.delegate=closer
   let begin=CACurrentMediaTime();menu.popUp(positioning:nil,at:NSPoint(x:120,y:120),in:nil)
   print("REAL_MENU_BUILD \(name) elapsed_ms=\((CACurrentMediaTime()-begin)*1000) items=\(menu.items.count)")
   menu.delegate=original;withExtendedLifetime(closer){}
  }
  defaults.set(0.37,forKey:"glass.height")
  track(glass,"glass-cold")
  guard let sliders=member(app,"glassSliders") as? [String:NSSlider] else{fatalError("No sliders")}
  precondition(abs((sliders["height"]?.doubleValue ?? -1)-0.37)<0.0001,"Stale startup prefs")
  precondition(sliders.values.allSatisfy{$0.target === app && $0.action != nil})
  let identities=glass.items.map(ObjectIdentifier.init);track(glass,"glass-repeat");precondition(glass.items.map(ObjectIdentifier.init)==identities)
  defaults.set(611.0,forKey:"notchDefaultWidth");defaults.set(400.0,forKey:"notchDefaultHeight")
  track(sizing,"sizing-cold")
  precondition(abs(((member(app,"defaultWidthSlider") as? NSSlider)?.doubleValue ?? -1)-611)<0.001)
  precondition((member(app,"defaultHeightSlider") as? NSSlider)?.doubleValue==400)
  guard let bundled=member(app,"catalog") as? Catalog else{fatalError("No catalog")}
  track(pets,"bundled-pet-shell")
  let categoryItems=pets.items.filter{$0.identifier?.rawValue=="pet-category"}
  let bundledGroups=PetCategory.allCases.compactMap{category -> (String,[Pet])? in
    let values=bundled.pets.filter{PetCategory.of($0)==category}.sorted{$0.displayName.localizedStandardCompare($1.displayName) == .orderedAscending}
    return values.isEmpty ? nil:("\(category.rawValue) (\(values.count))",values)
  }
  precondition(categoryItems.map(\.title)==bundledGroups.map{$0.0})
  for (item,group) in zip(categoryItems,bundledGroups){
    precondition(group.1.count<=40,"Update fixture to cover normal nonpaged categories")
    track(item.submenu!,"bundled-direct-category")
    precondition(item.submenu!.items.map(\.title)==group.1.map(\.displayName),"Normal category rows/order changed")
  }
  let first=Pet(id:"deferred-before",displayName:"Before",spritesheet:"none.png",grid:Grid(rows:1),animationOverrides:nil)
  app.testDeferredPetCatalog([first]);precondition((member(app,"petMenuRows") as? [String:Any])?.isEmpty==true)
  track(pets,"pets-shell-cold")
  precondition((member(app,"petMenuRows") as? [String:Any])?.isEmpty==true)
  guard let firstCategory=pets.items.first(where:{$0.identifier?.rawValue=="pet-category"})?.submenu else{fatalError("Category missing")}
  track(firstCategory,"pets-category-cold")
  precondition((member(app,"petMenuRows") as? [String:Any])?[first.id] != nil)
  let next=Pet(id:"deferred-after",displayName:"After",spritesheet:"none.png",grid:Grid(rows:1),animationOverrides:nil)
  app.testDeferredPetCatalog([next])
  precondition((member(app,"petMenuRows") as? [String:Any])?.isEmpty==true)
  guard let nextCategory=pets.items.first(where:{$0.identifier?.rawValue=="pet-category"})?.submenu else{fatalError("Updated category missing")}
  track(nextCategory,"pets-updated-category")
  let rows=member(app,"petMenuRows") as? [String:Any]
  precondition(rows?[next.id] != nil && rows?[first.id]==nil)
  precondition(pets.items.last?.title=="done" && pets.items.dropLast().last?.isSeparatorItem==true,"Trailing actions changed")
  let petIDs=pets.items.map(ObjectIdentifier.init);track(pets,"pets-repeat");precondition(pets.items.map(ObjectIdentifier.init)==petIDs)
  let many=(0..<1000).map{n in Pet(id:"large-fixture-\(n)",displayName:String(format:"Fixture %04d",n),spritesheet:"none.png",grid:Grid(rows:1),animationOverrides:nil,category:"Originals")}
  let installBegin=CACurrentMediaTime();app.testDeferredPetCatalog(many)
  print("LARGE_MENU_METADATA install_ms=\((CACurrentMediaTime()-installBegin)*1000)")
  guard let bigCategory=pets.items.first(where:{$0.identifier?.rawValue=="pet-category"})?.submenu else{fatalError("Large category missing")}
  track(bigCategory,"large-category-page-shells")
  precondition(bigCategory.items.count==25 && (member(app,"petMenuRows") as? [String:Any])?.isEmpty==true)
  guard let page=bigCategory.items.first?.submenu,let sibling=bigCategory.items.dropFirst().first?.submenu else{fatalError("Pages missing")}
  track(page,"large-first40")
  precondition((member(app,"petMenuRows") as? [String:Any])?.count==40)
  guard let old=(member(app,"petMenuRows") as? [String:Any])?["large-fixture-0"] as? NSView else{fatalError("Row missing")}
  let oldCallback=member(old,"onSelect") as? ()->Void
  guard let currentRow=(member(app,"petMenuRows") as? [String:Any])?["large-fixture-7"],let selectCurrent=member(currentRow,"onSelect") as? ()->Void else{fatalError("Current row callback missing")}
  selectCurrent()
  precondition((member(app,"petID") as? String)=="large-fixture-7" && (member(currentRow,"selected") as? Bool)==true,"Current pet callback/checkmark failed")
  let pageIdentities=page.items.map(ObjectIdentifier.init);track(page,"large-repeat40");precondition(page.items.map(ObjectIdentifier.init)==pageIdentities)
  track(sibling,"large-sibling40")
  precondition((member(app,"petMenuRows") as? [String:Any])?.count==40 && page.items.isEmpty)
  precondition((member(old,"onSelect") as? ()->Void)==nil && old.superview==nil,"Retired view retained callback")
  for other in bigCategory.items.dropFirst(2){track(other.submenu!,"large-traverse40");precondition((member(app,"petMenuRows") as? [String:Any])?.count==40)}
  let obsoleteDelegate=page.delegate
  let selectedBefore=member(app,"petID") as? String
  app.testDeferredPetCatalog([next]);oldCallback?();obsoleteDelegate?.menuNeedsUpdate?(page)
  precondition((member(app,"petMenuRows") as? [String:Any])?.isEmpty==true && (member(app,"petID") as? String)==selectedBefore,"Obsolete catalog callback changed selection")
  print("REAL_BOUNDED_PET_MENU_PASS 1000_catalog/25_page_shells/max40_rows/all_pages/retired_callbacks/stale_catalog_generation")
  print("REAL_DEFERRED_MENUS_PASS latest_prefs/rescan_before_after/callbacks/reopen/trailing_actions")
 }
}
