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
    /// Moments captured during this meeting, oldest first. Empty
    /// when the meeting has none. Older server builds (before the
    /// moments-API commit) omit the field entirely; the optional
    /// decode keeps Mac builds compatible.
    let moments: [Moment]?

    enum CodingKeys: String, CodingKey {
        case id, description, metadata, transcript, moments
        case startedAt = "started_at"
        case endedAt = "ended_at"
    }
}

/// One captured moment. `screenshotURL` is server-relative
/// (`/meetings/:id/moments/:id/screenshot`); resolve against the
/// REST base when fetching.
struct Moment: Decodable, Identifiable, Sendable, Equatable {
    let id: String
    let kind: String
    let t: Int64
    let note: String?
    let summary: String?
    let summaryStatus: String
    let screenshotURL: String?
    let createdAt: Date

    enum CodingKeys: String, CodingKey {
        case id, kind, t, note, summary
        case summaryStatus = "summary_status"
        case screenshotURL = "screenshot_url"
        case createdAt = "created_at"
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

    /// `POST /meetings/:id/moments` — multipart with `t` (required),
    /// optional note text, optional PNG screenshot bytes. Returns
    /// the freshly-created moment (summary will be `pending` for a
    /// few seconds while the server-side worker fills it in).
    func createMoment(
        meetingId: String,
        t: Int64,
        note: String?,
        screenshotPNG: Data?
    ) async throws -> Moment {
        let url = baseURL.appendingPathComponent("meetings/\(meetingId)/moments")
        let boundary = "MCBoundary-\(UUID().uuidString)"
        var body = Data()

        // `t` field
        appendFormField(&body, boundary: boundary, name: "t", text: String(t))

        // Optional `note`
        if let note, !note.isEmpty {
            appendFormField(&body, boundary: boundary, name: "note", text: note)
        }

        // Optional `screenshot` file
        if let png = screenshotPNG, !png.isEmpty {
            appendFileField(
                &body,
                boundary: boundary,
                name: "screenshot",
                filename: "screenshot.png",
                contentType: "image/png",
                data: png
            )
        }

        // Closing boundary
        body.append("--\(boundary)--\r\n".data(using: .utf8)!)

        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue(
            "multipart/form-data; boundary=\(boundary)",
            forHTTPHeaderField: "Content-Type"
        )
        req.httpBody = body

        let data: Data
        let resp: URLResponse
        do {
            (data, resp) = try await URLSession.shared.upload(for: req, from: body)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        guard let http = resp as? HTTPURLResponse else {
            throw MeetingsAPIError.http(0)
        }
        switch http.statusCode {
        case 200..<300: break
        case 401: throw MeetingsAPIError.unauthorized
        case 404: throw MeetingsAPIError.notFound
        default: throw MeetingsAPIError.http(http.statusCode)
        }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom(decodeRFC3339Date)
        do {
            return try decoder.decode(Moment.self, from: data)
        } catch {
            throw MeetingsAPIError.decode(error)
        }
    }

    /// Build `<base>/meetings/:meeting_id/moments/:moment_id/screenshot`
    /// for a given relative `screenshot_url`. Used by the meeting
    /// detail view to render thumbnails (with a Bearer header).
    func screenshotURL(forRelativePath relative: String) -> URL? {
        // `relative` is a server-rooted path like
        // `/meetings/abc/moments/def/screenshot`. Strip the leading
        // slash and append onto the base.
        let trimmed = relative.hasPrefix("/") ? String(relative.dropFirst()) : relative
        return URL(string: trimmed, relativeTo: baseURL)?.absoluteURL
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

/// Append a `Content-Disposition: form-data; name=...` text part.
/// Used by `createMoment` to encode `t` and `note`.
private func appendFormField(
    _ body: inout Data,
    boundary: String,
    name: String,
    text: String
) {
    body.append("--\(boundary)\r\n".data(using: .utf8)!)
    body.append(
        "Content-Disposition: form-data; name=\"\(name)\"\r\n\r\n".data(using: .utf8)!
    )
    body.append(text.data(using: .utf8)!)
    body.append("\r\n".data(using: .utf8)!)
}

/// Append a `Content-Disposition: form-data; name=...; filename=...`
/// binary part. Used for the optional screenshot upload.
private func appendFileField(
    _ body: inout Data,
    boundary: String,
    name: String,
    filename: String,
    contentType: String,
    data: Data
) {
    body.append("--\(boundary)\r\n".data(using: .utf8)!)
    body.append(
        "Content-Disposition: form-data; name=\"\(name)\"; filename=\"\(filename)\"\r\n"
            .data(using: .utf8)!
    )
    body.append("Content-Type: \(contentType)\r\n\r\n".data(using: .utf8)!)
    body.append(data)
    body.append("\r\n".data(using: .utf8)!)
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
