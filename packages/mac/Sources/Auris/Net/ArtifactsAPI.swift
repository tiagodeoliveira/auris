// ArtifactsAPI.swift
// REST client for the server's `/artifacts` endpoints. Mirrors
// `packages/server/src/api.rs` shapes (see PLAN.md §3.7).
//
//   GET    /artifacts                           → [Artifact]
//   POST   /artifacts                           → multipart, returns Artifact
//   GET    /artifacts/:id                       → Artifact
//   DELETE /artifacts/:id                       → 204
//   POST   /meetings/:id/artifacts              → 204 (body: { artifact_id })
//   DELETE /meetings/:id/artifacts/:artifact_id → 204

import Foundation

/// Wire shape returned by GET /artifacts. `summaryStatus` drives
/// the row badge in the Mac UI (pending / done / failed). Only
/// `done` artifacts are attachable to meetings — the picker filters
/// by status.
struct Artifact: Decodable, Identifiable, Sendable, Hashable {
    let id: String
    let name: String
    let mimeType: String
    let shortSummary: String?
    let longSummary: String?
    let summaryStatus: String
    let sizeBytes: Int64
    let createdAt: Date

    enum CodingKeys: String, CodingKey {
        case id, name
        case mimeType = "mime_type"
        case shortSummary = "short_summary"
        case longSummary = "long_summary"
        case summaryStatus = "summary_status"
        case sizeBytes = "size_bytes"
        case createdAt = "created_at"
    }
}

struct ArtifactsAPI: Sendable {
    let baseURL: URL
    let token: String

    static func fromWSURL(_ wsURL: String, token: String) -> ArtifactsAPI? {
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
        return ArtifactsAPI(baseURL: base, token: token)
    }

    func list() async throws -> [Artifact] {
        try await fetch(path: "artifacts")
    }

    func get(id: String) async throws -> Artifact {
        try await fetch(path: "artifacts/\(id)")
    }

    /// Upload as multipart/form-data with a single `file` field.
    /// `mimeType` controls server-side validation — only the
    /// whitelisted set is accepted (text/markdown, application/pdf,
    /// image/png, etc.). Returns the freshly-inserted row with
    /// `summaryStatus: "pending"`; the async worker fills summaries
    /// and flips status shortly after.
    func upload(name: String, mimeType: String, data: Data) async throws -> Artifact {
        let url = baseURL.appendingPathComponent("artifacts")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let boundary = "----auris-\(UUID().uuidString)"
        req.setValue("multipart/form-data; boundary=\(boundary)", forHTTPHeaderField: "Content-Type")
        req.httpBody = multipartBody(boundary: boundary, name: name, mimeType: mimeType, data: data)

        let respData: Data
        let resp: URLResponse
        do {
            (respData, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        try expectOK(resp)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom(decodeRFC3339DateForArtifacts)
        do {
            return try decoder.decode(Artifact.self, from: respData)
        } catch {
            throw MeetingsAPIError.decode(error)
        }
    }

    func delete(id: String) async throws {
        let url = baseURL.appendingPathComponent("artifacts/\(id)")
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let resp: URLResponse
        do {
            (_, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        try expectOK(resp)
    }

    /// Attach a previously-uploaded artifact to a meeting. Server
    /// rejects with 400 if the artifact's `summary_status` isn't yet
    /// `done` — caller (UI picker) is expected to filter.
    func attach(meetingId: String, artifactId: String) async throws {
        let url = baseURL.appendingPathComponent("meetings/\(meetingId)/artifacts")
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body = try JSONSerialization.data(withJSONObject: ["artifact_id": artifactId])
        req.httpBody = body
        let resp: URLResponse
        do {
            (_, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        try expectOK(resp)
    }

    func detach(meetingId: String, artifactId: String) async throws {
        let url = baseURL.appendingPathComponent("meetings/\(meetingId)/artifacts/\(artifactId)")
        var req = URLRequest(url: url)
        req.httpMethod = "DELETE"
        req.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let resp: URLResponse
        do {
            (_, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw MeetingsAPIError.transport(error)
        }
        try expectOK(resp)
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
        try expectOK(resp)
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .custom(decodeRFC3339DateForArtifacts)
        do {
            return try decoder.decode(T.self, from: data)
        } catch {
            throw MeetingsAPIError.decode(error)
        }
    }

    private func expectOK(_ resp: URLResponse) throws {
        guard let http = resp as? HTTPURLResponse else {
            throw MeetingsAPIError.http(0)
        }
        switch http.statusCode {
        case 200..<300: return
        case 401: throw MeetingsAPIError.unauthorized
        case 404: throw MeetingsAPIError.notFound
        default: throw MeetingsAPIError.http(http.statusCode)
        }
    }
}

private func multipartBody(boundary: String, name: String, mimeType: String, data: Data) -> Data {
    var body = Data()
    body.append("--\(boundary)\r\n".data(using: .utf8)!)
    body.append(
        "Content-Disposition: form-data; name=\"file\"; filename=\"\(name)\"\r\n"
            .data(using: .utf8)!
    )
    body.append("Content-Type: \(mimeType)\r\n\r\n".data(using: .utf8)!)
    body.append(data)
    body.append("\r\n--\(boundary)--\r\n".data(using: .utf8)!)
    return body
}

/// Same RFC 3339 / ISO 8601 dual-shape decode as MeetingsAPI uses;
/// duplicated here so the two clients can move independently.
@Sendable
private func decodeRFC3339DateForArtifacts(decoder: Decoder) throws -> Date {
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
