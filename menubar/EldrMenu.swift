import AppKit
import Combine
import Darwin
import Dispatch
import Foundation
import SwiftUI

struct EldrProcess: Decodable, Equatable, Identifiable {
    let pid: Int
    let name: String
    let cpu: Double?
    let memory: UInt64?

    var id: Int { pid }

    enum CodingKeys: String, CodingKey {
        case pid, name, cpu
        case memory = "mem"
    }
}

struct EldrStatus: Decodable, Equatable {
    let schemaVersion: String?
    let timestamp: String?
    let source: String?
    let level: String?
    let chip: String?
    let cpuUsage: Double?
    let cpuLoad: Double?
    let cpuTemperature: Double?
    let gpuTemperature: Double?
    let systemPower: Double?
    let memoryTotal: UInt64?
    let memoryUsed: UInt64?
    let memoryAvailable: UInt64?
    let memoryCompressed: UInt64?
    let memoryPressure: String?
    let swapUsed: UInt64?
    let swapTotal: UInt64?
    let thermal: String?
    let fanRPM: UInt64?
    let fanTargetRPM: UInt64?
    let batteryPercent: UInt64?
    let batteryState: String?
    let batteryTimeMinutes: UInt64?
    let diskTotal: UInt64?
    let diskFree: UInt64?
    let networkReceiveRate: Double?
    let networkTransmitRate: Double?
    let topProcesses: [EldrProcess]?
    let topMemory: [EldrProcess]?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case timestamp = "ts"
        case source, level, chip
        case cpuUsage = "cpu_usage_pct"
        case cpuLoad = "cpu_load_pct"
        case cpuTemperature = "cpu_temp"
        case gpuTemperature = "gpu_temp"
        case systemPower = "sys_power"
        case memoryTotal = "ram_total"
        case memoryUsed = "ram_used"
        case memoryAvailable = "ram_available"
        case memoryCompressed = "ram_compressed"
        case memoryPressure = "mem_pressure"
        case swapUsed = "swap_used"
        case swapTotal = "swap_total"
        case thermal
        case fanRPM = "fan_rpm"
        case fanTargetRPM = "fan_target"
        case batteryPercent = "battery_percent"
        case batteryState = "battery_state"
        case batteryTimeMinutes = "battery_time_min"
        case diskTotal = "disk_total"
        case diskFree = "disk_free"
        case networkReceiveRate = "net_rx_rate"
        case networkTransmitRate = "net_tx_rate"
        case topProcesses = "top_procs"
        case topMemory = "top_mem"
    }
}

struct EldrGuardHeartbeat: Decodable, Equatable {
    let schemaVersion: String?
    let kind: String?
    let pid: Int?
    let heartbeatAt: UInt64?
    let intervalSeconds: UInt64?
    let statusSampleTimestamp: String?
    let sequence: UInt64?

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case kind, pid, sequence
        case heartbeatAt = "heartbeat_at"
        case intervalSeconds = "interval_seconds"
        case statusSampleTimestamp = "status_sample_ts"
    }
}

struct EldrUpdateCache: Decodable, Equatable {
    let latest: String?
}

struct EldrMenuPaths: Equatable {
    let directory: URL

    var statusURL: URL { directory.appendingPathComponent("status.json") }
    var heartbeatURL: URL { directory.appendingPathComponent("menubar.json") }
    var updateURL: URL { directory.appendingPathComponent("update_check.json") }

    static func current(
        environment: [String: String] = ProcessInfo.processInfo.environment,
        home: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> Self {
        if let configured = environment["ELDR_DIR"], !configured.isEmpty {
            return EldrMenuPaths(directory: URL(fileURLWithPath: configured, isDirectory: true))
        }
        if let configured = launchAgentDataDirectory(home: home) {
            return EldrMenuPaths(directory: configured)
        }
        if let configured = configDataDirectory(home: home) {
            return EldrMenuPaths(directory: configured)
        }
        return EldrMenuPaths(directory: home.appendingPathComponent(".local/share/eldr", isDirectory: true))
    }

    private static func launchAgentDataDirectory(home: URL) -> URL? {
        let plist = home.appendingPathComponent("Library/LaunchAgents/com.petruarakiss.eldr.guard.plist")
        guard let data = try? Data(contentsOf: plist),
              let root = try? PropertyListSerialization.propertyList(from: data, format: nil),
              let dictionary = root as? [String: Any],
              let environment = dictionary["EnvironmentVariables"] as? [String: Any],
              let raw = environment["ELDR_DIR"] as? String,
              !raw.isEmpty
        else {
            return nil
        }
        return URL(fileURLWithPath: raw, isDirectory: true)
    }

    private static func configDataDirectory(home: URL) -> URL? {
        let config = home.appendingPathComponent(".config/eldr/config.toml")
        guard let text = try? String(contentsOf: config, encoding: .utf8) else { return nil }
        for rawLine in text.split(whereSeparator: { $0.isNewline }) {
            let line = String(rawLine).trimmingCharacters(in: .whitespaces)
            guard !line.hasPrefix("#"), let separator = line.firstIndex(of: "=") else { continue }
            let key = line[..<separator].trimmingCharacters(in: .whitespaces)
            guard key == "ELDR_DIR" else { continue }
            let value = line[line.index(after: separator)...]
                .trimmingCharacters(in: .whitespaces)
                .trimmingCharacters(in: CharacterSet(charactersIn: "\"'"))
            if !value.isEmpty { return URL(fileURLWithPath: value, isDirectory: true) }
        }
        return nil
    }
}

enum EldrMenuReadError: LocalizedError, Equatable {
    case missingStatus
    case invalidStatus
    case unsupportedSchema(String)

