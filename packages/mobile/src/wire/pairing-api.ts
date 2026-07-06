// REST client for the server's `/pair/*` endpoints.
//
// Mobile is the *issuer* side of the pairing flow — it lists paired
// devices and revokes them. Code mint happens over WS now via
// `Intent::MintPairCode` (the response arrives on the same connection
// as `Event::PairCodeMinted`); redeem + refresh stay on the PWA-side
// HTTP path.
//
// Shape mirrors meetings-api.ts / artifacts-api.ts: a class
// constructed with a base URL + a token provider, every request
// attaches the bearer header, errors map to a typed exception.

import { deriveApiBase } from "./meetings-api";

export interface PairedDevice {
  device_id: string;
  device_label: string;
  paired_at: string;
  last_seen_at: string;
}

export class PairingApiError extends Error {
  readonly status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
    this.name = "PairingApiError";
  }
}

export class PairingApi {
  constructor(
    private readonly baseUrl: string,
    private readonly tokenProvider: () => Promise<string>,
  ) {}

  static from(serverUrl: string, tokenProvider: () => Promise<string>): PairingApi | null {
    const base = deriveApiBase(serverUrl);
    if (!base) return null;
    return new PairingApi(base, tokenProvider);
  }

  /// List the user's currently-paired devices, newest first.
  /// Re-fetched by callers whenever the server fires
  /// `paired_devices_changed` (redeem or revoke from any surface).
  listDevices(): Promise<PairedDevice[]> {
    return this.getJson<PairedDevice[]>("/pair/devices");
  }

  /// Revoke a paired device. Server returns 204 on success, 404
  /// when the device doesn't exist OR belongs to someone else
  /// (intentionally indistinguishable).
  async revoke(deviceId: string): Promise<void> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + "/pair/revoke", {
        method: "POST",
        headers: {
          Authorization: `Bearer ${token}`,
          "Content-Type": "application/json",
        },
        body: JSON.stringify({ device_id: deviceId }),
      });
    } catch (e) {
      throw new PairingApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new PairingApiError(`Revoke failed (HTTP ${resp.status})`, resp.status);
    }
  }

  private async getJson<T>(path: string): Promise<T> {
    const token = await this.tokenProvider();
    let resp: Response;
    try {
      resp = await fetch(this.baseUrl + path, {
        headers: { Authorization: `Bearer ${token}` },
        cache: "no-store",
      });
    } catch (e) {
      throw new PairingApiError(e instanceof Error ? e.message : "Network error", 0);
    }
    if (!resp.ok) {
      throw new PairingApiError(`Server returned HTTP ${resp.status}.`, resp.status);
    }
    return (await resp.json()) as T;
  }
}
