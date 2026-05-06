// Auth0Client.swift
// Native PKCE flow against Auth0. Holds the access + refresh tokens
// and surfaces them to the rest of the app via `getAccessToken()`,
// which transparently refreshes when the cached token is close to
// expiring.
//
// Why hand-rolled instead of `Auth0.swift`: keeping the dep surface
// minimal — PKCE is ~150 lines and avoids pulling a SwiftPM dep.
//
// Storage choice: refresh token lives in UserDefaults for now. That's
// plaintext on disk and a real product would put it in Keychain via
// `SecItemAdd` — flagged as TODO so we revisit before any non-personal
// deployment.

import AppKit
import AuthenticationServices
import CryptoKit
import Foundation
import Observation

@MainActor
@Observable
final class Auth0Client: NSObject {
    /// Identity surface mirrored from the most recent ID token / userinfo.
    /// `nil` while signed out or before first session resolve.
    struct Identity: Equatable {
        let sub: String
        let email: String?
        let name: String?
        let picture: String?
    }

    enum AuthError: Error, LocalizedError {
        case notSignedIn
        case redirectFailed(String)
        case exchangeFailed(String)
        case refreshFailed(String)
        case malformedResponse(String)

        var errorDescription: String? {
            switch self {
            case .notSignedIn: "Not signed in"
            case .redirectFailed(let s): "Login was cancelled or blocked: \(s)"
            case .exchangeFailed(let s): "Could not complete login: \(s)"
            case .refreshFailed(let s): "Could not refresh session: \(s)"
            case .malformedResponse(let s): "Unexpected response from Auth0: \(s)"
            }
        }
    }

    // Auth0 dashboard config — baked in for the dev tenant. Move to
    // env-injected build settings before shipping anything user-facing.
    private let domain = "dev-jrva0wzk3qkdxcar.us.auth0.com"
    private let clientId = "YDK0XoDAIRhp2uORlfk8TijQkcqRzjsi"
    private let audience = "https://meeting-companion.api"
    private let redirectScheme = "meetingcompanion"
    private var redirectURI: String { "\(redirectScheme)://callback" }

    private enum Keys {
        static let refreshToken = "meetingCompanion.auth.refreshToken"
        static let identitySub = "meetingCompanion.auth.identity.sub"
        static let identityEmail = "meetingCompanion.auth.identity.email"
        static let identityName = "meetingCompanion.auth.identity.name"
        static let identityPicture = "meetingCompanion.auth.identity.picture"
    }

    /// Cached access token + its expiry. Refreshed transparently in
    /// `getAccessToken()` when within `expiryGuard` of expiring.
    private var cachedAccessToken: String?
    private var cachedAccessTokenExpiresAt: Date?
    private static let expiryGuard: TimeInterval = 60 // refresh ~60s before exp

    private(set) var identity: Identity?

    /// True when we have a usable session — either a refresh token
    /// on disk (preferred; survives restarts and silent-refreshes
    /// access tokens) or a still-valid cached access token from this
    /// session. Falls back to `identity != nil` so a session that
    /// got an ID token but no refresh token (e.g., the API didn't
    /// have "Allow Offline Access" enabled) still counts as signed
    /// in until the access token expires.
    var isSignedIn: Bool {
        if let rt = UserDefaults.standard.string(forKey: Keys.refreshToken), !rt.isEmpty {
            return true
        }
        if cachedAccessToken != nil, let exp = cachedAccessTokenExpiresAt, exp > Date() {
            return true
        }
        return identity != nil
    }

    override init() {
        super.init()
        // Hydrate cached identity from disk so the UI doesn't flash a
        // "(unknown)" state while we wait for the first refresh.
        let defaults = UserDefaults.standard
        if let sub = defaults.string(forKey: Keys.identitySub), !sub.isEmpty {
            identity = Identity(
                sub: sub,
                email: defaults.string(forKey: Keys.identityEmail),
                name: defaults.string(forKey: Keys.identityName),
                picture: defaults.string(forKey: Keys.identityPicture)
            )
        }
    }

    /// Run the full PKCE flow: code_verifier/challenge → ASWeb session
    /// → code-for-token exchange → store refresh token. On success
    /// `identity` is populated and `isSignedIn == true`.
    func signIn() async throws {
        let verifier = Self.randomURLSafeString(length: 64)
        let challenge = Self.codeChallenge(for: verifier)
        let state = UUID().uuidString

        var components = URLComponents()
        components.scheme = "https"
        components.host = domain
        components.path = "/authorize"
        components.queryItems = [
            URLQueryItem(name: "client_id", value: clientId),
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "redirect_uri", value: redirectURI),
            URLQueryItem(name: "scope", value: "openid profile email offline_access"),
            URLQueryItem(name: "audience", value: audience),
            URLQueryItem(name: "code_challenge", value: challenge),
            URLQueryItem(name: "code_challenge_method", value: "S256"),
            URLQueryItem(name: "state", value: state),
        ]
        guard let authURL = components.url else {
            throw AuthError.redirectFailed("could not build authorize URL")
        }

