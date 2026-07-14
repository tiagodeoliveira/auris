export interface Config {
  baseUrl: string;
  token: string;
}

const DEFAULT_BASE_URL = "https://auris.tiago.tools";

/**
 * Read configuration from the environment. `AURIS_MCP_TOKEN` (the auris bearer
 * token) is required; `AURIS_BASE_URL` is optional and defaults to production.
 */
export function loadConfig(env: NodeJS.ProcessEnv = process.env): Config {
  const token = env.AURIS_MCP_TOKEN?.trim();
  if (!token) {
    throw new Error(
      "AURIS_MCP_TOKEN is not set. Provide your auris bearer token via the AURIS_MCP_TOKEN environment variable.",
    );
  }
  const baseUrl = (env.AURIS_BASE_URL?.trim() || DEFAULT_BASE_URL).replace(/\/+$/, "");
  return { baseUrl, token };
}
