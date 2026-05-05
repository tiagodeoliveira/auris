// MeetingsAPI.swift
// Thin REST client for the server's `/meetings` endpoints. Mirrors
// `packages/server/src/api.rs` shapes. Two endpoints:
//
//   GET /meetings        → [MeetingSummary]   (newest first)
//   GET /meetings/:id    → MeetingDetail      (summary + inlined transcript)
//
// Auth: `Authorization: Bearer <token>` — same token the WS uses.
// Base URL: derived from the WS settings URL by replacing the scheme
// (ws → http, wss → https) and bumping the port by +1, matching the
// server's `--api-port` default. If we ever support a non-default
// API port, the server can advertise its api endpoint in `Snapshot`
// and clients can read it from there; for now, the convention holds.

import Foundation

struct MeetingSummary: Decodable, Identifiable, Sendable, Hashable {
    let id: String
    let description: String?
    let metadata: [String: String]
    let startedAt: Date
    let endedAt: Date?

    enum CodingKeys: String, CodingKey {
        case id, description, metadata
        case startedAt = "started_at"
        case endedAt = "ended_at"
    }
}

struct MeetingDetail: Decodable, Identifiable, Sendable {
    let id: String
    let description: String?
    let metadata: [String: String]
    let startedAt: Date
    let endedAt: Date?
    let transcript: [Item]

    enum CodingKeys: String, CodingKey {
        case id, description, metadata, transcript
        case startedAt = "started_at"
        case endedAt = "ended_at"
    }
}

enum MeetingsAPIError: Error, LocalizedError {
    case invalidServerURL
    case unauthorized
    case notFound
    case http(Int)
    case decode(Error)
    case transport(Error)

    var errorDescription: String? {
        switch self {
        case .invalidServerURL:
            return "Server URL is invalid or has no explicit port. Check Settings."
        case .unauthorized:
            return "Server rejected the token (HTTP 401). Check Settings."
        case .notFound:
            return "Meeting not found (HTTP 404)."
        case .http(let code):
            return "Server returned HTTP \(code)."
        case .decode(let e):
            return "Failed to decode response: \(e.localizedDescription)"
        case .transport(let e):
            return e.localizedDescription
        }
    }
}

struct MeetingsAPI: Sendable {
    let baseURL: URL
    let token: String

    /// Build a client from the WS server URL. WS and REST share a
    /// single port now (axum routes both); we just upgrade the
    /// scheme and strip the path/query.
    static func fromWSURL(_ wsURL: String, token: String) -> MeetingsAPI? {
        guard let parsed = URL(string: wsURL),
            var components = URLComponents(url: parsed, resolvingAgainstBaseURL: false)
        else { return nil }
        switch components.scheme?.lowercased() {
        case "ws": components.scheme = "http"
        case "wss": components.scheme = "https"
        default: return nil
        }
        components.path = ""
        components.query = nil
        guard let base = components.url else { return nil }
        return MeetingsAPI(baseURL: base, token: token)
    }

    func list() async throws -> [MeetingSummary] {
        try await fetch(path: "meetings")
    }

    func detail(id: String) async throws -> MeetingDetail {
        try await fetch(path: "meetings/\(id)")
    }

    // MARK: - Internals

    private func fetch<T: Decodable>(path: String) async throws -> T {
        let url = baseURL.appendingPathComponent(path)
        var req = URLRequest(url: url)
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.cachePolicy = .reloadIgnoringLocalCacheData

        let data: Data
        let resp: URLResponse
        do {
            (data, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        let http = resp as? HTTPURLResponse
        switch http?.statusCode {
        case .some(200..<300): break
        case .some(401): throw MeetingsAPIError.unauthorized
        case .some(404): throw MeetingsAPIError.notFound
        case .some(let code): throw MeetingsAPIError.http(code)
        case .none: throw MeetingsAPIError.http(0)
        }

        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom(decodeRFC3339Date)
        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            throw MeetingsAPIError.decode(error)
        }
    }
}

/// Decode RFC 3339 / ISO 8601 timestamps with *or without* fractional
/// seconds. The server emits both shapes — `chrono::DateTime<Utc>` ->
/// JSON includes microseconds, but ad-hoc inserts (e.g. via sqlite
/// shell) typically don't. Swift's stock `.iso8601` strategy only
/// accepts the no-fraction form, so we roll our own.
@Sendable
private func decodeRFC3339Date(decoder: Decoder) throws -> Date {
    let str = try decoder.singleValueContainer().decode(String.self)
    let withFractional = ISO8601DateFormatter()
    withFractional.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
    if let d = withFractional.date(from: str) { return d }
    let plain = ISO8601DateFormatter()
    plain.formatOptions = [.withInternetDateTime]
    if let d = plain.date(from: str) { return d }
    throw DecodingError.dataCorruptedError(
        in: try decoder.singleValueContainer(),
        debugDescription: "Invalid RFC 3339 / ISO 8601 timestamp: \(str)"
    )
}
