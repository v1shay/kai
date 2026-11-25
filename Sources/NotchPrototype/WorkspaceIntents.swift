import AppKit
import AppIntents

@available(macOS 13.0,*)
struct RunKaiWorkflowIntent:AppIntent {
    static var title:LocalizedStringResource="Run Kai Workflow"
    static var description=IntentDescription("Run a named workflow saved in Kai Unified using its saved project, provider and model.")
    static var openAppWhenRun=true
    @Parameter(title:"Workflow name") var name:String
    @MainActor func perform()async throws->some IntentResult{
        guard !name.isEmpty,name.utf8.count<=256,var components=URLComponents(string:"kai://workflow/") else{throw NSError(domain:"KaiWorkflow",code:1,userInfo:[NSLocalizedDescriptionKey:"Choose a saved workflow name"])}
        components.path="/"+name
        guard let url=components.url else{throw NSError(domain:"KaiWorkflow",code:2)}
        NSWorkspace.shared.open(url);return .result()
    }
}
@available(macOS 13.0,*)
struct KaiWorkflowShortcuts:AppShortcutsProvider {
    static var appShortcuts:[AppShortcut]{AppShortcut(intent:RunKaiWorkflowIntent(),phrases:["Run a workflow in \(.applicationName)"],shortTitle:"Run workflow",systemImageName:"play.circle")}
}