    var errorDescription: String? {
        switch self {
        case .missingStatus:
            return "No Eldr status is available yet."
        case .invalidStatus:
            return "Eldr wrote an unreadable status file."
        case let .unsupportedSchema(version):
            return "Eldr status schema \(version) is not supported by this app."
        }
    }
}

enum EldrMenuReader {
    static func readStatus(from url: URL) throws -> EldrStatus {
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw EldrMenuReadError.missingStatus
        }
        guard let status = try? JSONDecoder().decode(EldrStatus.self, from: Data(contentsOf: url)) else {
            throw EldrMenuReadError.invalidStatus
        }
        if let version = status.schemaVersion, version != "1" {
            throw EldrMenuReadError.unsupportedSchema(version)
        }
        return status
    }

    static func readHeartbeat(from url: URL) -> EldrGuardHeartbeat? {
        guard let data = try? Data(contentsOf: url),
              let heartbeat = try? JSONDecoder().decode(EldrGuardHeartbeat.self, from: data),
              heartbeat.schemaVersion == nil || heartbeat.schemaVersion == "1",
              heartbeat.kind == nil || heartbeat.kind == "eldr.menubar"
        else {
            return nil
        }
        return heartbeat
    }

    static func readUpdate(from url: URL) -> EldrUpdateCache? {
        guard let data = try? Data(contentsOf: url) else { return nil }
        return try? JSONDecoder().decode(EldrUpdateCache.self, from: data)
    }
}

enum EldrFreshness: Equatable {
    case live
    case delayed
    case stopped
    case snapshotOnly
    case unavailable

    var label: String {
        switch self {
        case .live: return "Monitoring"
        case .delayed: return "Sample delayed"
        case .stopped: return "Guard is not active"
        case .snapshotOnly: return "One-time snapshot"
        case .unavailable: return "No data"
        }
    }

    var symbol: String {
        switch self {
        case .live: return "checkmark.circle.fill"
        case .delayed: return "clock.badge.exclamationmark"
        case .stopped: return "pause.circle.fill"
        case .snapshotOnly: return "doc.text"
        case .unavailable: return "questionmark.circle"
        }
    }

    var tint: Color {
        switch self {
        case .live: return .green
        case .delayed: return .orange
        case .stopped, .unavailable: return .red
        case .snapshotOnly: return .secondary
        }
    }
}

@MainActor
final class EldrStatusStore: ObservableObject {
    @Published private(set) var status: EldrStatus?
    @Published private(set) var heartbeat: EldrGuardHeartbeat?
    @Published private(set) var update: EldrUpdateCache?
    @Published private(set) var issue: String?
    @Published private(set) var refreshedAt = Date()

    let paths: EldrMenuPaths
    private var watcher: EldrDirectoryWatcher?
    private var timer: Timer?
    private var lastRecoveryRead = Date.distantPast
    private var refreshQueued = false

    init(paths: EldrMenuPaths = .current()) {
        self.paths = paths
        refresh()
        startWatching()
        timer = Timer.scheduledTimer(withTimeInterval: 15, repeats: true) { [weak self] _ in
            Task { @MainActor [weak self] in
                self?.tick()
            }
        }
    }

    deinit {
        timer?.invalidate()
    }

    func refresh() {
        startWatching()
        refreshedAt = Date()
        lastRecoveryRead = refreshedAt
        do {
            status = try EldrMenuReader.readStatus(from: paths.statusURL)
            issue = nil
        } catch let error as EldrMenuReadError {
            switch error {
            case .invalidStatus:
                // Atomic publication means this is normally transient. Keep the prior sample
                // visible, but make the failure explicit until the next valid replacement.
                issue = error.localizedDescription
            case .missingStatus, .unsupportedSchema:
                status = nil
                issue = error.localizedDescription
            }
        } catch {
            issue = error.localizedDescription
        }
        heartbeat = EldrMenuReader.readHeartbeat(from: paths.heartbeatURL)
        update = EldrMenuReader.readUpdate(from: paths.updateURL)
    }

    private func tick() {
        refreshedAt = Date()
        // File events drive normal reads. Once a minute, or while the data directory does
        // not exist, retry to recover from a missed event without a continuous file poll.
        if watcher == nil || refreshedAt.timeIntervalSince(lastRecoveryRead) >= 60 {
            refresh()
        }
    }

    var freshness: EldrFreshness {
        guard status != nil else { return .unavailable }
        guard heartbeatMatchesStatus else { return .snapshotOnly }
        return Self.freshness(heartbeat: heartbeat, statusTimestamp: statusTimestamp, now: Date())
    }

    private var heartbeatMatchesStatus: Bool {
        guard let expected = heartbeat?.statusSampleTimestamp,
              let actual = status?.timestamp
        else {
            return true
        }
        return expected == actual
    }

    var statusTimestamp: Date? {
        guard let value = status?.timestamp else { return nil }
        return Self.iso8601.date(from: value)
    }

