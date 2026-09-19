import SwiftUI

private let canvas = Color(red: 0.075, green: 0.075, blue: 0.078)
private let sidebar = Color(red: 0.052, green: 0.052, blue: 0.055)
private let surface = Color(red: 0.105, green: 0.105, blue: 0.11)
private let line = Color.white.opacity(0.085)
private let primaryText = Color.white.opacity(0.94)
private let mutedText = Color.white.opacity(0.48)
private let quietText = Color.white.opacity(0.30)
private let signal = Color(red: 0.56, green: 0.72, blue: 1.0)
private let success = Color(red: 0.50, green: 0.78, blue: 0.65)

struct RootView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 0) {
            SidebarView()
                .frame(width: 218)
            Rectangle().fill(line).frame(width: 1)
            Group {
                switch model.selection {
                case .overview: OverviewView()
                case .analyze: AnalyzeView()
                case .receive: ReceiveView()
                case .activity: ActivityView()
                case .about: AboutView()
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(canvas)
        }
        .background(canvas)
        .alert("ShardMeld", isPresented: Binding(
            get: { model.alertMessage != nil },
            set: { if !$0 { model.alertMessage = nil } }
        )) {
            Button("好") { model.alertMessage = nil }
        } message: {
            Text(model.alertMessage ?? "")
        }
    }
}

private struct SidebarView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                BrandGlyph()
                Text("ShardMeld")
                    .font(.system(size: 14, weight: .semibold))
                    .foregroundStyle(primaryText)
            }
            .padding(.top, 27)
            .padding(.horizontal, 18)
            .padding(.bottom, 27)

            VStack(spacing: 3) {
                ForEach(AppSection.allCases) { section in
                    Button {
                        model.selection = section
                    } label: {
                        HStack(spacing: 11) {
                            Image(systemName: section.symbol)
                                .font(.system(size: 13, weight: .medium))
                                .frame(width: 18)
                            Text(section.title)
                                .font(.system(size: 13, weight: .medium))
                            Spacer()
                        }
                        .foregroundStyle(model.selection == section ? primaryText : mutedText)
                        .padding(.horizontal, 11)
                        .frame(height: 38)
                        .background(
                            RoundedRectangle(cornerRadius: 7, style: .continuous)
                                .fill(model.selection == section ? Color.white.opacity(0.075) : .clear)
                        )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 9)

            Spacer()

            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 8) {
                    Circle()
                        .fill(model.isBusy ? signal : success)
                        .frame(width: 6, height: 6)
                    Text(model.phase)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(mutedText)
                        .lineLimit(1)
                }
                Text("本地处理 · 明确授权")
                    .font(.system(size: 10))
                    .foregroundStyle(quietText)
            }
            .padding(.horizontal, 19)
            .padding(.bottom, 20)
        }
        .background(sidebar)
    }
}

private struct Page<Content: View>: View {
    let title: String
    let subtitle: String
    @ViewBuilder let content: Content

    var body: some View {
        VStack(spacing: 0) {
            HStack(alignment: .center) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(title)
                        .font(.system(size: 14, weight: .semibold))
                        .foregroundStyle(primaryText)
                    Text(subtitle)
                        .font(.system(size: 11))
                        .foregroundStyle(mutedText)
                }
                Spacer()
                Text("2.2")
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .foregroundStyle(quietText)
            }
            .padding(.horizontal, 30)
            .frame(height: 66)

            Rectangle().fill(line).frame(height: 1)

            ScrollView {
                content
                    .frame(maxWidth: 880, alignment: .leading)
                    .padding(.horizontal, 46)
                    .padding(.vertical, 44)
            }
        }
    }
}

