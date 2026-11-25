import AppKit
@main struct GalleryPageHarness{
 @MainActor static func main(){
  _=NSApplication.shared
  let gallery=PetGallery(),bounds=CGRect(x:0,y:0,width:470,height:320)
  let motion=PetGalleryMotion(row:0,columns:[0,1],durationsMs:[100,200],loop:true)
  let specs=(0..<1000).map{n in PetGallerySpec(id:"pet-\(n)",spritesheet:"\(n).png",rows:1,motions:["idle":motion],colors:[],category:n%2==0 ? "Originals":"Characters",displayName:String(format:"Pet %04d",n))}
  var pending=[(URL,(CGImage?)->Void)]()
  gallery.testImageLoader={url,completion in pending.append((url,completion))}
  func sprites()->[CALayer]{(gallery.layer.sublayers ?? []).filter{$0.name?.hasPrefix("gallery-pet:")==true}}
  func ids()->Set<String>{Set(sprites().compactMap(\.name))}
  let base=gallery.layer.sublayers?.count ?? 0
  gallery.configure(specs,at:URL(fileURLWithPath:"/tmp"))
  guard sprites().isEmpty else{print("GALLERY_PAGE_FAIL configure_allocated_sprites=\(sprites().count) entry_layers=\((gallery.layer.sublayers?.count ?? 0)-base)");exit(1)}
  gallery.layout(in:bounds,selected:"pet-0")
  precondition(sprites().count==12 && gallery.layer.sublayers?.count==base+48)
  let initial=sprites(),identities=initial.map(ObjectIdentifier.init)
  for n in 0..<50{gallery.layout(in:CGRect(x:0,y:0,width:CGFloat(470+n),height:320),selected:"pet-0")}
  precondition(sprites().map(ObjectIdentifier.init)==identities,"Resize recreated current entries")
  gallery.show(selected:"pet-0",motion:"idle");let stale=pending;pending=[]
  precondition(gallery.handleNavigation(at:CGPoint(x:450,y:12),selected:"pet-0"))
  precondition(sprites().count==12 && !ids().contains("gallery-pet:pet-0"))
  precondition(pending.count==12,"Navigation registered duplicate page-load callbacks")
  precondition(initial.allSatisfy{$0.superlayer==nil && $0.contents==nil && ($0.animationKeys()?.isEmpty ?? true)},"Retired layers retained visuals")
  let data=Data(repeating:255,count:16),provider=CGDataProvider(data:data as CFData)!
  let image=CGImage(width:2,height:2,bitsPerComponent:8,bitsPerPixel:32,bytesPerRow:8,space:CGColorSpaceCreateDeviceRGB(),bitmapInfo:CGBitmapInfo(rawValue:CGImageAlphaInfo.premultipliedLast.rawValue),provider:provider,decode:nil,shouldInterpolate:false,intent:.defaultIntent)!
  stale.forEach{$0.1(image)}
  precondition(sprites().allSatisfy{$0.contents==nil},"Old page completion populated new page")
  let current=pending;pending=[];current.forEach{$0.1(image)}
  precondition(sprites().allSatisfy{$0.contents != nil},"Current page load lost")
  gallery.layout(in:bounds,selected:"pet-999")
  precondition(ids().contains("gallery-pet:pet-999") && sprites().count<=12)
  precondition(!pending.isEmpty,"Visible selection jump did not schedule new image load")
  let replaced=pending;pending=[]
  gallery.configure(specs,at:URL(fileURLWithPath:"/different"));gallery.layout(in:bounds,selected:"pet-999");gallery.show(selected:"pet-999",motion:"idle")
  replaced.forEach{$0.1(image)}
  precondition(sprites().allSatisfy{$0.contents==nil},"Old catalog completion applied")
  // Category All->Originals; the selected odd pet is absent in Originals.
  let tabsCount=PetCategory.allCases.count+1
  let originalTab=PetCategory.allCases.firstIndex{$0.rawValue=="Originals"}!+1
  let categoryX=bounds.width/CGFloat(tabsCount)*(CGFloat(originalTab)+0.5)
  precondition(gallery.handleNavigation(at:CGPoint(x:categoryX,y:306),selected:"pet-999"))
  precondition(!ids().contains("gallery-pet:pet-999") && sprites().count<=12)
  precondition(gallery.pet(at:CGPoint(x:categoryX,y:306))==nil)
  let hidden=pending;pending=[];gallery.hide();hidden.forEach{$0.1(image)}
  precondition(sprites().allSatisfy{$0.contents==nil},"Hidden completion applied")
  precondition((gallery.layer.sublayers?.count ?? 0)<=base+48)
  print("GALLERY_COUNTS metadata=1000 configured_entry_layers=0 settled_entry_layers_max=48 repeated_layouts=50 nav_callbacks=12")
  print("GALLERY_PAGE_PASS 1000_specs/48_entry_layers/resize_identity/pagination/selection/category/stale_async/hide")
 }
}
