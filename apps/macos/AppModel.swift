import AppKit
import Foundation
import SwiftUI

enum AppSection: String, CaseIterable, Identifiable {
    case overview
    case analyze
    case receive
    case activity
    case about

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: return "概览"
        case .analyze: return "复用分析"
        case .receive: return "接收数据"
        case .activity: return "活动"
        case .about: return "关于"
        }
    }

    var symbol: String {
        switch self {
        case .overview: return "square.grid.2x2"
        case .analyze: return "sparkles.rectangle.stack"
        case .receive: return "arrow.down.circle"
        case .activity: return "waveform.path.ecg"
        case .about: return "info.circle"
        }
    }
}

struct CompareReport: Codable {
    let targetName: String
    let targetBytes: UInt64
    let targetChunks: UInt64
    let matchedChunks: UInt64
    let missingChunks: UInt64
    let localReusableBytes: UInt64
    let missingPayloadBytes: UInt64
    let reuseRatio: Double

    enum CodingKeys: String, CodingKey {
        case targetName = "target_name"
        case targetBytes = "target_bytes"
        case targetChunks = "target_chunks"
        case matchedChunks = "matched_chunks"
        case missingChunks = "missing_chunks"
        case localReusableBytes = "local_reusable_bytes"
        case missingPayloadBytes = "missing_payload_bytes"
        case reuseRatio = "reuse_ratio"
    }
}

struct IndexReport: Codable {
    let filesIndexed: UInt64
    let bytesIndexed: UInt64
    let chunksIndexed: UInt64
    let elapsedMs: UInt64

    enum CodingKeys: String, CodingKey {
        case filesIndexed = "files_indexed"
        case bytesIndexed = "bytes_indexed"
        case chunksIndexed = "chunks_indexed"
        case elapsedMs = "elapsed_ms"
    }
}

struct ActivityEntry: Identifiable, Codable {
    let id: UUID
    let date: Date
    let title: String
    let detail: String
    let succeeded: Bool

    init(title: String, detail: String, succeeded: Bool) {
        id = UUID()
        date = Date()
        self.title = title
        self.detail = detail
        self.succeeded = succeeded
    }
}

enum ReceiveKind: String, CaseIterable, Identifiable {
    case torrent = "Torrent 文件"
    case magnet = "Magnet 链接"
    var id: String { rawValue }
}

@MainActor
final class AppModel: ObservableObject {
    @Published var selection: AppSection = .overview
    @Published var libraryURL: URL?
    @Published var targetURL: URL?
    @Published var descriptorURL: URL?
    @Published var torrentURL: URL?
    @Published var outputURL: URL?
    @Published var magnet = ""
    @Published var metadataPeer = ""
    @Published var receiveKind: ReceiveKind = .torrent
    @Published var analysis: CompareReport?
    @Published var indexedFiles: UInt64 = 0
    @Published var indexedBytes: UInt64 = 0
    @Published var isBusy = false
    @Published var phase = "引擎就绪"
    @Published var progress = 0.0
    @Published var activities: [ActivityEntry] = []
    @Published var alertMessage: String?

    private let runner = EngineRunner()

    init() {
        loadSnapshot()
    }

    var canAnalyze: Bool {
        libraryURL != nil && targetURL != nil && !isBusy
    }