struct OverviewView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Page(title: "概览", subtitle: "只传真正缺少的数据") {
            VStack(alignment: .leading, spacing: 0) {
                Text(model.analysis == nil ? "少下载，先重建。" : headline)
                    .font(.system(size: 42, weight: .medium))
                    .tracking(-1.3)
                    .foregroundStyle(primaryText)
                    .padding(.bottom, 13)

                Text(model.analysis == nil ?
                     "ShardMeld 会先在你允许的本地材料中寻找目标内容，再通过 BitTorrent 获取剩余部分。" :
                     "\(model.analysis!.targetName) 中已有 \(AppModel.percent(model.analysis!.reuseRatio)) 可以从本机直接恢复。")
                    .font(.system(size: 15))
                    .foregroundStyle(mutedText)
                    .lineSpacing(4)
                    .frame(maxWidth: 650, alignment: .leading)

                HStack(spacing: 10) {
                    SolidButton(title: model.analysis == nil ? "分析本地复用" : "重新分析", symbol: "sparkles") {
                        model.selection = .analyze
                    }
                    TextButton(title: "接收数据", symbol: "arrow.down") {
                        model.selection = .receive
                    }
                }
                .padding(.top, 25)

                DataBand().padding(.top, 52)

                HStack(spacing: 0) {
                    PlainMetric(label: "目标大小", value: model.analysis.map { byteString($0.targetBytes) } ?? "—")
                    VerticalRule()
                    PlainMetric(label: "本地可用", value: model.analysis.map { byteString($0.localReusableBytes) } ?? "—")
                    VerticalRule()
                    PlainMetric(label: "需要网络", value: model.analysis.map { byteString($0.missingPayloadBytes) } ?? "—")
                    VerticalRule()
                    PlainMetric(label: "已索引文件", value: model.indexedFiles == 0 ? "—" : model.indexedFiles.formatted())
                }
                .padding(.vertical, 28)

                Rectangle().fill(line).frame(height: 1)

                SectionHeading(title: "最近活动", actionTitle: model.activities.isEmpty ? nil : "查看全部") {
                    model.selection = .activity
                }
                .padding(.top, 28)
                .padding(.bottom, 10)

                if model.isBusy {
                    RunningRow()
                } else if let entry = model.activities.first {
                    ActivityRow(entry: entry)
                } else {
                    Text("完成一次分析或接收后，结果会出现在这里。")
                        .font(.system(size: 13))
                        .foregroundStyle(quietText)
                        .padding(.vertical, 18)
                }
            }
        }
    }

    private var headline: String {
        guard let analysis = model.analysis else { return "少下载，先重建。" }
        return "省下 \(byteString(analysis.localReusableBytes))。"
    }
}

private struct DataBand: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 13) {
            HStack {
                Text("数据构成")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(primaryText)
                Spacer()
                if let analysis = model.analysis {
                    Text("\(analysis.matchedChunks.formatted()) / \(analysis.targetChunks.formatted()) 个数据块来自本地")
                        .font(.system(size: 11))
                        .foregroundStyle(mutedText)
                } else {
                    Text("等待分析")
                        .font(.system(size: 11))
                        .foregroundStyle(quietText)
                }
            }

            GeometryReader { proxy in
                let ratio = max(0, min(1, model.analysis?.reuseRatio ?? 0))
                HStack(spacing: 3) {
                    Rectangle()
                        .fill(model.analysis == nil ? Color.white.opacity(0.10) : primaryText)
                        .frame(width: max(0, (proxy.size.width - 3) * ratio))
                    Rectangle()
                        .fill(model.analysis == nil ? Color.white.opacity(0.05) : signal.opacity(0.70))
                }
            }
            .frame(height: 7)
            .clipShape(Capsule())

            HStack(spacing: 20) {
                LegendDot(color: primaryText, title: "本地重建")
                LegendDot(color: signal.opacity(0.8), title: "网络获取")
            }
        }
    }
}

struct AnalyzeView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Page(title: "复用分析", subtitle: "计算本机已拥有的数据") {
            VStack(alignment: .leading, spacing: 0) {
                Text("你已经拥有多少？")
                    .font(.system(size: 34, weight: .medium))
                    .tracking(-0.8)
                    .foregroundStyle(primaryText)
                Text("选择一个本地材料文件夹和目标文件。索引只记录数据位置与哈希，不复制原始内容。")
                    .font(.system(size: 14))
                    .foregroundStyle(mutedText)
                    .lineSpacing(4)
                    .padding(.top, 10)
                    .padding(.bottom, 33)

                FormSurface {
                    SelectRow(index: "01", title: "本地材料", detail: model.libraryURL?.path ?? "选择允许读取的文件夹", selected: model.libraryURL != nil, symbol: "folder") {
                        model.chooseLibrary()
                    }
                    RowDivider()
                    SelectRow(index: "02", title: "目标文件", detail: model.targetURL?.path ?? "选择要测算的目标文件", selected: model.targetURL != nil, symbol: "doc") {
                        model.chooseTarget()
                    }
                }

                if let analysis = model.analysis {
                    HStack {
                        Text("上次结果")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(mutedText)
                        Spacer()
                        Text("\(AppModel.percent(analysis.reuseRatio)) 本地复用 · 节省 \(byteString(analysis.localReusableBytes))")
                            .font(.system(size: 12, weight: .medium))
                            .foregroundStyle(primaryText)
                    }
                    .padding(.top, 18)
                }

                FormAction(note: model.isBusy ? model.phase : "所有读取均限制在你明确选择的范围内。", progress: model.isBusy ? model.progress : nil, title: "开始分析", enabled: model.canAnalyze) {
                    model.runAnalysis()
                }
            }
        }
    }
}

