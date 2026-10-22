import Foundation

/// Match the chat shown by the compact indicator. External dot activities have
/// no Codex conversation to open and must not be sent to thread/read.
enum RunningChatSelection {
    static func target(in state:KaiState)->(id:String,name:String)? {
        let tasks=state.activeTasks.filter{!$0.id.isEmpty && !$0.id.hasPrefix("dot:")}
        if let focus=state.focusThreadId,let task=tasks.first(where:{$0.id == focus}) {return (task.id,task.name)}
        if let id=state.threadId,state.activeTurnId != nil || tasks.contains(where:{$0.id == id}) {return (id,state.threadName)}
        if let task=tasks.first {return (task.id,task.name)}
        return nil
    }
}
