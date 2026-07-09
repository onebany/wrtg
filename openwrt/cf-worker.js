import { connect } from "cloudflare:sockets";

const ALLOWED_PORTS = new Set([80, 443, 5222]);
const TELEGRAM_CIDRS = [
  ["91.108.4.0", 22],
  ["91.108.8.0", 22],
  ["91.108.12.0", 22],
  ["91.108.16.0", 22],
  ["91.108.20.0", 22],
  ["91.108.56.0", 22],
  ["91.105.192.0", 23],
  ["149.154.160.0", 20],
  ["149.154.176.0", 20],
  ["185.76.151.0", 24],
];

function ipv4ToInt(value) {
  const parts = value.split(".");
  if (parts.length !== 4) return null;
  let result = 0;
  for (const part of parts) {
    if (!/^(0|[1-9][0-9]{0,2})$/.test(part)) return null;
    const octet = Number(part);
    if (octet > 255) return null;
    result = (result * 256 + octet) >>> 0;
  }
  return result;
}

function inCidr(ip, network, prefix) {
  const value = ipv4ToInt(ip);
  const base = ipv4ToInt(network);
  if (value === null || base === null) return false;
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return (value & mask) === (base & mask);
}

function isTelegramIp(ip) {
  return TELEGRAM_CIDRS.some(([network, prefix]) => inCidr(ip, network, prefix));
}

async function toBytes(data) {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (ArrayBuffer.isView(data)) {
    return new Uint8Array(data.buffer, data.byteOffset, data.byteLength);
  }
  if (typeof data === "string") return new TextEncoder().encode(data);
  if (data?.arrayBuffer) return new Uint8Array(await data.arrayBuffer());
  throw new TypeError("unsupported websocket message");
}

export default {
  async fetch(request, env) {
    if ((request.headers.get("Upgrade") || "").toLowerCase() !== "websocket") {
      return new Response("Expected websocket", { status: 426 });
    }

    const url = new URL(request.url);
    if (url.pathname !== "/apiws") {
      return new Response("Not found", { status: 404 });
    }

    if (env.WRTG_TOKEN && request.headers.get("X-WRTG-Token") !== env.WRTG_TOKEN) {
      return new Response("Forbidden", { status: 403 });
    }

    const dst = url.searchParams.get("dst") || "";
    const portText = url.searchParams.get("port") || "443";
    if (!/^[0-9]{1,5}$/.test(portText)) {
      return new Response("Invalid port", { status: 400 });
    }
    const port = Number(portText);
    if (!isTelegramIp(dst) || !ALLOWED_PORTS.has(port)) {
      return new Response("Destination not allowed", { status: 403 });
    }

    let socket;
    try {
      socket = connect({ hostname: dst, port });
    } catch {
      return new Response("Upstream connect failed", { status: 502 });
    }

    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    server.accept();

    const tcpReader = socket.readable.getReader();
    const tcpWriter = socket.writable.getWriter();
    let writeChain = Promise.resolve();

    server.addEventListener("message", (event) => {
      writeChain = writeChain
        .then(async () => tcpWriter.write(await toBytes(event.data)))
        .catch(() => {
          try { server.close(1011, "tcp write failed"); } catch {}
        });
    });

    server.addEventListener("close", async () => {
        try { await writeChain; } catch {}
        try { await tcpWriter.close(); } catch {}
        try { socket.close(); } catch {}
    });

    (async () => {
      try {
        while (true) {
          const { value, done } = await tcpReader.read();
          if (done) break;
          if (value) server.send(value);
        }
      } catch {
        try { server.close(1011, "tcp read failed"); } catch {}
      } finally {
        try { tcpReader.releaseLock(); } catch {}
        try { socket.close(); } catch {}
        try { server.close(); } catch {}
      }
    })();

    return new Response(null, { status: 101, webSocket: client });
  },
};