struct ReceiveView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Page(title: "接收数据", subtitle: "BitTorrent 兼容接收") {
            VStack(alignment: .leading, spacing: 0) {
                Text("补齐缺少的部分。")
                    .font(.system(size: 34, weight: .medium))
                    .tracking(-0.8)
                    .foregroundStyle(primaryText)
                Text("从本地材料先重建，再向标准 BitTorrent v1 Peer 请求剩余数据。")
                    .font(.system(size: 14))
                    .foregroundStyle(mutedText)
                    .padding(.top, 10)
                    .padding(.bottom, 28)

                SourceSwitch(selection: $model.receiveKind).padding(.bottom, 15)

                FormSurface {
                    SelectRow(index: "01", title: "本地材料", detail: model.libraryURL?.path ?? "选择允许读取的文件夹", selected: model.libraryURL != nil, symbol: "folder") { model.chooseLibrary() }
                    RowDivider()
                    SelectRow(index: "02", title: "目标描述", detail: model.descriptorURL?.path ?? "选择发布者提供的 .meld 文件", selected: model.descriptorURL != nil, symbol: "checkmark.seal") { model.chooseDescriptor() }
                    RowDivider()

                    if model.receiveKind == .torrent {
                        SelectRow(index: "03", title: "Torrent", detail: model.torrentURL?.path ?? "选择单文件 BitTorrent v1 元数据", selected: model.torrentURL != nil, symbol: "link") { model.chooseTorrent() }
                    } else {
                        TextInputRow(index: "03", title: "Magnet", placeholder: "magnet:?xt=urn:btih:…", text: $model.magnet, symbol: "link")
                        RowDivider()
                        SelectRow(index: "04", title: "本地元数据", detail: model.torrentURL?.path ?? "可选：选择匹配的 .torrent", selected: model.torrentURL != nil, symbol: "doc") { model.chooseTorrent() }
                        RowDivider()
                        TextInputRow(index: "—", title: "或元数据 Peer", placeholder: "127.0.0.1:6881", text: $model.metadataPeer, symbol: "network")
                    }

                    RowDivider()
                    SelectRow(index: model.receiveKind == .torrent ? "04" : "05", title: "保存位置", detail: model.outputURL?.path ?? "选择完成文件的保存位置", selected: model.outputURL != nil, symbol: "arrow.down.doc") { model.chooseOutput() }
                }

                FormAction(note: model.isBusy ? model.phase : "完成文件只会在 Piece SHA-1 与目标 SHA-256 全部通过后发布。", progress: model.isBusy ? model.progress : nil, title: "开始接收", enabled: model.canReceive) {
                    model.runReceive()
                }

                Text("当前支持单文件 v1 Torrent。DHT、PEX、多文件 Torrent 和 v2/hybrid 尚未实现。")
                    .font(.system(size: 10))
                    .foregroundStyle(quietText)
                    .padding(.top, 22)
            }
        }
    }
}

struct ActivityView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Page(title: "活动", subtitle: "保存在这台 Mac 上") {
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text("最近任务")
                        .font(.system(size: 34, weight: .medium))
                        .tracking(-0.8)
                        .foregroundStyle(primaryText)
                    Spacer()
                    if !model.activities.isEmpty {
                        Button("清除") { model.clearActivity() }
                            .buttonStyle(.plain)
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(mutedText)
                    }
                }
                .padding(.bottom, 27)

                if model.activities.isEmpty {
                    Text("还没有活动。完成一次复用分析或接收任务后，结果会出现在这里。")
                        .font(.system(size: 14))
                        .foregroundStyle(mutedText)
                        .padding(.vertical, 28)
                } else {
                    ForEach(Array(model.activities.enumerated()), id: \.element.id) { index, entry in
                        ActivityRow(entry: entry)
                        if index < model.activities.count - 1 {
                            Rectangle().fill(line).frame(height: 1)
                        }
                    }
                }
            }
        }
    }
}

