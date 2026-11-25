import AVFoundation
import CoreMedia
import Speech

/// (context, text, start, end, isFinal, error)
public typealias SpeechCallback = @convention(c) (
    UnsafeMutableRawPointer?, UnsafePointer<CChar>?, Double, Double, Bool, UnsafePointer<CChar>?
) -> Void
public typealias ProgressCallback = @convention(c) (UnsafeMutableRawPointer?, Double) -> Void

struct Update: Sendable {
    let text: String
    let range: CMTimeRange
    let isFinal: Bool
}

/// One recognizer behind a common shape, whichever Apple model serves the locale.
struct Recognizer {
    let module: any SpeechModule
    let listen: @Sendable (@escaping @Sendable (Update) -> Void) async throws -> Void

    /// SpeechTranscriber is the better model; DictationTranscriber covers more languages.
    init(_ tag: String) async throws {
        let wanted = Locale(identifier: tag)
        if SpeechTranscriber.isAvailable,
            let locale = await SpeechTranscriber.supportedLocale(equivalentTo: wanted)
        {
            let t = SpeechTranscriber(
                locale: locale, transcriptionOptions: [],
                reportingOptions: [.volatileResults, .fastResults],
                attributeOptions: [.audioTimeRange])
            module = t
            listen = { send in
                for try await r in t.results {
                    send(Update(text: String(r.text.characters), range: r.range, isFinal: r.isFinal))
                }
            }
        } else if let locale = await DictationTranscriber.supportedLocale(equivalentTo: wanted) {
            let t = DictationTranscriber(
                locale: locale, contentHints: [], transcriptionOptions: [.punctuation],
                reportingOptions: [.volatileResults, .frequentFinalization],
                attributeOptions: [.audioTimeRange])
            module = t
            listen = { send in
                for try await r in t.results {
                    send(Update(text: String(r.text.characters), range: r.range, isFinal: r.isFinal))
                }
            }
        } else {
            throw BridgeError("Apple speech recognition has no model for \(tag)")
        }
    }
}

@_cdecl("ol_stt_languages")
public func olSttLanguages() -> UnsafeMutablePointer<CChar>? {
    let tags = try? blocking {
        let all = await SpeechTranscriber.supportedLocales + DictationTranscriber.supportedLocales
        return Set(all.map { $0.identifier(.bcp47) }).sorted()
    }.get()
    return strdup((tags ?? []).joined(separator: "\n"))
}

/// 0 ready, 1 can be downloaded, 2 unsupported.
@_cdecl("ol_stt_status")
public func olSttStatus(_ tag: UnsafePointer<CChar>) -> Int32 {
    let tag = string(tag)
    let status = try? blocking {
        await AssetInventory.status(forModules: [try await Recognizer(tag).module])
    }.get()
    switch status {
    case .installed: return 0
    case .supported, .downloading: return 1
    default: return 2
    }
}

@_cdecl("ol_stt_install")
public func olSttInstall(
    _ tag: UnsafePointer<CChar>, _ context: UnsafeMutableRawPointer?,
    _ progress: @escaping ProgressCallback
) -> UnsafeMutablePointer<CChar>? {
    let tag = string(tag)
    let context = Box(context)
    let result: Result<Void, Error> = blocking {
        let module = try await Recognizer(tag).module
        // The system keeps a limited number of languages per app; make room for the new one.
        while await AssetInventory.reservedLocales.count >= AssetInventory.maximumReservedLocales,
            let oldest = await AssetInventory.reservedLocales.first
        {
            guard await AssetInventory.release(reservedLocale: oldest) else { break }
        }
        guard let request = try await AssetInventory.assetInstallationRequest(supporting: [module])
        else { return }
        let ticker = Task {
            while !Task.isCancelled {
                progress(context.value, request.progress.fractionCompleted)
                try await Task.sleep(for: .milliseconds(200))
            }
        }
        defer { ticker.cancel() }
        try await request.downloadAndInstall()
        ticker.cancel()
        // The caller's context is gone once this returns, so wait for the last tick.
        _ = await ticker.result
    }
    if case .failure(let error) = result { return text(error) }
    return nil
}

final class SpeechSession: @unchecked Sendable {
    private let analyzer: SpeechAnalyzer
    private let input: AsyncStream<AnalyzerInput>.Continuation
    private let converter: AVAudioConverter?
    private let source: AVAudioFormat
    private let target: AVAudioFormat
    private let reader: Task<Void, Error>
    private let gate = Gate()
    private var heardAudio = false

