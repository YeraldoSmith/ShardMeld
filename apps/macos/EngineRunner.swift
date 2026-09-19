import Foundation

struct EngineFailure: LocalizedError {
    let message: String
    var errorDescription: String? { message }
}

final class EngineRunner: @unchecked Sendable {
    func run(_ arguments: [String]) async throws {
        let executable = try engineURL()
        try await withCheckedThrowingContinuation { continuation in
            let process = Process()
            let output = Pipe()
            let errors = Pipe()
            process.executableURL = executable
            process.arguments = arguments
            process.standardOutput = output
            process.standardError = errors
            process.terminationHandler = { process in
                let stdout = output.fileHandleForReading.readDataToEndOfFile()
                let stderr = errors.fileHandleForReading.readDataToEndOfFile()
                if process.terminationStatus == 0 {
                    continuation.resume()
                    return
                }
                let detailData = stderr.isEmpty ? stdout : stderr
                let detail = String(data: detailData, encoding: .utf8)?
                    .trimmingCharacters(in: .whitespacesAndNewlines)
                continuation.resume(throwing: EngineFailure(
                    message: detail?.isEmpty == false ? detail! : "ShardMeld 引擎返回错误 \(process.terminationStatus)"
                ))
            }
            do {
                try process.run()
            } catch {
                continuation.resume(throwing: error)
            }
        }
    }

    private func engineURL() throws -> URL {
        if let bundled = Bundle.main.url(forResource: "shardmeld", withExtension: nil) {
            return bundled
        }
        let development = URL(fileURLWithPath: FileManager.default.currentDirectoryPath)
            .appendingPathComponent("dist/shardmeld-macos-arm64")
        if FileManager.default.isExecutableFile(atPath: development.path) {
            return development
        }
        throw EngineFailure(message: "应用内未找到 ShardMeld 引擎，请重新安装完整应用。")
    }
}
