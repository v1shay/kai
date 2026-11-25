import AppKit
private func field(_ object:Any,_ key:String)->Any?{Mirror(reflecting:object).children.first{$0.label==key}?.value}
@main struct AgentPickerAudit {
 @MainActor static func main()throws{
  _=NSApplication.shared
  let picker=NativeAgentPicker();var actions=[String]();var providerRequest=""
  picker.onAgent={id,request in providerRequest=request;actions.append("agent:"+id)};picker.onModel={actions.append("model:"+$0)};picker.onEffort={actions.append("effort:"+$0+":"+$1)}
  func packet(agent:String="codex",horizontal:Bool=true,owner:String="owner",thread:String="code:picker",failed:Bool=false,ownFailure:Bool=false,dense:Bool=false,unavailableGemini:Bool=false,ninja:Bool=false)->KaiState{
   var models=["gpt-6-luna","gpt-6.1-sol","gpt-6-astra"].map{["model":$0,"displayName":$0,"supportedReasoningEfforts":[["reasoningEffort":"low"],["reasoningEffort":"high"]]] as [String:Any]}
   if dense{for i in models.indices{models[i]["supportedReasoningEfforts"]=["none","minimal","low","medium","high","xhigh","max","ultra"].map{["reasoningEffort":$0]}};models += (0..<64).map{["model":"model-\($0)","displayName":"Model \($0)","supportedReasoningEfforts":[]] as [String:Any]}}
   var body:[String:Any]=["project":"/private/tmp","projects":[],"chats":[],"standaloneChats":[],"standalone":true,"threadId":thread,"threadName":"Picker audit","history":[],"attachments":[],"readOnly":false,"approval":false,"activity":["scene":"off","text":"Ready"],"activeTasks":[],"notice":"Ready","selectedAgent":agent,"effectiveModel":"gpt-6-luna","selectedModel":"gpt-6-luna","effectiveReasoningEffort":"low","models":models,"ownerToken":owner,"horizontalAgentPicker":horizontal,"agents":[["id":"codex","name":"Codex","available":true],["id":"gemini","name":"Gemini","available":true]]]
   if dense{body["agents"]=["codex","claude","gemini","opencode","cursor","grok","antigravity"].map{["id":$0,"name":$0,"available":true] as [String:Any]}}
   if unavailableGemini{body["agents"]=[["id":"codex","name":"Codex","available":true],["id":"gemini","name":"Gemini","available":false]]}
   if ninja{body["ninja"]=["selection":["machine":"local","endpoint":"http://127.0.0.1:11434","engine":"ollama","harness":"codex","model":"gpt-6-luna"],"machines":[["id":"local","name":"Local","online":true,"local":true]],"loading":false]}
   if failed{body["completedRequestId"]="unrelated-provider";body["completedRequestError"]="Rejected"}
   if ownFailure{body["completedRequests"]=[["id":providerRequest,"error":"Provider unavailable"]]}
   return try! JSONDecoder().decode(KaiState.self,from:JSONSerialization.data(withJSONObject:body))
  }
  let anchor=CGPoint(x:400,y:600)
  picker.show(scope:.providers,state:packet(),at:anchor)
  precondition(picker.visible && picker.column==0,"Provider scope did not open")
  precondition((field(picker,"columns") as! [NSView]).count==1,"/agent exposed models or intensity")
  picker.handleKey(125);picker.handleKey(36)
  precondition(actions==["agent:gemini"] && !picker.visible,"Provider arrows/Enter did not select Gemini and close")
  picker.show(scope:.models,state:packet(),at:anchor)
  let cols=field(picker,"columns") as! [NSView]
  precondition(cols.count==2 && cols[0].frame.maxX<cols[1].frame.minX,"/model is not model-to-intensity from left to right")
  let rows=field(picker,"effortRows") as! [EffortTileRow];precondition(rows.allSatisfy{$0.bounds.width==$0.frame.width},"Thinking text was scaled instead of laid out at native resolution")
  picker.handleKey(125);picker.handleKey(124)
  precondition(actions.last=="model:gpt-6.1-sol" && picker.column==2,"Right did not select the model and focus intensity")
  picker.handleKey(125);picker.handleKey(36)
  precondition(actions.last=="effort:gpt-6.1-sol:high" && !picker.visible,"Intensity arrows/Enter used the wrong model")
  picker.show(scope:.all,state:packet(horizontal:false),at:anchor)
  let stack=field(picker,"columns") as! [NSView]
  precondition(stack.count==3 && stack[0].frame.minY>stack[1].frame.minY && stack[1].frame.minY>stack[2].frame.minY,"Vertical preference was ignored")
  picker.handleKey(123);picker.handleKey(125);picker.handleKey(124)
  let count=actions.count
  picker.handleKey(36);precondition(actions.count==count,"Previous provider models were selectable before the new catalog arrived")
  picker.update(packet(agent:"gemini",horizontal:false,owner:"gemini-owner"));picker.handleKey(124)
  precondition(picker.column==2,"Expected provider catalog did not resume keyboard selection")
  picker.update(packet(agent:"gemini",horizontal:false,owner:"different-owner",thread:"other-chat"))
  precondition(!picker.visible,"Picker retained commands belonging to another chat")
  picker.show(scope:.models,state:packet(),at:anchor);picker.show(scope:.models,state:packet(),at:anchor)
  precondition(!picker.visible,"Second click did not toggle the picker closed")
  picker.show(scope:.models,state:packet(),at:anchor);picker.handleKey(53);precondition(!picker.visible,"Escape did not close picker")
  picker.show(scope:.all,state:packet(),at:anchor);picker.handleKey(123);picker.handleKey(125);picker.handleKey(124);picker.update(packet(failed:true));precondition(picker.visible,"An unrelated failed command closed the pending provider picker");picker.update(packet(failed:true,ownFailure:true));precondition(!picker.visible,"Rejected provider selection left models permanently disabled")
  picker.show(scope:.all,state:packet(horizontal:false,dense:true),at:anchor)
  let denseColumns=field(picker,"columns") as! [NSView];precondition(denseColumns.allSatisfy{$0.frame.minY>=0},"Long provider/model/intensity lists clip the vertical picker")
  picker.dismiss()
  picker.show(scope:.providers,state:packet(),at:anchor)
  picker.handleKey(125)
  precondition(picker.cursors[0]==1,"Provider cursor did not move")
  picker.update(packet(unavailableGemini:true))
  precondition((field(picker,"buttons") as! [[NSButton]])[0].count==1,"Provider availability changed without rebuilding the visible rows")
  precondition(picker.cursors[0]==0,"Provider cursor escaped the refreshed catalog")
  picker.handleKey(36)
  precondition(!picker.visible,"Refreshed provider menu could not select its remaining provider")
  picker.show(scope:.models,state:packet(agent:"ninja",ninja:true),at:anchor)
  let search=field(picker,"ninjaSearch") as! NSSearchField
  let searchWindow=search.window!
  searchWindow.makeKeyAndOrderFront(nil)
  searchWindow.makeFirstResponder(search)
  let editor=search.currentEditor() as! NSTextView
  editor.string="model query"
  editor.setSelectedRange(NSRange(location:5,length:0))
  precondition(!picker.handleKey(123) && !picker.handleKey(124),"Search cursor arrows were consumed as model navigation")
  editor.setMarkedText("漢",selectedRange:NSRange(location:1,length:0),replacementRange:NSRange(location:NSNotFound,length:0))
  precondition(editor.hasMarkedText(),"Search fixture did not create composition")
  precondition(!picker.handleKey(36) && !picker.handleKey(53),"IME commit/cancel was consumed by the model picker")
  let beforeComposition=(field(picker,"columns") as! [NSView]).first!
  picker.update(packet(agent:"ninja",ninja:true))
  precondition((field(picker,"columns") as! [NSView]).first === beforeComposition,"A backend update replaced the active IME editor")
  editor.unmarkText()
  picker.dismiss(restoreFocus:true)
  print("Native picker regressions: provider availability refresh, bounded cursor, search caret arrows, and IME-safe commit/cancel passed")
  print("Native agent picker: provider-only and model/intensity scopes, horizontal/vertical layout, arrows, pending provider isolation, owner invalidation, toggle and Escape passed")
 }
}