    init(_ tag: String, _ context: UnsafeMutableRawPointer?, _ callback: @escaping SpeechCallback)
        async throws
    {
        let recognizer = try await Recognizer(tag)
        guard
            let source = AVAudioFormat(
                commonFormat: .pcmFormatFloat32, sampleRate: 16_000, channels: 1, interleaved: false),
            let target = await SpeechAnalyzer.bestAvailableAudioFormat(
                compatibleWith: [recognizer.module])
        else { throw BridgeError("speech model for \(tag) is not installed") }
        self.source = source
        self.target = target
        converter = source == target ? nil : AVAudioConverter(from: source, to: target)
        converter?.primeMethod = .none
        analyzer = SpeechAnalyzer(modules: [recognizer.module])
        let (stream, continuation) = AsyncStream<AnalyzerInput>.makeStream()
        input = continuation
        let context = Box(context)
        let gate = gate
        reader = Task.detached(priority: .userInitiated) {
            do {
                try await recognizer.listen { u in
                    gate.pass {
                        callback(
                            context.value, u.text, u.range.start.seconds, u.range.end.seconds,
                            u.isFinal, nil)
                    }
                }
            } catch {
                gate.pass {
                    callback(context.value, nil, 0, 0, false, error.localizedDescription)
                }
                throw error
            }
        }
        try await analyzer.start(inputSequence: stream)
    }

    func push(_ samples: UnsafePointer<Float>, _ count: Int) throws {
        guard count > 0,
            let buffer = AVAudioPCMBuffer(pcmFormat: source, frameCapacity: AVAudioFrameCount(count))
        else { return }
        heardAudio = true
        buffer.frameLength = AVAudioFrameCount(count)
        buffer.floatChannelData![0].update(from: samples, count: count)
        guard let converter else {
            input.yield(AnalyzerInput(buffer: buffer))
            return
        }
        let capacity = AVAudioFrameCount(
            (Double(count) * target.sampleRate / source.sampleRate).rounded(.up)) + 64
        guard let out = AVAudioPCMBuffer(pcmFormat: target, frameCapacity: capacity) else { return }
        var pending: AVAudioPCMBuffer? = buffer
        var failure: NSError?
        converter.convert(to: out, error: &failure) { _, status in
            defer { pending = nil }
            status.pointee = pending == nil ? .noDataNow : .haveData
            return pending
        }
        if let failure { throw failure }
        if out.frameLength > 0 { input.yield(AnalyzerInput(buffer: out)) }
    }

    /// Never blocks for long, and no callback happens after it returns.
    func finish() throws {
        defer { gate.close() }
        input.finish()
        // Without any audio the result sequence never ends, so there is nothing to wait for.
        guard heardAudio else {
            reader.cancel()
            Task.detached { [analyzer] in await analyzer.cancelAndFinishNow() }
            return
        }
        let result = blocking(atMost: 4) { [analyzer, reader] in
            try await analyzer.finalizeAndFinishThroughEndOfInput()
            try await reader.value
        }
        guard let result else {
            reader.cancel()
            Task.detached { [analyzer] in await analyzer.cancelAndFinishNow() }
            throw BridgeError("speech recognition did not close in time")
        }
        try result.get()
    }
}

@_cdecl("ol_stt_open")
public func olSttOpen(
    _ tag: UnsafePointer<CChar>, _ context: UnsafeMutableRawPointer?,
    _ callback: @escaping SpeechCallback,
    _ error: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>
) -> UnsafeMutableRawPointer? {
    let tag = string(tag)
    let context = Box(context)
    switch blocking({ try await SpeechSession(tag, context.value, callback) }) {
    case .success(let session): return Unmanaged.passRetained(session).toOpaque()
    case .failure(let failure):
        error.pointee = text(failure)
        return nil
    }
}

@_cdecl("ol_stt_push")
public func olSttPush(
    _ session: UnsafeMutableRawPointer, _ samples: UnsafePointer<Float>, _ count: Int
) -> UnsafeMutablePointer<CChar>? {
    do {
        try Unmanaged<SpeechSession>.fromOpaque(session).takeUnretainedValue().push(samples, count)
        return nil
    } catch { return text(error) }
}

/// Delivers the remaining results, then releases the session.
@_cdecl("ol_stt_finish")
public func olSttFinish(_ session: UnsafeMutableRawPointer) -> UnsafeMutablePointer<CChar>? {
    let session = Unmanaged<SpeechSession>.fromOpaque(session).takeRetainedValue()
    do {
        try session.finish()
        return nil
    } catch { return text(error) }
}
