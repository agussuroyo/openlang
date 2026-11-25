import Foundation

struct BridgeError: LocalizedError {
    let errorDescription: String?
    init(_ message: String) { errorDescription = message }
}

final class Box<T>: @unchecked Sendable {
    var value: T
    init(_ value: T) { self.value = value }
}

/// Runs async work from a synchronous C call. Must not be called on the main thread, because
/// Translation needs a live main run loop to make progress.
func blocking<T>(_ work: @escaping @Sendable () async throws -> T) -> Result<T, Error> {
    let done = DispatchSemaphore(value: 0)
    let box = Box<Result<T, Error>?>(nil)
    Task.detached(priority: .userInitiated) {
        do { box.value = .success(try await work()) } catch { box.value = .failure(error) }
        done.signal()
    }
    done.wait()
    return box.value!
}

/// Same, but gives up waiting. `nil` means the work is still running.
func blocking<T>(atMost seconds: Double, _ work: @escaping @Sendable () async throws -> T)
    -> Result<T, Error>?
{
    let done = DispatchSemaphore(value: 0)
    let box = Box<Result<T, Error>?>(nil)
    Task.detached(priority: .userInitiated) {
        do { box.value = .success(try await work()) } catch { box.value = .failure(error) }
        done.signal()
    }
    return done.wait(timeout: .now() + seconds) == .success ? box.value : nil
}

/// Lets callbacks through until closed. Once `close` returns, none is running or will run.
final class Gate: @unchecked Sendable {
    private let lock = NSLock()
    private var isOpen = true

    func pass(_ body: () -> Void) {
        lock.lock()
        defer { lock.unlock() }
        if isOpen { body() }
    }

    func close() {
        lock.lock()
        isOpen = false
        lock.unlock()
    }
}

func text(_ error: Error) -> UnsafeMutablePointer<CChar>? { strdup(error.localizedDescription) }

func string(_ pointer: UnsafePointer<CChar>) -> String { String(cString: pointer) }

@_cdecl("ol_free") public func olFree(_ pointer: UnsafeMutablePointer<CChar>?) { free(pointer) }
