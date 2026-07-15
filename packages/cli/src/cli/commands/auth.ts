import {
  clearCredentials,
  decodeToken,
  defaultCredPath,
  deviceLogin,
  loadCredentials,
} from "../../core/auth.js";
import { requireAuth0, resolveConfig } from "../../core/config.js";

export async function loginCommand(credPath: string = defaultCredPath()): Promise<void> {
  const { auth0 } = resolveConfig();
  await deviceLogin(requireAuth0(auth0), credPath);
}

export async function logoutCommand(credPath: string = defaultCredPath()): Promise<void> {
  await clearCredentials(credPath);
  console.log("Logged out.");
}

export async function whoamiCommand(credPath: string = defaultCredPath()): Promise<string> {
  const cred = await loadCredentials(credPath);
  if (!cred) return "not logged in — run `auris login`.";
  const { sub, email, exp } = decodeToken(cred.access_token);
  const when = exp ? new Date(exp * 1000).toISOString() : "unknown";
  return `sub:    ${sub ?? "?"}${email ? `\nemail:  ${email}` : ""}\nexpires: ${when}`;
}
