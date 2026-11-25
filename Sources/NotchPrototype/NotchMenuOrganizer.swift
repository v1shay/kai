import AppKit

@MainActor enum NotchMenuOrganizer {
    struct Section {let title:String;let items:[NSMenuItem]}
    /// Move the original items, retaining views, actions, targets, and live state.
    /// Unclassified controls remain available even when new features are added.
    static func organize(_ menu:NSMenu,quick:[NSMenuItem],sections:[Section],footer:[NSMenuItem]){
        let original=menu.items.filter{!$0.isSeparatorItem}
        let known=Set((quick+sections.flatMap(\.items)+footer).map{ObjectIdentifier($0)})
        let remaining=original.filter{!known.contains(ObjectIdentifier($0))}
        menu.removeAllItems()
        var placed=Set<ObjectIdentifier>()
        func append(_ items:[NSMenuItem],to destination:NSMenu){
            for item in items where placed.insert(ObjectIdentifier(item)).inserted{destination.addItem(item)}
        }
        append(quick,to:menu);menu.addItem(.separator())
        for section in sections where !section.items.isEmpty {
            let root=menu.addItem(withTitle:section.title,action:nil,keyEquivalent:"")
            let submenu=NSMenu();root.submenu=submenu;append(section.items,to:submenu)
        }
        if !remaining.isEmpty {
            let root=menu.addItem(withTitle:"More controls",action:nil,keyEquivalent:"")
            root.submenu=NSMenu();append(remaining,to:root.submenu!)
        }
        menu.addItem(.separator());append(footer,to:menu)
    }
}
