import AppKit
import SwiftUI
import Translation

/// Like `ol_tr_status`, for the low-latency pack; unavailable before macOS 26.4.
@_cdecl("ol_tr_fast_status")
public func olTrFastStatus(_ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>)
    -> Int32
{
    code(
        fastStatus(
            Locale.Language(identifier: string(source)),
            Locale.Language(identifier: string(target))))
}

/// Asks the user, in the system's own download sheet, for the languages of this pair, the fast
/// kind when this macOS has it. Returns once they were downloaded or the user said no; only
/// SwiftUI can show that sheet. A process without a running AppKit app, such as a test, cannot
/// show it and returns at once.
@_cdecl("ol_tr_install")
public func olTrInstall(_ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>)
    -> UnsafeMutablePointer<CChar>?
{
    let running = DispatchQueue.main.sync { NSApp?.isRunning ?? false }
    guard running else { return nil }
    let source = Locale.Language(identifier: string(source))
    let target = Locale.Language(identifier: string(target))
    let done = DispatchSemaphore(value: 0)
    let failure = Box<Error?>(nil)
    let window = Box<NSWindow?>(nil)
    // Runs on the main thread, once: the sheet's answer or the time limit, whichever is first.
    let end = { (error: Error?) in
        guard let shown = window.value else { return }
        window.value = nil
        failure.value = error
        shown.close()
        done.signal()
    }
    DispatchQueue.main.async {
        let shown = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 420, height: 120),
            styleMask: [.titled], backing: .buffered, defer: false)
        shown.title = "OpenLang"
        shown.isReleasedWhenClosed = false
        shown.contentView = NSHostingView(
            rootView: Download(source: source, target: target) { error in
                DispatchQueue.main.async { end(error) }
            })
        shown.center()
        shown.level = .floating
        window.value = shown
        NSApplication.shared.activate()
        shown.makeKeyAndOrderFront(nil)
    }
    // Nobody may be there to answer. A download already started goes on in the background.
    if done.wait(timeout: .now() + 120) == .timedOut {
        DispatchQueue.main.sync { end(nil) }
    }
    guard let error = failure.value else { return nil }
    return text(error)
}

private struct Download: View {
    let source: Locale.Language
    let target: Locale.Language
    let finished: (Error?) -> Void

    var body: some View {
        Text("Downloading languages for live translation…")
            .padding()
            .frame(width: 420, height: 120)
            .translationTask(configuration) { session in
                do {
                    try await session.prepareTranslation()
                    finished(nil)
                } catch {
                    finished(error)
                }
            }
    }

    private var configuration: TranslationSession.Configuration {
        if #available(macOS 26.4, *) {
            return .init(source: source, target: target, preferredStrategy: .lowLatency)
        }
        return .init(source: source, target: target)
    }
}
