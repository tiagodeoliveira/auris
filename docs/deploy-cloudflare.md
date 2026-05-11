# Deploying behind Cloudflare (TLS via Origin Certificate)

Public-internet deploy of `meeting-companion` fronted by Cloudflare,
**without** a Cloudflare Tunnel daemon. Cloudflare proxies traffic to
your VM over a port-forwarded public IP and re-encrypts to the origin
using a **Cloudflare Origin Certificate** that you serve via Caddy.

```
[client] ──TLS (CF Universal cert)──▶ [Cloudflare edge] ──TLS (Origin cert)──▶ [home router :443] ──port-fwd──▶ [VM Caddy:443] ──HTTP (docker net)──▶ [server:7331]
```

Why this shape:

- **Universal cert** at the edge → publicly trusted; no per-device
  trust dance for PWA / Mac / iOS / Android.
- **Origin cert** on the VM → trusted only by Cloudflare. Anyone who
  bypasses Cloudflare and hits the VM IP directly will get a TLS
  validation failure. That's intentional.
- **No tunnel** → no `cloudflared` daemon, no authentication leg, no
  extra moving part. Cost: you do need port 443 reachable from
  Cloudflare's IP ranges (locked down at the router + VM firewall).

This guide assumes the FQDN **`auris.tiago.tools`** as the running
example. Substitute your own everywhere.

---

## 1. Cloudflare dashboard

Pick the zone that owns your apex (e.g. `tiago.tools`).

1. **DNS → Records → Add record**
   - Type: `A`
   - Name: `auris`
   - IPv4: your home **public** IP (`curl ifconfig.me` from the VM)
   - Proxy status: **Proxied** (orange cloud)

2. **SSL/TLS → Overview**
   - Encryption mode: **Full (strict)**.
   - _Not_ "Flexible" (clear HTTP from CF to origin), _not_ plain
     "Full" (TLS to origin but cert not validated). Full (strict)
     validates the Origin Cert against CF's CA — that's what makes
     bypass-resistant.

3. **SSL/TLS → Origin Server → Create Certificate**
   - Private key type: RSA (2048).
   - Hostnames: `auris.tiago.tools` (or `*.tiago.tools` if you want
     reuse across subdomains).
   - Validity: 15 years (the max — fine for a hobby deploy).
   - Click **Create**. The certificate PEM and the private key PEM
     are shown **once**. Copy both into your password manager now.

4. **Network**
   - WebSockets: **On**. (Default on Free, but worth verifying — the
     entire app fails without it.)

5. **(Recommended) SSL/TLS → Edge Certificates → Always Use HTTPS**: On.

---

## 2. Drop the cert files on the VM

```bash
cd /path/to/meeting-companion
mkdir -p certs && chmod 700 certs
$EDITOR certs/cert.pem    # paste the Origin Certificate (the "Origin certificate" PEM block)
$EDITOR certs/key.pem     # paste the private key PEM
chmod 600 certs/cert.pem certs/key.pem
```

`certs/` is gitignored.

---

## 3. Wire up the Caddyfile + env

```bash
cp Caddyfile.example Caddyfile
```

Then in `.env.deploy`:

```bash
PUBLIC_DOMAIN=auris.tiago.tools
```

The shipped `Caddyfile.example` reads `$DOMAIN` from the environment
and bind-mounts the certs from `certs/`. No further edits needed for
the standard path — Caddy v2 forwards WebSocket `Upgrade` headers
transparently, no special directive required.

---

## 4. Home router port-forward

Forward **TCP :443** on the public WAN → VM's LAN IP **:443**.

Do **not** forward `:80`. Caddy here doesn't need it (we provide the
cert manually — no ACME HTTP challenge), so forwarding `:80` only
adds attack surface.

### 4a. Using a non-default port (`:443` already taken on the host)