struct AboutView: View {
    var body: some View {
        Page(title: "关于", subtitle: "ShardMeld Desktop") {
            VStack(alignment: .leading, spacing: 0) {
                BrandGlyph(size: 46).padding(.bottom, 25)
                Text("ShardMeld")
                    .font(.system(size: 38, weight: .medium))
                    .tracking(-1)
                    .foregroundStyle(primaryText)
                Text("拾构 · 2.2 for macOS")
                    .font(.system(size: 14))
                    .foregroundStyle(mutedText)
                    .padding(.top, 7)

                Text("Reconstruct first.\nTransfer only what’s missing.")
                    .font(.system(size: 21))
                    .foregroundStyle(primaryText)
                    .lineSpacing(5)
                    .padding(.top, 38)

                Rectangle().fill(line).frame(height: 1).padding(.vertical, 34)

                VStack(alignment: .leading, spacing: 15) {
                    AboutLine(title: "索引", detail: "只读取你明确选择的范围")
                    AboutLine(title: "存储", detail: "不复制本地材料的原始数据")
                    AboutLine(title: "完整性", detail: "SHA-1 Piece 与目标 SHA-256 双重校验")
                    AboutLine(title: "许可", detail: "GNU AGPL v3.0 only")
                }

                Text("Copyright © 2026 YeraldoSmith")
                    .font(.system(size: 11))
                    .foregroundStyle(quietText)
                    .padding(.top, 44)
            }
        }
    }
}

private struct FormSurface<Content: View>: View {
    @ViewBuilder let content: Content
    var body: some View {
        VStack(spacing: 0) { content }
            .background(surface)
            .clipShape(RoundedRectangle(cornerRadius: 11, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 11, style: .continuous).stroke(line, lineWidth: 1))
    }
}