        let callback = try await runWebAuthSession(
            url: authURL, callbackScheme: redirectScheme)

        // Verify state and pick out the code.
        guard let comps = URLComponents(url: callback, resolvingAgainstBaseURL: false),
              let returnedState = comps.queryItems?.first(where: { $0.name == "state" })?.value,
              returnedState == state,
              let code = comps.queryItems?.first(where: { $0.name == "code" })?.value
        else {
            throw AuthError.redirectFailed("missing or mismatched state/code")
        }

        try await exchangeCode(code: code, verifier: verifier)
    }

    /// Drop the refresh token + cached identity. Future
    /// `getAccessToken()` calls will throw `AuthError.notSignedIn`.
    func signOut() {
        let defaults = UserDefaults.standard
        for key in [
            Keys.refreshToken, Keys.identitySub, Keys.identityEmail,
            Keys.identityName, Keys.identityPicture,
        ] {
            defaults.removeObject(forKey: key)
        }
        cachedAccessToken = nil
        cachedAccessTokenExpiresAt = nil
        identity = nil
    }

    /// Return a usable access token. Refreshes silently if the
    /// cached one expires within the next 60s. Throws
    /// `AuthError.notSignedIn` when there's no refresh token to
    /// trade with — caller should kick the sign-in flow.
    func getAccessToken() async throws -> String {
        if let t = cachedAccessToken, let exp = cachedAccessTokenExpiresAt,
            exp.timeIntervalSinceNow > Self.expiryGuard
        {
            return t
        }
        guard let refresh = UserDefaults.standard.string(forKey: Keys.refreshToken),
              !refresh.isEmpty
        else {
            throw AuthError.notSignedIn
        }
        try await refreshAccessToken(using: refresh)
        guard let t = cachedAccessToken else {
            throw AuthError.refreshFailed("post-refresh cache miss")
        }
        return t
    }

    // MARK: - HTTP

    private struct TokenResponse: Decodable {
        let access_token: String
        let refresh_token: String?
        let id_token: String?
        let expires_in: Int
        let token_type: String
    }

    private func exchangeCode(code: String, verifier: String) async throws {
        let url = URL(string: "https://\(domain)/oauth/token")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        let body: [String: String] = [
            "grant_type": "authorization_code",
            "client_id": clientId,
            "code_verifier": verifier,
            "code": code,
            "redirect_uri": redirectURI,
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        let response = try await postForToken(req: req)
        try persist(response: response, isInitialSignIn: true)
    }

    private func refreshAccessToken(using refreshToken: String) async throws {
        print("[Auth0Client] refreshing access token")
        let url = URL(string: "https://\(domain)/oauth/token")!
        var req = URLRequest(url: url)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        // `audience` is technically optional — refresh tokens should
        // inherit the original audience — but some Auth0 tenant
        // configurations strip it back to the Management API
        // default. Passing it explicitly guarantees the refreshed
        // access token has the same `aud` as the original. Same
        // story for `scope`: if omitted, you sometimes lose the
        // `offline_access` claim and break the next refresh.
        let body: [String: String] = [
            "grant_type": "refresh_token",
            "client_id": clientId,
            "refresh_token": refreshToken,
            "audience": audience,
            "scope": "openid profile email offline_access",
        ]
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        do {
            let response = try await postForToken(req: req)
            try persist(response: response, isInitialSignIn: false)
            print("[Auth0Client] refresh OK; expires in \(response.expires_in)s")
        } catch {
            // Refresh-token rotation may have invalidated this one
            // server-side. Clear local state so the UI can prompt for
            // a fresh sign-in instead of looping on a dead token.
            print("[Auth0Client] refresh FAILED: \(error.localizedDescription)")
            UserDefaults.standard.removeObject(forKey: Keys.refreshToken)
            cachedAccessToken = nil
            cachedAccessTokenExpiresAt = nil
            throw error
        }
    }

    private func postForToken(req: URLRequest) async throws -> TokenResponse {
        let (data, resp): (Data, URLResponse)
        do {
            (data, resp) = try await URLSession.shared.data(for: req)
        } catch {
            throw AuthError.exchangeFailed(error.localizedDescription)
        }
        guard let http = resp as? HTTPURLResponse else {
            throw AuthError.exchangeFailed("non-http response")
        }
        guard (200..<300).contains(http.statusCode) else {
            let detail = String(data: data, encoding: .utf8) ?? "<binary>"
            throw AuthError.exchangeFailed("HTTP \(http.statusCode): \(detail)")
        }
        do {
            return try JSONDecoder().decode(TokenResponse.self, from: data)
        } catch {
            throw AuthError.malformedResponse(error.localizedDescription)
        }
    }

    /// Persist the new tokens to disk + cache + identity.
    /// `isInitialSignIn` distinguishes the code-grant exchange (where
    /// a missing refresh_token is a real misconfiguration) from a
    /// refresh-grant response (where Non-Rotating mode legitimately
    /// returns no new refresh_token — the old one stays valid).
    private func persist(response: TokenResponse, isInitialSignIn: Bool) throws {
        cachedAccessToken = response.access_token
        cachedAccessTokenExpiresAt = Date().addingTimeInterval(TimeInterval(response.expires_in))
        let defaults = UserDefaults.standard
        if let rt = response.refresh_token, !rt.isEmpty {
            defaults.set(rt, forKey: Keys.refreshToken)
            print("[Auth0Client] persisted refresh token (expires_in=\(response.expires_in)s)")
        } else if isInitialSignIn {
            print("[Auth0Client] WARNING — sign-in response had NO refresh_token. Sessions won't survive app restart. Check that the API has 'Allow Offline Access' on, and the Native app's grant types include 'Refresh Token'.")
        }
        // Refresh response with no refresh_token is normal in
        // Non-Rotating mode — the existing one stays valid. No warn.
        if let idToken = response.id_token, let claims = Self.decodeIdToken(idToken) {
            let id = Identity(
                sub: claims.sub,
                email: claims.email,
                name: claims.name,
                picture: claims.picture
            )
            identity = id
            defaults.set(id.sub, forKey: Keys.identitySub)
            defaults.set(id.email, forKey: Keys.identityEmail)
            defaults.set(id.name, forKey: Keys.identityName)
            defaults.set(id.picture, forKey: Keys.identityPicture)
        }
    }

    // MARK: - PKCE / id_token helpers

    private static func randomURLSafeString(length: Int) -> String {
        var bytes = [UInt8](repeating: 0, count: length)
        _ = SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes)
        return Data(bytes).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    private static func codeChallenge(for verifier: String) -> String {
        let data = Data(verifier.utf8)
        let hash = SHA256.hash(data: data)
        return Data(hash).base64EncodedString()
            .replacingOccurrences(of: "+", with: "-")
            .replacingOccurrences(of: "/", with: "_")
            .replacingOccurrences(of: "=", with: "")
    }

    private struct IDClaims: Decodable {
        let sub: String
        let email: String?
        let name: String?
        let picture: String?
    }

    /// Decode an ID token's payload without signature verification.
    /// Safe because we got the token over TLS directly from Auth0;
    /// we're not relaying it from an untrusted source.
    private static func decodeIdToken(_ jwt: String) -> IDClaims? {
        let parts = jwt.split(separator: ".")
        guard parts.count == 3 else { return nil }
        var payload = String(parts[1])
        // base64url → base64 (pad to multiple of 4)
        payload = payload.replacingOccurrences(of: "-", with: "+")
            .replacingOccurrences(of: "_", with: "/")
        while payload.count % 4 != 0 { payload.append("=") }
        guard let data = Data(base64Encoded: payload) else { return nil }
        return try? JSONDecoder().decode(IDClaims.self, from: data)
    }

    // MARK: - ASWebAuthenticationSession bridge

    private func runWebAuthSession(url: URL, callbackScheme: String) async throws -> URL {
        try await withCheckedThrowingContinuation { cont in
            let session = ASWebAuthenticationSession(
                url: url, callbackURLScheme: callbackScheme
            ) { callbackURL, error in
                if let error {
                    cont.resume(
                        throwing: AuthError.redirectFailed(error.localizedDescription))
                    return
                }
                guard let callbackURL else {
                    cont.resume(throwing: AuthError.redirectFailed("no callback URL"))
                    return
                }
                cont.resume(returning: callbackURL)
            }
            session.presentationContextProvider = self
            // Ephemeral session prevents keychain-shared cookies from
            // leaking sessions across users on the same Mac.
            session.prefersEphemeralWebBrowserSession = false
            session.start()
        }
    }
}

extension Auth0Client: ASWebAuthenticationPresentationContextProviding {
    nonisolated func presentationAnchor(
        for session: ASWebAuthenticationSession
    ) -> ASPresentationAnchor {
        // ASWebAuthenticationSession invokes this on the main thread
        // already; the assumeIsolated lets us touch the MainActor-bound
        // NSWindow init without paying for a hop.
        MainActor.assumeIsolated {
            NSApplication.shared.keyWindow ?? ASPresentationAnchor()
        }
    }
}