If `:443` on the VM (or the WAN) is already in use by another app,
Caddy can listen on a different port. Pick from Cloudflare's HTTPS
origin-port allowlist (the only ports CF's proxy will connect to over
HTTPS):

> `443, 2053, 2083, 2087, 2096, 8443`

Reference: <https://developers.cloudflare.com/fundamentals/reference/network-ports/#network-ports-compatible-with-cloudflares-proxy>

`8443` is conventional. Two steps:

1. **In `.env.deploy`**, set:

   ```bash
   PUBLIC_PORT=8443
   ```

   The compose file now publishes `8443:8443` and Caddy binds the
   same port (the Caddyfile reads `{$PORT}`).

2. **In Cloudflare dashboard** → **Rules → Origin Rules → Create rule**:
   - Name: `meeting-companion origin port`
   - When incoming requests match → **Hostname equals** `auris.tiago.tools`
   - Then → **Override origin destination port** → `8443`
   - Deploy.

   This tells CF's proxy: keep terminating user TLS on the public
   `:443` like normal, but when forwarding the request to your
   origin, talk to `:8443` instead of `:443`. The browser still hits
   `https://auris.tiago.tools/` (no port in the URL).

3. **Port-forward** at the home router: WAN `:8443` → VM `:8443`.
   (Or, if your router supports port translation, WAN `:443` →
   VM `:8443` works too — but then the router rewrites the
   destination port and the WAN side stays on `:443`.)

4. **Firewall**: the rule from §5 below stays the same — just allow
   CF IPs on `:8443` instead of `:443`.

---

## 5. Lock the VM firewall to Cloudflare's IPs

If anything else can reach `https://<your-public-ip>:443`, it
bypasses Cloudflare. The Origin Cert mismatch will block normal
browsers (they don't trust CF's internal CA), but a script can still
poke the port. Add a network-level allowlist:

Cloudflare's published ranges:

- IPv4: <https://www.cloudflare.com/ips-v4>
- IPv6: <https://www.cloudflare.com/ips-v6>

`ufw` example (Ubuntu/Debian):

```bash
# Default-deny on 443 first.
sudo ufw deny 443/tcp

# Allow only Cloudflare IPv4 ranges.
for ip in $(curl -s https://www.cloudflare.com/ips-v4); do
  sudo ufw allow from "$ip" to any port 443 proto tcp
done

# Repeat for IPv6 if your router forwards IPv6.
for ip in $(curl -s https://www.cloudflare.com/ips-v6); do
  sudo ufw allow from "$ip" to any port 443 proto tcp
done

sudo ufw status numbered
```

CF's IP list changes once in a while — a monthly cron that
rebuilds these rules keeps you current.

---

## 6. Auth0 origin allowlist

Update the **PWA application** in Auth0 (Dashboard → Applications →
your PWA app → Settings):

- **Allowed Callback URLs**: add `https://<host-serving-pwa>/`
- **Allowed Web Origins**: add the same
- **Allowed Logout URLs**: add the same

The Mac and Mobile native apps don't use web origins — their auth
flow uses native loopback / app-link redirects, so no Auth0-side
change is needed for them. Only the WS server URL changed, which is
not an Auth0 concern.

---

## 7. Rebuild & point the clients at `wss://`

The WS URL is **build-time** for PWA and Mobile, **runtime** for Mac.

**PWA** — pass at build time:

```bash
cd packages/pwa
VITE_SERVER_URL=wss://auris.tiago.tools npm run build
# upload dist/ to your static host
```

**Mobile (Expo)** — set in your build env:

```bash
EXPO_PUBLIC_SERVER_URL=wss://auris.tiago.tools eas build
```

**Mac** — open the app → Settings → Server URL → `wss://auris.tiago.tools`. No rebuild.

---

## 8. Boot

```bash
cd /path/to/meeting-companion
docker compose -f docker-compose.deploy.yml --env-file .env.deploy pull
docker compose -f docker-compose.deploy.yml --env-file .env.deploy up -d
```

Verify the three containers:

```bash
docker compose -f docker-compose.deploy.yml ps
# Expect: meeting-companion-postgres, meeting-companion-server, meeting-companion-caddy
```

Tail the relevant logs:

```bash
docker compose -f docker-compose.deploy.yml logs -f caddy server
```

You should see Caddy boot with something like
`serving initial configuration` and `auris.tiago.tools` in its
listeners list, plus the server's normal startup line.

---

## 9. Smoke test

From your laptop (**not** the VM):

```bash
curl -i https://auris.tiago.tools/
# Expect a 2xx/4xx response — the exact body doesn't matter; the
# point is that the TLS handshake succeeded and Cloudflare proxied
# the request to your origin.
```

Then load the PWA in a browser and start a meeting. Open dev tools →
Network → filter on "ws". You should see the WebSocket upgrade
return `101 Switching Protocols` and stay open.

---

## Troubleshooting

| Symptom                                                                    | Likely cause / fix                                                                                                                                                                                                      |
| -------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `curl` returns `ERR_SSL_PROTOCOL_ERROR` or `522`                           | SSL/TLS mode on Cloudflare is "Flexible" — switch to **Full (strict)**.                                                                                                                                                 |
| `525` from Cloudflare                                                      | TLS handshake to origin failed. Either Caddy isn't running (`docker compose logs caddy`), the cert hostname doesn't match `$DOMAIN`, or the cert PEM is malformed.                                                      |
| `526` from Cloudflare                                                      | Origin cert didn't validate against CF's CA. Most often: you pasted a Let's Encrypt cert into `certs/cert.pem` by mistake — must be the **Origin Certificate** from CF's dashboard.                                     |
| `522` from Cloudflare                                                      | TCP timeout to origin. Port-forward isn't working, or VM firewall blocks CF's IPs. From a third-party VM (e.g. another cloud), try `nc -vz <your-public-ip> 443` — if it hangs, the path is broken upstream of your VM. |
| WebSocket connects then drops every ~100 s                                 | Cloudflare Free's idle timeout on WS. The server already sends heartbeats every `MEETING_COMPANION_HEARTBEAT_MS=10000` ms — verify it's set and that the client is also receiving / replying.                           |
| Browser DevTools shows the WS request as `(failed)` with no further detail | Auth0 token rejected. Check the server logs for `401`. The PWA bundle may have been built against the wrong `VITE_AUTH0_API_AUDIENCE` for this server — rebuild with the right one.                                     |
| `docker compose up` fails with `Set PUBLIC_DOMAIN in .env.deploy`          | The `caddy` service has a hard-required env var. Add `PUBLIC_DOMAIN=auris.tiago.tools` to `.env.deploy`.                                                                                                                |
| `https://<vm-public-ip>` (no proxy, raw IP) loads                          | Your firewall isn't blocking non-CF IPs. Re-check step 5. The Origin Cert will fail browser validation, but the port shouldn't even be reachable from arbitrary clients.                                                |

---

## Operational notes

- **WS auth tokens travel in the upgrade URL** (`?token=…`). Cloudflare
  logs request URLs in its analytics. Auth0 access tokens are
  short-lived and the server re-validates per connection, so the
  practical blast radius is small — but it's not zero. The Auth0
  user allowlist further bounds it.
- **Cert rotation**: CF Origin Certs default to 15-year validity, so
  rotation is rare. When you do rotate: regenerate via the CF
  dashboard, replace `certs/cert.pem` and `certs/key.pem` on the VM,
  `docker compose -f docker-compose.deploy.yml restart caddy`.
- **The Origin Cert is meant to _only_ be valid via Cloudflare.** If
  you ever flip the DNS record from "Proxied" to "DNS only" (grey
  cloud), browsers will reject the TLS handshake. That's the
  feature, not a bug — you have to choose between proxied + Origin
  Cert, or unproxied + a publicly-trusted cert (e.g. Let's Encrypt).