    var ageLabel: String {
        let timestamp: Date?
        if heartbeatMatchesStatus, let heartbeatAt = heartbeat?.heartbeatAt {
            timestamp = Date(timeIntervalSince1970: TimeInterval(heartbeatAt))
        } else {
            timestamp = statusTimestamp
        }
        guard let timestamp else { return "No sample" }
        let age = max(0, Int(Date().timeIntervalSince(timestamp)))
        if age < 60 { return "\(age)s ago" }
        return "\(age / 60)m ago"
    }

    var menuTitle: String {
        guard let status else { return "Eldr, \(headline)" }
        let cpu = status.cpuUsage.map(Self.percentValue) ?? "unavailable"
        let free = status.memoryAvailable.map(Self.bytes) ?? "unavailable"
        return "Eldr, \(headline), CPU \(cpu), \(free) free memory"
    }

    var indicatorSymbol: String {
        guard freshness == .live else { return freshness.symbol }
        switch status?.level {
        case "ALERT": return "exclamationmark.triangle.fill"
        case "WARN": return "exclamationmark.circle.fill"
        default: return freshness.symbol
        }
    }

    var indicatorTint: Color {
        guard freshness == .live else { return freshness.tint }
        switch status?.level {
        case "ALERT": return .red
        case "WARN": return .orange
        default: return freshness.tint
        }
    }

    var headline: String {
        guard freshness == .live else { return freshness.label }
        switch status?.level {
        case "ALERT": return "Action needed"
        case "WARN": return "Attention needed"
        default: return freshness.label
        }
    }

    var updateAvailable: String? {
        guard let latest = update?.latest,
              EldrVersion(latest).isNewer(than: EldrVersion.current)
        else { return nil }
        return latest
    }

    func revealDataDirectory() {
        NSWorkspace.shared.open(paths.directory)
    }

    func copyUpdateCommand() {
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString("eldr update", forType: .string)
    }

    private func startWatching() {
        guard watcher == nil else { return }
        watcher = EldrDirectoryWatcher(directory: paths.directory) { [weak self] in
            Task { @MainActor [weak self] in
                self?.scheduleRefresh()
            }
        }
    }

    private func scheduleRefresh() {
        guard !refreshQueued else { return }
        refreshQueued = true
        Task { @MainActor [weak self] in
            try? await Task.sleep(nanoseconds: 150_000_000)
            guard let self else { return }
            self.refreshQueued = false
            self.refresh()
        }
    }

    private static let iso8601 = ISO8601DateFormatter()

    static func freshness(
        heartbeat: EldrGuardHeartbeat?,
        statusTimestamp: Date?,
        now: Date
    ) -> EldrFreshness {
        if let heartbeatAt = heartbeat?.heartbeatAt {
            let age = now.timeIntervalSince1970 - TimeInterval(heartbeatAt)
            let interval = TimeInterval(max(heartbeat?.intervalSeconds ?? 30, 1))
            let delayedAfter = max(50, interval * 5 / 3)
            let stoppedAfter = max(90, interval * 3)
            if age <= delayedAfter { return .live }
            if age <= stoppedAfter { return .delayed }
            return .stopped
        }
        if statusTimestamp != nil { return .snapshotOnly }
        return .unavailable
    }

    static func percent(_ fraction: Double) -> String {
        String(format: "%.0f%% CPU", fraction * 100)
    }

    static func percentValue(_ fraction: Double) -> String {
        String(format: "%.0f%%", fraction * 100)
    }

    nonisolated static func bytes(_ value: UInt64) -> String {
        ByteCountFormatter.string(fromByteCount: Int64(clamping: value), countStyle: .binary)
    }

    static func watts(_ value: Double) -> String {
        String(format: "%.1f W", value)
    }

    static func temperature(_ value: Double) -> String {
        String(format: "%.0f °C", value)
    }

    static func rate(_ value: Double) -> String {
        "\(bytes(UInt64(max(0, value))))/s"
    }
}

final class EldrDirectoryWatcher {
    private let source: DispatchSourceFileSystemObject

    init?(directory: URL, onChange: @escaping @Sendable () -> Void) {
        let descriptor = open(directory.path, O_EVTONLY)
        guard descriptor >= 0 else { return nil }
        let source = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: descriptor,
            eventMask: [.write, .rename, .delete, .extend, .attrib],
            queue: DispatchQueue.global(qos: .utility)
        )
        source.setEventHandler(handler: onChange)
        source.setCancelHandler {
            close(descriptor)
        }
        self.source = source
        source.resume()
    }

    deinit {
        source.cancel()
    }
}

struct EldrVersion: Comparable, Equatable {
    let components: [Int]

    init(_ value: String) {
        components = value
            .trimmingCharacters(in: CharacterSet(charactersIn: "v"))
            .split(separator: ".")
            .map { Int($0) ?? 0 }
    }

    static let current = EldrVersion(
        Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
    )

    static func < (lhs: EldrVersion, rhs: EldrVersion) -> Bool {
        let count = max(lhs.components.count, rhs.components.count)
        for index in 0..<count {
            let left = index < lhs.components.count ? lhs.components[index] : 0
            let right = index < rhs.components.count ? rhs.components[index] : 0
            if left != right { return left < right }
        }
        return false
    }

    func isNewer(than current: EldrVersion) -> Bool {
        current < self
    }
}

enum EldrBrand {
    static let menuMark = image(named: "eldr-menubar-template", fileExtension: "png", template: true)
    static let appIcon = image(named: "eldr", fileExtension: "icns")