private struct SelectRow: View {
    let index: String
    let title: String
    let detail: String
    let selected: Bool
    let symbol: String
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 14) {
                Text(index)
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .foregroundStyle(quietText)
                    .frame(width: 22, alignment: .leading)
                Image(systemName: symbol)
                    .font(.system(size: 14, weight: .medium))
                    .foregroundStyle(mutedText)
                    .frame(width: 20)
                VStack(alignment: .leading, spacing: 5) {
                    Text(title).font(.system(size: 13, weight: .medium)).foregroundStyle(primaryText)
                    Text(detail)
                        .font(.system(size: 11))
                        .foregroundStyle(selected ? mutedText : quietText)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
                Spacer()
                Text(selected ? "更改" : "选择")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(mutedText)
                Image(systemName: "chevron.right")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(quietText)
            }
            .padding(.horizontal, 18)
            .frame(height: 72)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

private struct TextInputRow: View {
    let index: String
    let title: String
    let placeholder: String
    @Binding var text: String
    let symbol: String

    var body: some View {
        HStack(spacing: 14) {
            Text(index)
                .font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(quietText)
                .frame(width: 22, alignment: .leading)
            Image(systemName: symbol)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(mutedText)
                .frame(width: 20)
            VStack(alignment: .leading, spacing: 5) {
                Text(title).font(.system(size: 13, weight: .medium)).foregroundStyle(primaryText)
                TextField(placeholder, text: $text)
                    .textFieldStyle(.plain)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(mutedText)
            }
        }
        .padding(.horizontal, 18)
        .frame(height: 72)
    }
}

private struct SourceSwitch: View {
    @Binding var selection: ReceiveKind
    var body: some View {
        HStack(spacing: 3) {
            ForEach(ReceiveKind.allCases) { kind in
                Button {
                    selection = kind
                } label: {
                    Text(kind.rawValue)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(selection == kind ? primaryText : mutedText)
                        .padding(.horizontal, 13)
                        .frame(height: 30)
                        .background(RoundedRectangle(cornerRadius: 6, style: .continuous).fill(selection == kind ? Color.white.opacity(0.09) : .clear))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(3)
        .background(surface)
        .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct FormAction: View {
    let note: String
    let progress: Double?
    let title: String
    let enabled: Bool
    let action: () -> Void

    var body: some View {
        VStack(spacing: 14) {
            HStack {
                Text(note).font(.system(size: 11)).foregroundStyle(mutedText)
                Spacer()
                SolidButton(title: title, symbol: "arrow.right", action: action)
                    .disabled(!enabled)
                    .opacity(enabled ? 1 : 0.32)
            }
            if let progress {
                ProgressView(value: progress).tint(signal)
            }
        }
        .padding(.top, 20)
    }
}

private struct PlainMetric: View {
    let label: String
    let value: String
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(label).font(.system(size: 10, weight: .medium)).foregroundStyle(quietText)
            Text(value).font(.system(size: 19, weight: .medium)).foregroundStyle(primaryText)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct VerticalRule: View {
    var body: some View {
        Rectangle().fill(line).frame(width: 1, height: 39).padding(.horizontal, 20)
    }
}

private struct LegendDot: View {
    let color: Color
    let title: String
    var body: some View {
        HStack(spacing: 7) {
            Circle().fill(color).frame(width: 6, height: 6)
            Text(title).font(.system(size: 10)).foregroundStyle(mutedText)
        }
    }
}

private struct SectionHeading: View {
    let title: String
    let actionTitle: String?
    let action: () -> Void
    var body: some View {
        HStack {
            Text(title).font(.system(size: 12, weight: .semibold)).foregroundStyle(primaryText)
            Spacer()
            if let actionTitle {
                Button(actionTitle, action: action)
                    .buttonStyle(.plain)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(mutedText)
            }
        }
    }
}

private struct ActivityRow: View {
    let entry: ActivityEntry
    var body: some View {
        HStack(spacing: 13) {
            Image(systemName: entry.succeeded ? "checkmark" : "exclamationmark")
                .font(.system(size: 10, weight: .bold))
                .foregroundStyle(entry.succeeded ? success : Color.orange)
                .frame(width: 24, height: 24)
                .background(Color.white.opacity(0.055), in: Circle())
            VStack(alignment: .leading, spacing: 4) {
                Text(entry.title).font(.system(size: 12, weight: .medium)).foregroundStyle(primaryText)
                Text(entry.detail).font(.system(size: 11)).foregroundStyle(mutedText).lineLimit(1)
            }
            Spacer()
            Text(entry.date.formatted(date: .abbreviated, time: .shortened))
                .font(.system(size: 10))
                .foregroundStyle(quietText)
        }
        .padding(.vertical, 15)
    }
}

private struct RunningRow: View {
    @EnvironmentObject private var model: AppModel
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text(model.phase).font(.system(size: 12, weight: .medium)).foregroundStyle(primaryText)
                Spacer()
                Text(AppModel.percent(model.progress)).font(.system(size: 10, design: .monospaced)).foregroundStyle(mutedText)
            }
            ProgressView(value: model.progress).tint(signal)
        }
        .padding(.vertical, 15)
    }
}

private struct AboutLine: View {
    let title: String
    let detail: String
    var body: some View {
        HStack(alignment: .firstTextBaseline) {
            Text(title).font(.system(size: 12, weight: .medium)).foregroundStyle(mutedText).frame(width: 75, alignment: .leading)
            Text(detail).font(.system(size: 12)).foregroundStyle(primaryText)
        }
    }
}

private struct SolidButton: View {
    let title: String
    let symbol: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title)
                Image(systemName: symbol).font(.system(size: 10, weight: .semibold))
            }
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(Color.black.opacity(0.86))
            .padding(.horizontal, 15)
            .frame(height: 36)
            .background(primaryText)
            .clipShape(RoundedRectangle(cornerRadius: 8, style: .continuous))
        }
        .buttonStyle(.plain)
    }
}

private struct TextButton: View {
    let title: String
    let symbol: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) {
                Text(title)
                Image(systemName: symbol).font(.system(size: 10, weight: .semibold))
            }
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(primaryText)
            .padding(.horizontal, 14)
            .frame(height: 36)
            .overlay(RoundedRectangle(cornerRadius: 8, style: .continuous).stroke(line, lineWidth: 1))
        }
        .buttonStyle(.plain)
    }
}

private struct BrandGlyph: View {
    var size: CGFloat = 27
    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.25, style: .continuous).fill(primaryText)
            Image(systemName: "square.stack.3d.up.fill")
                .font(.system(size: size * 0.42, weight: .semibold))
                .foregroundStyle(Color.black.opacity(0.84))
        }
        .frame(width: size, height: size)
    }
}

private struct RowDivider: View {
    var body: some View {
        Rectangle().fill(line).frame(height: 1).padding(.leading, 74)
    }
}

private func byteString(_ value: UInt64) -> String {
    ByteCountFormatter.string(fromByteCount: Int64(value), countStyle: .file)
}
