import SwiftUI

@main
struct ShardMeldApp: App {
    @StateObject private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environmentObject(model)
                .preferredColorScheme(.dark)
                .frame(minWidth: 1_080, minHeight: 700)
        }
        .windowStyle(.hiddenTitleBar)
        .windowToolbarStyle(.unifiedCompact)
        .commands {
            CommandGroup(replacing: .newItem) { }
            CommandMenu("ShardMeld") {
                Button("开始复用分析") {
                    model.selection = .analyze
                }
                .keyboardShortcut("a", modifiers: [.command])

                Button("新建接收任务") {
                    model.selection = .receive
                }
                .keyboardShortcut("n", modifiers: [.command])
            }
        }
    }
}