    private static func image(named name: String, fileExtension: String, template: Bool = false) -> NSImage? {
        guard let url = Bundle.main.url(forResource: name, withExtension: fileExtension),
              let image = NSImage(contentsOf: url)
        else {
            return nil
        }
        image.isTemplate = template
        return image
    }
}

private struct EldrAppMark: View {
    let size: CGFloat

    var body: some View {
        Group {
            if let image = EldrBrand.appIcon {
                Image(nsImage: image)
                    .resizable()
                    .interpolation(.high)
            } else {
                Image(systemName: "flame.fill")
                    .resizable()
                    .scaledToFit()
                    .foregroundStyle(.orange)
                    .padding(size * 0.16)
            }
        }
        .frame(width: size, height: size)
        .clipShape(RoundedRectangle(cornerRadius: size * 0.22, style: .continuous))
    }
}

struct EldrMenuView: View {
    @ObservedObject var store: EldrStatusStore

    private let panelWidth: CGFloat = 456
    private let panelHeight: CGFloat = 660

    var body: some View {
        ZStack {
            EldrPalette.canvas.ignoresSafeArea()
            ScrollView(showsIndicators: false) {
                VStack(alignment: .leading, spacing: 12) {
                    header
                    if let issue = store.issue {
                        issueBanner(issue)
                    }
                    if let status = store.status {
                        focusCard(for: status)
                        metricStrip(for: status)
                        processConsumers(for: status)
                        diagnostics(for: status)
                    } else {
                        emptyState
                    }
                    if let version = store.updateAvailable {
                        updateBanner(version: version)
                    }
                    footer
                }
                .padding(14)
            }
        }
        .frame(width: panelWidth, height: panelHeight)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            EldrAppMark(size: 38)
                .shadow(color: EldrPalette.ember.opacity(0.22), radius: 8, y: 2)
            VStack(alignment: .leading, spacing: 2) {
                Text("ELDR")
                    .font(.system(size: 11, weight: .bold, design: .monospaced))
                    .tracking(1.7)
                Text(store.status?.chip ?? "Local hardware monitor")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            VStack(alignment: .trailing, spacing: 5) {
                EldrStatusPill(
                    title: store.headline,
                    symbol: store.indicatorSymbol,
                    tint: store.indicatorTint
                )
                Text("Updated \(store.ageLabel)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(store.menuTitle)
    }

    private func issueBanner(_ issue: String) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(EldrPalette.warning)
            Text(issue)
                .font(.caption)
                .foregroundStyle(.primary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(10)
        .background(EldrPalette.warning.opacity(0.11), in: RoundedRectangle(cornerRadius: 11, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 11, style: .continuous)
                .stroke(EldrPalette.warning.opacity(0.32), lineWidth: 1)
        }
    }

    private var emptyState: some View {
        EldrPanel {
            HStack(alignment: .top, spacing: 11) {
                Image(systemName: "waveform.path.ecg")
                    .font(.title3)
                    .foregroundStyle(EldrPalette.ember)
                VStack(alignment: .leading, spacing: 4) {
                    Text("Waiting for Eldr")
                        .font(.headline)
                    Text(store.issue ?? "The guard will populate this panel after its first sample.")
                        .font(.callout)
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func focusCard(for status: EldrStatus) -> some View {
        let culprit = primaryCulprit(for: status)
        return VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("RIGHT NOW")
                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                    .tracking(1.2)
                    .foregroundStyle(EldrPalette.ember)
                Spacer()
                Text(store.freshness == .live ? "LIVE SAMPLE" : "LAST SAMPLE")
                    .font(.system(size: 10, weight: .semibold, design: .monospaced))
                    .foregroundStyle(.secondary)
            }
            if let culprit {
                HStack(alignment: .center, spacing: 12) {
                    Image(systemName: culprit.symbol)
                        .font(.title2)
                        .foregroundStyle(culprit.tint)
                        .frame(width: 30)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(culprit.name)
                            .font(.system(.title3, design: .rounded).weight(.semibold))
                            .lineLimit(1)
                        Text(culprit.detail)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 8)
                    VStack(alignment: .trailing, spacing: 2) {
                        Text(culprit.value)
                            .font(.system(.title2, design: .rounded).weight(.bold))
                            .monospacedDigit()
                        Text(culprit.category.uppercased())
                            .font(.system(size: 10, weight: .bold, design: .monospaced))
                            .foregroundStyle(culprit.tint)
                    }
                }
            } else {
                HStack(alignment: .center, spacing: 12) {
                    Image(systemName: "checkmark.shield.fill")
                        .font(.title2)
                        .foregroundStyle(EldrPalette.good)
                        .frame(width: 30)
                    VStack(alignment: .leading, spacing: 3) {
                        Text("No active resource hog")
                            .font(.system(.title3, design: .rounded).weight(.semibold))
                        Text("The current sample is calm. Eldr will flag the next sustained pressure source here.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
        .padding(14)
        .background(
            LinearGradient(
                colors: [EldrPalette.ember.opacity(0.18), EldrPalette.panel],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            ),
            in: RoundedRectangle(cornerRadius: 16, style: .continuous)
        )
        .overlay {
            RoundedRectangle(cornerRadius: 16, style: .continuous)
                .stroke(EldrPalette.ember.opacity(0.32), lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
    }

    private func metricStrip(for status: EldrStatus) -> some View {
        HStack(spacing: 8) {
            EldrMetricTile(
                title: "CPU",
                value: status.cpuUsage.map(EldrStatusStore.percentValue) ?? "--",
                detail: status.cpuLoad.map { "Load \(EldrStatusStore.percentValue($0))" } ?? "No load reading",
                symbol: "cpu",
                tint: cpuTint(for: status),
                progress: status.cpuUsage.map(clamp)
            )
            EldrMetricTile(
                title: "MEMORY",
                value: status.memoryAvailable.map(EldrStatusStore.bytes) ?? "--",
                detail: "\(titleCase(status.memoryPressure, fallback: "Pressure unknown")) free",
                symbol: "memorychip",
                tint: memoryTint(for: status),
                progress: memoryProgress(for: status)
            )
            EldrMetricTile(
                title: "THERMAL",
                value: status.cpuTemperature.map(EldrStatusStore.temperature) ?? "--",
                detail: titleCase(status.thermal, fallback: "No thermal sensor"),
                symbol: "thermometer.medium",
                tint: thermalTint(for: status),
                progress: temperatureProgress(for: status)
            )
        }
    }

    private func processConsumers(for status: EldrStatus) -> some View {
        let cpu = Array((status.topProcesses ?? []).prefix(2))
        let memory = Array((status.topMemory ?? []).prefix(2))
        return Group {
            if !cpu.isEmpty || !memory.isEmpty {
                EldrPanel {
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(alignment: .firstTextBaseline) {
                            Text("RESOURCE CONSUMERS")
                                .font(.system(size: 10, weight: .bold, design: .monospaced))
                                .tracking(1.1)
                                .foregroundStyle(.secondary)
                            Spacer()
                            Text("Top live processes")
                                .font(.caption2)
                                .foregroundStyle(.tertiary)
                        }
                        if !cpu.isEmpty {
                            processGroup(title: "CPU", processes: cpu, metric: .cpu, tint: EldrPalette.ember)
                        }
                        if !cpu.isEmpty && !memory.isEmpty {
                            Divider()
                        }
                        if !memory.isEmpty {
                            processGroup(title: "MEMORY", processes: memory, metric: .memory, tint: EldrPalette.violet)
                        }
                    }
                }
            }
        }
    }

    private func processGroup(
        title: String,
        processes: [EldrProcess],
        metric: EldrProcessMetric,
        tint: Color
    ) -> some View {
        let maximum = max(processes.map { metric.value(for: $0) }.max() ?? 0, 1)
        return VStack(alignment: .leading, spacing: 7) {
            Text(title)
                .font(.system(size: 10, weight: .bold, design: .monospaced))
                .tracking(1.0)
                .foregroundStyle(tint)
            ForEach(processes) { process in
                EldrProcessRow(
                    process: process,
                    value: metric.display(for: process),
                    progress: metric.value(for: process) / maximum,
                    tint: tint
                )
            }
        }
    }

    private func diagnostics(for status: EldrStatus) -> some View {
        let items = diagnosticItems(for: status)
        return EldrPanel {
            VStack(alignment: .leading, spacing: 10) {
                Text("SYSTEM DETAIL")
                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                    .tracking(1.1)
                    .foregroundStyle(.secondary)
                Grid(horizontalSpacing: 8, verticalSpacing: 8) {
                    GridRow {
                        EldrDiagnosticCell(item: items[0])
                        EldrDiagnosticCell(item: items[1])
                    }
                    GridRow {
                        EldrDiagnosticCell(item: items[2])
                        EldrDiagnosticCell(item: items[3])
                    }
                    if items.count > 4 {
                        GridRow {
                            EldrDiagnosticCell(item: items[4])
                        }
                    }
                }
            }
        }
    }

    private func updateBanner(version: String) -> some View {
        HStack(spacing: 10) {
            Image(systemName: "arrow.down.circle.fill")
                .font(.title3)
                .foregroundStyle(EldrPalette.ember)
            VStack(alignment: .leading, spacing: 2) {
                Text("Eldr v\(version) is ready")
                    .font(.subheadline.weight(.semibold))
                Text("Copy the update command and run it when you are ready.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 6)
            Button(action: store.copyUpdateCommand) {
                Label("Copy", systemImage: "doc.on.doc")
                    .font(.caption.weight(.semibold))
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(11)
        .background(EldrPalette.ember.opacity(0.10), in: RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(EldrPalette.ember.opacity(0.28), lineWidth: 1)
        }
    }

    private var footer: some View {
        HStack(spacing: 5) {
            EldrFooterAction(title: "Refresh", symbol: "arrow.clockwise", action: store.refresh)
            EldrFooterAction(title: "Data", symbol: "folder", action: store.revealDataDirectory)
            Spacer()
            EldrFooterAction(
                title: "Quit",
                symbol: "power",
                tint: .secondary,
                action: { NSApplication.shared.terminate(nil) }
            )
        }
    }

    private func primaryCulprit(for status: EldrStatus) -> EldrCulprit? {
        if let process = status.topProcesses?.first, let cpu = process.cpu,
           cpu >= 25 || status.level == "WARN" || status.level == "ALERT" {
            return EldrCulprit(
                name: process.name,
                category: "CPU pressure",
                value: String(format: "%.0f%%", cpu),
                detail: "PID \(process.pid) is the strongest current CPU consumer.",
                symbol: "cpu",
                tint: cpuTint(for: status)
            )
        }
        let pressure = status.memoryPressure?.lowercased()
        if ["warn", "warning", "critical"].contains(pressure),
           let process = status.topMemory?.first, let memory = process.memory {
            return EldrCulprit(
                name: process.name,
                category: "Memory pressure",
                value: EldrStatusStore.bytes(memory),
                detail: "PID \(process.pid) is the largest resident process under memory pressure.",
                symbol: "memorychip",
                tint: memoryTint(for: status)
            )
        }
        return nil
    }

    private func diagnosticItems(for status: EldrStatus) -> [EldrDiagnosticItem] {
        var items = [
            EldrDiagnosticItem(
                title: "System power",
                value: status.systemPower.map(EldrStatusStore.watts) ?? "Unavailable",
                symbol: "bolt.fill",
                tint: EldrPalette.amber
            ),
            EldrDiagnosticItem(
                title: "Cooling",
                value: coolingLabel(status),
                symbol: "fanblades.fill",
                tint: thermalTint(for: status)
            ),
            EldrDiagnosticItem(
                title: "Storage free",
                value: storageLabel(status),
                symbol: "internaldrive",
                tint: EldrPalette.good
            ),
            EldrDiagnosticItem(
                title: "Network",
                value: networkLabel(status),
                symbol: "arrow.left.arrow.right",
                tint: EldrPalette.blue
            )
        ]
        if let battery = batteryLabel(status) {
            items.append(EldrDiagnosticItem(
                title: "Battery",
                value: battery,
                symbol: "battery.75percent",
                tint: EldrPalette.good
            ))
        }
        return items
    }

    private func cpuTint(for status: EldrStatus) -> Color {
        switch status.level {
        case "ALERT": return EldrPalette.danger
        case "WARN": return EldrPalette.warning
        default: return EldrPalette.ember
        }
    }

    private func memoryTint(for status: EldrStatus) -> Color {
        switch status.memoryPressure?.lowercased() {
        case "critical": return EldrPalette.danger
        case "warn", "warning", "elevated": return EldrPalette.warning
        default: return EldrPalette.violet
        }
    }

    private func thermalTint(for status: EldrStatus) -> Color {
        switch status.thermal?.lowercased() {
        case "critical", "serious": return EldrPalette.danger
        case "fair", "warm", "elevated": return EldrPalette.warning
        default: return EldrPalette.good
        }
    }

    private func memoryProgress(for status: EldrStatus) -> Double? {
        guard let total = status.memoryTotal, total > 0 else { return nil }
        if let used = status.memoryUsed {
            return clamp(Double(used) / Double(total))
        }
        if let available = status.memoryAvailable {
            return clamp(1 - Double(available) / Double(total))
        }
        return nil
    }

    private func temperatureProgress(for status: EldrStatus) -> Double? {
        guard let temperature = status.cpuTemperature else { return nil }
        return clamp((temperature - 25) / 75)
    }

    private func clamp(_ value: Double) -> Double {
        min(max(value, 0), 1)
    }

    private func titleCase(_ value: String?, fallback: String) -> String {
        guard let value, !value.isEmpty else { return fallback }
        return value.replacingOccurrences(of: "_", with: " ").capitalized
    }

    private func coolingLabel(_ status: EldrStatus) -> String {
        guard let rpm = status.fanRPM else { return "No fan reading" }
        if let target = status.fanTargetRPM { return "\(rpm) rpm / target \(target)" }
        return "\(rpm) rpm"
    }

    private func storageLabel(_ status: EldrStatus) -> String {
        guard let free = status.diskFree else { return "Unavailable" }
        return "\(EldrStatusStore.bytes(free)) free"
    }

    private func networkLabel(_ status: EldrStatus) -> String {
        guard let receive = status.networkReceiveRate, let transmit = status.networkTransmitRate else {
            return "No rate reading"
        }
        return "↓ \(EldrStatusStore.rate(receive))  ↑ \(EldrStatusStore.rate(transmit))"
    }

    private func batteryLabel(_ status: EldrStatus) -> String? {
        guard let percent = status.batteryPercent else { return nil }
        var label = "\(percent)%"
        if let state = status.batteryState { label += " / \(state)" }
        if let minutes = status.batteryTimeMinutes { label += " / \(minutes)m" }
        return label
    }
}

private enum EldrPalette {
    static let canvas = Color(nsColor: .windowBackgroundColor)
    static let panel = Color(nsColor: .controlBackgroundColor)
    static let inset = Color(nsColor: .underPageBackgroundColor)
    static let border = Color(nsColor: .separatorColor).opacity(0.48)
    static let ember = Color(red: 1.00, green: 0.42, blue: 0.10)
    static let amber = Color(red: 0.98, green: 0.68, blue: 0.25)
    static let good = Color(red: 0.33, green: 0.72, blue: 0.48)
    static let warning = Color(red: 0.96, green: 0.60, blue: 0.14)
    static let danger = Color(red: 0.94, green: 0.31, blue: 0.27)
    static let violet = Color(red: 0.60, green: 0.47, blue: 0.91)
    static let blue = Color(red: 0.32, green: 0.62, blue: 0.93)
}

private struct EldrCulprit {
    let name: String
    let category: String
    let value: String
    let detail: String
    let symbol: String
    let tint: Color
}

private enum EldrProcessMetric {
    case cpu
    case memory

    func value(for process: EldrProcess) -> Double {
        switch self {
        case .cpu:
            return process.cpu ?? 0
        case .memory:
            return Double(process.memory ?? 0)
        }
    }

    func display(for process: EldrProcess) -> String {
        switch self {
        case .cpu:
            guard let cpu = process.cpu else { return "--" }
            return String(format: "%.0f%%", cpu)
        case .memory:
            guard let memory = process.memory else { return "--" }
            return EldrStatusStore.bytes(memory)
        }
    }
}

private struct EldrPanel<Content: View>: View {
    let content: Content

    init(@ViewBuilder content: () -> Content) {
        self.content = content()
    }

    var body: some View {
        content
            .padding(12)
            .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            .overlay {
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .stroke(EldrPalette.border, lineWidth: 1)
            }
    }
}

private struct EldrStatusPill: View {
    let title: String
    let symbol: String
    let tint: Color

    var body: some View {
        Label(title.uppercased(), systemImage: symbol)
            .font(.system(size: 10, weight: .bold, design: .monospaced))
            .foregroundStyle(tint)
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(tint.opacity(0.12), in: Capsule())
            .overlay {
                Capsule().stroke(tint.opacity(0.28), lineWidth: 1)
            }
    }
}

private struct EldrMetricTile: View {
    let title: String
    let value: String
    let detail: String
    let symbol: String
    let tint: Color
    let progress: Double?

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 5) {
                Image(systemName: symbol)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(tint)
                Text(title)
                    .font(.system(size: 10, weight: .bold, design: .monospaced))
                    .tracking(0.8)
                    .foregroundStyle(.secondary)
            }
            Text(value)
                .font(.system(.title3, design: .rounded).weight(.bold))
                .monospacedDigit()
                .lineLimit(1)
                .minimumScaleFactor(0.72)
            Text(detail)
                .font(.caption2)
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .minimumScaleFactor(0.76)
            if let progress {
                ProgressView(value: progress)
                    .progressViewStyle(.linear)
                    .tint(tint)
                    .frame(height: 4)
            } else {
                Capsule()
                    .fill(EldrPalette.border)
                    .frame(height: 4)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(11)
        .background(EldrPalette.inset.opacity(0.74), in: RoundedRectangle(cornerRadius: 13, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 13, style: .continuous)
                .stroke(EldrPalette.border, lineWidth: 1)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(title), \(value). \(detail)")
    }
}

private struct EldrProcessRow: View {
    let process: EldrProcess
    let value: String
    let progress: Double
    let tint: Color

    var body: some View {
        HStack(spacing: 8) {
            Circle()
                .fill(tint)
                .frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: 1) {
                Text(process.name)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                Text("PID \(process.pid)")
                    .font(.caption2)
                    .foregroundStyle(.tertiary)
            }
            Spacer(minLength: 6)
            ProgressView(value: min(max(progress, 0), 1))
                .progressViewStyle(.linear)
                .tint(tint)
                .frame(width: 62)
            Text(value)
                .font(.caption.weight(.semibold))
                .monospacedDigit()
                .frame(minWidth: 46, alignment: .trailing)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(process.name), PID \(process.pid), \(value)")
    }
}

private struct EldrDiagnosticItem: Identifiable {
    let title: String
    let value: String
    let symbol: String
    let tint: Color

    var id: String { title }
}

private struct EldrDiagnosticCell: View {
    let item: EldrDiagnosticItem

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Image(systemName: item.symbol)
                .font(.caption.weight(.semibold))
                .foregroundStyle(item.tint)
                .frame(width: 13)
            VStack(alignment: .leading, spacing: 2) {
                Text(item.title.uppercased())
                    .font(.system(size: 9, weight: .bold, design: .monospaced))
                    .tracking(0.65)
                    .foregroundStyle(.secondary)
                Text(item.value)
                    .font(.caption.weight(.medium))
                    .lineLimit(1)
                    .minimumScaleFactor(0.72)
            }
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(9)
        .background(EldrPalette.inset.opacity(0.50), in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("\(item.title), \(item.value)")
    }
}

private struct EldrFooterAction: View {
    let title: String
    let symbol: String
    var tint: Color = .primary
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Label(title, systemImage: symbol)
                .font(.caption.weight(.medium))
                .foregroundStyle(tint)
                .padding(.horizontal, 8)
                .padding(.vertical, 6)
                .background(tint.opacity(0.08), in: Capsule())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(title)
    }
}

#if ELDR_MENU_TESTS
@main
struct EldrMenuTests {
    @MainActor
    static func main() {
        let fixture = """
        {"schema_version":"1","ts":"2026-07-09T21:18:37Z","source":"guard","level":"OK","cpu_usage_pct":0.25,"ram_available":4294967296,"swap_used":536870912,"fan_rpm":1200,"top_procs":[{"pid":8,"cpu":320.5,"name":"cmux"}],"top_mem":[{"pid":9,"mem":2147483648,"name":"Docker"}]}
        """
        let status = try! JSONDecoder().decode(EldrStatus.self, from: Data(fixture.utf8))
        precondition(status.cpuUsage == 0.25)
        precondition(status.topProcesses?.first?.cpu == 320.5)
        precondition(status.topMemory?.first?.memory == 2_147_483_648)
        precondition(EldrVersion("2.0.0").isNewer(than: EldrVersion("1.9.0")))
        precondition(!EldrVersion("1.9.0").isNewer(than: EldrVersion("2.0.0")))
        let heartbeat = """
        {"schema_version":"1","kind":"eldr.menubar","pid":42,"heartbeat_at":1760000000,"interval_seconds":30,"status_sample_ts":"2026-07-09T21:18:37Z","sequence":7}
        """
        let decodedHeartbeat = try! JSONDecoder().decode(EldrGuardHeartbeat.self, from: Data(heartbeat.utf8))
        precondition(decodedHeartbeat.pid == 42)
        precondition(decodedHeartbeat.intervalSeconds == 30)
        let slowHeartbeat = EldrGuardHeartbeat(
            schemaVersion: "1",
            kind: "eldr.menubar",
            pid: 42,
            heartbeatAt: 1_760_000_000,
            intervalSeconds: 120,
            statusSampleTimestamp: nil,
            sequence: 1
        )
        let now = Date(timeIntervalSince1970: 1_760_000_180)
        precondition(EldrStatusStore.freshness(heartbeat: slowHeartbeat, statusTimestamp: nil, now: now) == .live)
        precondition(EldrStatusStore.freshness(heartbeat: decodedHeartbeat, statusTimestamp: nil, now: now) == .stopped)

        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try! FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let configDirectory = directory.appendingPathComponent("configured-state")
        let configPath = directory.appendingPathComponent(".config/eldr/config.toml")
        try! FileManager.default.createDirectory(at: configPath.deletingLastPathComponent(), withIntermediateDirectories: true)
        try! Data("ELDR_DIR=\(configDirectory.path)\n".utf8).write(to: configPath)
        precondition(EldrMenuPaths.current(environment: [:], home: directory).directory.path == configDirectory.path)
        let launchDirectory = directory.appendingPathComponent("launch-agent-state")
        let launchPath = directory.appendingPathComponent("Library/LaunchAgents/com.petruarakiss.eldr.guard.plist")
        try! FileManager.default.createDirectory(at: launchPath.deletingLastPathComponent(), withIntermediateDirectories: true)
        let launchPlist: [String: Any] = ["EnvironmentVariables": ["ELDR_DIR": launchDirectory.path]]
        let launchData = try! PropertyListSerialization.data(fromPropertyList: launchPlist, format: .xml, options: 0)
        try! launchData.write(to: launchPath)
        precondition(EldrMenuPaths.current(environment: [:], home: directory).directory.path == launchDirectory.path)
        let statusURL = directory.appendingPathComponent("status.json")
        do {
            _ = try EldrMenuReader.readStatus(from: statusURL)
            preconditionFailure("missing status must fail")
        } catch let error as EldrMenuReadError {
            precondition(error == .missingStatus)
        } catch {
            preconditionFailure("unexpected missing-status error")
        }
        try! Data("not-json".utf8).write(to: statusURL)
        do {
            _ = try EldrMenuReader.readStatus(from: statusURL)
            preconditionFailure("invalid JSON must fail")
        } catch let error as EldrMenuReadError {
            precondition(error == .invalidStatus)
        } catch {
            preconditionFailure("unexpected invalid-JSON error")
        }
        try! Data("{\"schema_version\":\"2\"}".utf8).write(to: statusURL)
        do {
            _ = try EldrMenuReader.readStatus(from: statusURL)
            preconditionFailure("unsupported schema must fail")
        } catch let error as EldrMenuReadError {
            precondition(error == .unsupportedSchema("2"))
        } catch {
            preconditionFailure("unexpected schema error")
        }
        print("Eldr menu tests passed")
    }
}
#else
@MainActor
final class EldrAppDelegate: NSObject, NSApplicationDelegate {
    private let store = EldrStatusStore()
    private var statusItem: NSStatusItem?
    private var popover: NSPopover?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let item = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        guard let button = item.button else { return }

        let image: NSImage?
        if let menuMark = EldrBrand.menuMark?.copy() as? NSImage {
            image = menuMark
            image?.isTemplate = true
        } else {
            image = (EldrBrand.appIcon?.copy() as? NSImage) ?? NSImage(
                systemSymbolName: "flame.fill",
                accessibilityDescription: "Eldr"
            )
            image?.isTemplate = false
        }
        image?.size = NSSize(width: 18, height: 18)
        button.image = image
        button.imagePosition = .imageOnly
        button.imageScaling = .scaleProportionallyUpOrDown
        button.toolTip = "Eldr hardware monitor"
        button.setAccessibilityLabel("Eldr")
        button.setAccessibilityHelp("Open the Eldr hardware monitor")
        button.target = self
        button.action = #selector(togglePopover(_:))

        let panel = NSPopover()
        panel.behavior = .transient
        panel.animates = true
        panel.contentSize = NSSize(width: 456, height: 660)
        panel.contentViewController = NSHostingController(rootView: EldrMenuView(store: store))

        statusItem = item
        popover = panel
    }

    @objc private func togglePopover(_ sender: Any?) {
        guard let popover else { return }
        if popover.isShown {
            popover.performClose(sender)
        } else {
            showPopover()
        }
    }

    private func showPopover() {
        guard let button = statusItem?.button, let popover else { return }
        popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)
    }
}

@main
struct EldrMenuApp: App {
    @NSApplicationDelegateAdaptor(EldrAppDelegate.self) private var appDelegate

    var body: some Scene {
        Settings {
            EmptyView()
        }
    }
}
#endif
