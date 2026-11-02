// The connection code a board scans: farol://connect?u=<url>&p=<password>&n=<host>,
// each value percent-encoded (rust/logic/src/qr.rs parses it). Nothing is stored.
import qrcode from "qrcode-generator";

/**
 * The daemon WebSocket URL a board should open. Accepts what people paste:
 * "paseo.example.net", "https://paseo.example.net", "wss://host:port/ws".
 */
export function normalizeAddress(input: string): string | null {
  let s = input.trim();
  if (!s) return null;
  if (!/^[a-z]+:\/\//i.test(s)) s = `wss://${s}`;
  let url: URL;
  try {
    url = new URL(s);
  } catch {
    return null;
  }
  const scheme = { "http:": "ws:", "https:": "wss:", "ws:": "ws:", "wss:": "wss:" }[url.protocol];
  if (!scheme || !url.hostname) return null;
  url.protocol = scheme;
  if (url.pathname === "" || url.pathname === "/") url.pathname = "/ws";
  url.hash = "";
  return url.toString();
}

export function connectPayload(url: string, password: string, host: string): string {
  const parts = [`u=${encodeURIComponent(url)}`];
  if (password) parts.push(`p=${encodeURIComponent(password)}`);
  if (host) parts.push(`n=${encodeURIComponent(host)}`);
  return `farol://connect?${parts.join("&")}`;
}

/** QR modules, one string per row ("1" dark). Error correction M survives a phone-screen glare. */
export function qrRows(payload: string): string[] {
  const code = qrcode(0, "M");
  code.addData(payload, "Byte");
  code.make();
  const n = code.getModuleCount();
  const rows: string[] = [];
  for (let r = 0; r < n; r++) {
    let row = "";
    for (let c = 0; c < n; c++) row += code.isDark(r, c) ? "1" : "0";
    rows.push(row);
  }
  return rows;
}
