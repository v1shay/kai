import AppKit

/// Retained by the native owner; NSMenu retains its delegate weakly.
/// The shell remains immediately available to every entry point. Custom controls
/// are built once, on the main thread, immediately before AppKit tracks the menu.
@MainActor final class DeferredNativeMenu:NSObject,NSMenuDelegate {
    private var build:((NSMenu)->Void)?
    init(menu:NSMenu,build:@escaping(NSMenu)->Void){
        self.build=build;super.init();menu.delegate=self
    }
    func menuNeedsUpdate(_ menu:NSMenu){
        guard let build else{return}
        // Consume first: a builder may synchronously re-enter menu updating.
        self.build=nil
        build(menu)
    }
}