    var canReceive: Bool {
        guard libraryURL != nil, descriptorURL != nil, outputURL != nil, !isBusy else { return false }
        switch receiveKind {
        case .torrent:
            return torrentURL != nil
        case .magnet:
            return !magnet.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty &&
                (torrentURL != nil || !metadataPeer.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        }
    }

    func chooseLibrary() {
        libraryURL = chooseDirectory(title: "选择允许 ShardMeld 扫描的文件夹")
    }

    func chooseTarget() {
        targetURL = chooseFile(title: "选择要测算复用率的目标文件")
    }

    func chooseDescriptor() {
        descriptorURL = chooseFile(title: "选择 ShardMeld 描述文件", allowedExtensions: ["meld"])
    }

    func chooseTorrent() {
        torrentURL = chooseFile(title: "选择单文件 BitTorrent v1 元数据", allowedExtensions: ["torrent"])
    }

    func chooseOutput() {
        let panel = NSSavePanel()
        panel.title = "选择完成文件的保存位置"
        panel.nameFieldStringValue = descriptorURL?.deletingPathExtension().lastPathComponent ?? "ShardMeld-output"
        panel.canCreateDirectories = true
        if panel.runModal() == .OK { outputURL = panel.url }
    }

    func runAnalysis() {
        guard let libraryURL, let targetURL else { return }
        isBusy = true
        progress = 0.08
        phase = "正在建立本地材料索引…"

        Task {
            do {
                let run = try makeRunDirectory(prefix: "analysis")
                let db = run.appendingPathComponent("library.sqlite")
                let descriptor = run.appendingPathComponent("target.meld")
                let indexJSON = run.appendingPathComponent("index.json")
                let compareJSON = run.appendingPathComponent("compare.json")

                try await runner.run(["index", "--source", libraryURL.path, "--db", db.path, "--profile", "m", "--json", indexJSON.path])
                progress = 0.45
                phase = "正在描述目标内容…"

                try await runner.run(["describe", "--target", targetURL.path, "--out", descriptor.path, "--profile", "m"])
                progress = 0.68
                phase = "正在寻找可复用数据…"

                try await runner.run(["compare", "--descriptor", descriptor.path, "--db", db.path, "--json", compareJSON.path])
                let decoded = try JSONDecoder().decode(CompareReport.self, from: Data(contentsOf: compareJSON))
                let indexed = try JSONDecoder().decode(IndexReport.self, from: Data(contentsOf: indexJSON))

                analysis = decoded
                indexedFiles = indexed.filesIndexed
                indexedBytes = indexed.bytesIndexed
                descriptorURL = descriptor
                activities.insert(ActivityEntry(
                    title: "复用分析完成",
                    detail: "\(decoded.targetName) · 本地可复用 \(Self.percent(decoded.reuseRatio))",
                    succeeded: true
                ), at: 0)
                saveSnapshot()
                progress = 1
                phase = "分析完成"
                selection = .overview
            } catch {
                fail(title: "复用分析失败", error: error)
            }
            isBusy = false
        }
    }

    func runReceive() {
        guard let libraryURL, let descriptorURL, let outputURL else { return }
        isBusy = true
        progress = 0.06
        phase = "正在索引可复用的本地材料…"

        Task {
            do {
                let run = try makeRunDirectory(prefix: "receive")
                let db = run.appendingPathComponent("library.sqlite")
                let indexJSON = run.appendingPathComponent("index.json")
                let fetchJSON = run.appendingPathComponent("fetch.json")
                try await runner.run(["index", "--source", libraryURL.path, "--db", db.path, "--profile", "m", "--json", indexJSON.path])
                progress = 0.28
                phase = "正在连接 BitTorrent 网络并重建…"

                var arguments: [String]
                switch receiveKind {
                case .torrent:
                    guard let torrentURL else { throw UIError.missingInput("请选择 Torrent 文件") }
                    arguments = [
                        "bt-fetch-tracker", "--torrent", torrentURL.path,
                        "--descriptor", descriptorURL.path, "--db", db.path,
                        "--out", outputURL.path, "--json", fetchJSON.path
                    ]
                case .magnet:
                    arguments = ["bt-fetch-magnet", "--magnet", magnet]
                    if let torrentURL {
                        arguments += ["--metadata", torrentURL.path]
                    } else {
                        arguments += ["--metadata-peer", metadataPeer]
                    }
                    arguments += [
                        "--descriptor", descriptorURL.path, "--db", db.path,
                        "--out", outputURL.path, "--json", fetchJSON.path
                    ]
                }

                try await runner.run(arguments)
                progress = 1
                phase = "接收完成并已校验"
                activities.insert(ActivityEntry(
                    title: "接收完成",
                    detail: outputURL.lastPathComponent + " · SHA-256 校验通过",
                    succeeded: true
                ), at: 0)
                saveSnapshot()
                selection = .activity
            } catch {
                fail(title: "接收失败", error: error)
            }
            isBusy = false
        }
    }

    func revealOutput() {
        guard let outputURL else { return }
        NSWorkspace.shared.activateFileViewerSelecting([outputURL])
    }

    func clearActivity() {
        activities.removeAll()
        saveSnapshot()
    }

    private func fail(title: String, error: Error) {
        progress = 0
        phase = "需要处理"
        let detail = error.localizedDescription
        activities.insert(ActivityEntry(title: title, detail: detail, succeeded: false), at: 0)
        alertMessage = detail
        saveSnapshot()
    }

    private func makeRunDirectory(prefix: String) throws -> URL {
        let root = try applicationSupportDirectory()
        let run = root.appendingPathComponent("Runs", isDirectory: true)
            .appendingPathComponent("\(prefix)-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: run, withIntermediateDirectories: true)
        return run
    }

    private func applicationSupportDirectory() throws -> URL {
        guard let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else {
            throw UIError.missingInput("无法访问应用支持目录")
        }
        let root = base.appendingPathComponent("ShardMeld", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    private func chooseDirectory(title: String) -> URL? {
        let panel = NSOpenPanel()
        panel.title = title
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        return panel.runModal() == .OK ? panel.url : nil
    }

    private func chooseFile(title: String, allowedExtensions: [String] = []) -> URL? {
        let panel = NSOpenPanel()
        panel.title = title
        panel.canChooseFiles = true
        panel.canChooseDirectories = false
        panel.allowsMultipleSelection = false
        if !allowedExtensions.isEmpty {
            panel.allowedContentTypes = allowedExtensions.compactMap { UTType(filenameExtension: $0) }
        }
        return panel.runModal() == .OK ? panel.url : nil
    }

    private func saveSnapshot() {
        let encoder = JSONEncoder()
        if let analysis, let data = try? encoder.encode(analysis) {
            UserDefaults.standard.set(data, forKey: "lastAnalysis")
        }
        if let data = try? encoder.encode(Array(activities.prefix(30))) {
            UserDefaults.standard.set(data, forKey: "activities")
        }
        UserDefaults.standard.set(indexedFiles, forKey: "indexedFiles")
        UserDefaults.standard.set(indexedBytes, forKey: "indexedBytes")
    }

    private func loadSnapshot() {
        let decoder = JSONDecoder()
        if let data = UserDefaults.standard.data(forKey: "lastAnalysis") {
            analysis = try? decoder.decode(CompareReport.self, from: data)
        }
        if let data = UserDefaults.standard.data(forKey: "activities") {
            activities = (try? decoder.decode([ActivityEntry].self, from: data)) ?? []
        }
        indexedFiles = UInt64(UserDefaults.standard.object(forKey: "indexedFiles") as? Int ?? 0)
        indexedBytes = UInt64(UserDefaults.standard.object(forKey: "indexedBytes") as? Int ?? 0)
    }

    static func percent(_ value: Double) -> String {
        value.formatted(.percent.precision(.fractionLength(1)))
    }
}

private enum UIError: LocalizedError {
    case missingInput(String)
    var errorDescription: String? {
        switch self { case .missingInput(let message): return message }
    }
}

import UniformTypeIdentifiers
