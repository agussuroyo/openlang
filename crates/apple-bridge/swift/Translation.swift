import Foundation
import Translation

final class TranslatorSession: @unchecked Sendable {
    let session: TranslationSession
    init(_ session: TranslationSession) { self.session = session }
}

@_cdecl("ol_tr_languages")
public func olTrLanguages() -> UnsafeMutablePointer<CChar>? {
    let tags = try? blocking {
        Set(await LanguageAvailability().supportedLanguages.map { $0.minimalIdentifier }).sorted()
    }.get()
    return strdup((tags ?? []).joined(separator: "\n"))
}

/// 0 ready, 1 the system can download it, 2 unsupported.
@_cdecl("ol_tr_status")
public func olTrStatus(_ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>) -> Int32 {
    let source = Locale.Language(identifier: string(source))
    let target = Locale.Language(identifier: string(target))
    let status = try? blocking { await LanguageAvailability().status(from: source, to: target) }.get()
    return code(status)
}

/// 0 installed, 1 the user can download it, 2 unavailable.
func code(_ status: LanguageAvailability.Status?) -> Int32 {
    switch status {
    case .installed: return 0
    case .supported: return 1
    default: return 2
    }
}

/// Status of the low-latency language pack; `nil` before macOS 26.4, which has none.
func fastStatus(_ source: Locale.Language, _ target: Locale.Language)
    -> LanguageAvailability.Status?
{
    guard #available(macOS 26.4, *) else { return nil }
    return try? blocking {
        await LanguageAvailability(preferredStrategy: .lowLatency).status(from: source, to: target)
    }.get()
}

/// Low latency answers in tens of milliseconds instead of about half a second, so it is used
/// whenever its language pack is installed.
@_cdecl("ol_tr_open")
public func olTrOpen(_ source: UnsafePointer<CChar>, _ target: UnsafePointer<CChar>)
    -> UnsafeMutableRawPointer
{
    let source = Locale.Language(identifier: string(source))
    let target = Locale.Language(identifier: string(target))
    let session =
        fastSession(source, target) ?? TranslationSession(installedSource: source, target: target)
    return Unmanaged.passRetained(TranslatorSession(session)).toOpaque()
}

func fastSession(_ source: Locale.Language, _ target: Locale.Language) -> TranslationSession? {
    guard #available(macOS 26.4, *), fastStatus(source, target) == .installed else { return nil }
    return TranslationSession(installedSource: source, target: target, preferredStrategy: .lowLatency)
}

@_cdecl("ol_tr_translate")
public func olTrTranslate(
    _ session: UnsafeMutableRawPointer, _ source: UnsafePointer<CChar>,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> UnsafeMutablePointer<CChar>? {
    let translator = Unmanaged<TranslatorSession>.fromOpaque(session).takeUnretainedValue()
    let source = string(source)
    switch blocking({ try await translator.session.translate(source).targetText }) {
    case .success(let translated): return strdup(translated)
    case .failure(let failure):
        error.pointee = text(failure)
        return nil
    }
}

@_cdecl("ol_tr_close")
public func olTrClose(_ session: UnsafeMutableRawPointer) {
    Unmanaged<TranslatorSession>.fromOpaque(session).release()
}
