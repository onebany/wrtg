# Cloudflare Worker для wrtg

Fallback когда прямой WS на `FRONT_IP` возвращает HTTP 302 (блокировка `kws1`…`kws5` для DC1/DC3/DC5).

**Реализовано в v0.3.0.** wrtg пробует CF Worker после неудачи direct WS (или сразу при WS blacklist / ip_fail cooldown). Это **нативное решение wrtg** — не требует zapret или сторонних DPI-обходчиков.

Для DC5 (в т.ч. анимированные emoji через `91.108.56.155`) CF Worker — рекомендуемый путь обхода 302 на direct WS.

## Развёртывание Worker

**Пошаговая инструкция (workers.dev, без своего домена):** [docs/CF_WORKER_SETUP.md](../docs/CF_WORKER_SETUP.md)

Кратко:

1. [Cloudflare Dashboard](https://dash.cloudflare.com/) → **Compute** → **Workers & Pages**
2. **Create application** → **Hello World** → **Deploy**
3. **Edit code** — вставьте скрипт ниже → **Deploy**
4. Скопируйте домен вида `random-1234.username.workers.dev`
5. На роутере в `/etc/wrtg/config` (шаблон: `openwrt/config.cfworker.template`):

```sh
CF_WORKER_DOMAIN="random-1234.username.workers.dev"
/etc/init.d/wrtg restart
```

В логах при успехе: `WS connected via CF worker` для DC1/DC3/DC5.

## Код Worker

```javascript
import { connect } from "cloudflare:sockets";

function toBytes(data) {
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  if (typeof data === "string") return new TextEncoder().encode(data);
  if (data?.arrayBuffer) return data.arrayBuffer().then((ab) => new Uint8Array(ab));
  return new Uint8Array();
}

export default {
  async fetch(request) {
    if ((request.headers.get("Upgrade") || "").toLowerCase() !== "websocket")
      return new Response("Expected websocket", { status: 426 });
    const url = new URL(request.url);
    if (url.pathname !== "/apiws")
      return new Response("Not found", { status: 404 });

    const dst = url.searchParams.get("dst");
    const port = parseInt(url.searchParams.get("port") || "443", 10) || 443;
    const pair = new WebSocketPair();
    const client = pair[0];
    const server = pair[1];
    server.accept();

    const socket = connect({ hostname: dst, port });
    const tcpReader = socket.readable.getReader();
    const tcpWriter = socket.writable.getWriter();

    server.addEventListener("message", async (event) => {
      try { await tcpWriter.write(await toBytes(event.data)); }
      catch { try { server.close(1011, "tcp write failed"); } catch {} }
    });

    server.addEventListener("close", async () => {
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
      } catch {} finally {
        try { server.close(); } catch {}
        try { tcpReader.releaseLock(); } catch {}
        try { socket.close(); } catch {}
      }
    })();

    return new Response(null, { status: 101, webSocket: client });
  },
};
```

wrtg подключается к `wss://<CF_WORKER_DOMAIN>/apiws?dst=<telegram-dc-ip>&dc=<n>`.
