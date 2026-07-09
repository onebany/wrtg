# Настройка Cloudflare Worker для wrtg (workers.dev)

Пошаговая инструкция для **бесплатного** аккаунта Cloudflare **без собственного домена**.  
Worker даёт рабочий WSS-fallback для DC1/DC3/DC5, когда direct WS на `FRONT_IP` возвращает HTTP 302.

**Требуется:** wrtg **v0.4.0+** (в v0.3.0 воркер получал `dst=FRONT_IP` для DC1/3/5 и не работал; с v0.4.0 `dst` = реальный IP DC), доступ роутера к `*.workers.dev` (DNS + HTTPS/WSS).

См. также: [openwrt/CfWorker.md](../openwrt/CfWorker.md) (код Worker), [DEVELOPMENT.md](DEVELOPMENT.md) (контекст проекта).

---

## Зачем это нужно

На заблокированных сетях запросы к `kws1.web.telegram.org` … `kws5.web.telegram.org` через `FRONT_IP` получают **HTTP 302**. wrtg помечает DC в blacklist и переходит на TCP — для media это часто нестабильно.

CF Worker обходит блокировку:

```
Telegram-клиент → wrtg → wss://ваш-worker.workers.dev/apiws?dst=...&dc=...
                              → Cloudflare → Telegram DC
```

В логах при успехе: `WS connected via CF worker`.

---

## Шаг 1. Аккаунт Cloudflare (бесплатно)

1. Откройте [https://dash.cloudflare.com/sign-up](https://dash.cloudflare.com/sign-up)
2. Зарегистрируйтесь (email + пароль)
3. **Свой домен добавлять не нужно** — используем поддомен `*.workers.dev`

---

## Шаг 2. Создать Worker

1. В Dashboard: **Compute** → **Workers & Pages**
2. **Create application**
3. Выберите **Hello World** (или **Create Worker**)
4. Имя worker — любое латиницей, например `wrtg-tg-bridge`
5. Нажмите **Deploy**

---

## Шаг 3. Вставить код и задеплоить

1. На странице Worker нажмите **Edit code** (или **Quick edit**)
2. Удалите шаблонный код
3. Вставьте скрипт из [openwrt/CfWorker.md](../openwrt/CfWorker.md) (секция «Код Worker»):

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

4. **Deploy** (сохранить и опубликовать)

---

## Шаг 4. Скопировать URL workers.dev

1. На странице Worker найдите домен вида:

   ```
   wrtg-tg-bridge.<ваш-username>.workers.dev
   ```

2. Скопируйте **только hostname** (без `https://`):

   ```
   wrtg-tg-bridge.username.workers.dev
   ```

3. Проверка в браузере: `https://ваш-worker.workers.dev/apiws` → ответ `Expected websocket` (426) — это нормально.

---

## Шаг 5. Проверить доступность workers.dev с роутера

Убедитесь, что роутер резолвит домен и достигает Cloudflare:

```bash
nslookup wrtg-tg-bridge.username.workers.dev
curl -i "https://wrtg-tg-bridge.username.workers.dev/apiws?dst=149.154.175.50&dc=1"
```

Ожидаемый ответ curl: `426 Expected websocket` — Worker доступен.

Если DNS не резолвится или curl таймаутит — проверьте upstream DNS роутера, блокировку ISP или используйте другой DNS (например `1.1.1.1`). wrtg **не требует** zapret или иных DPI-обходчиков.

---

## Шаг 6. Добавить в `/etc/wrtg/config` на роутере

SSH на роутер:

```bash
ssh root@192.168.20.254
```

Отредактируйте конфиг (или скопируйте шаблон `openwrt/config.cfworker.template`):

```sh
# /etc/wrtg/config

FRONT_IP="149.154.167.220"

# Вставьте ваш workers.dev hostname:
CF_WORKER_DOMAIN="wrtg-tg-bridge.username.workers.dev"

# Несколько Worker (через запятую):
# CF_WORKER_DOMAIN="worker1.user.workers.dev,worker2.user.workers.dev"
```

Готовый шаблон в репозитории: [openwrt/config.cfworker.template](../openwrt/config.cfworker.template).

---

## Шаг 7. Перезапустить wrtg

Для **новых** переменных окружения нужен полный рестарт (не только reload):

```bash
/etc/init.d/wrtg restart
```

Проверка:

```bash
/etc/init.d/wrtg status
pidof wrtg
logread -e wrtg | tail -20
```

При старте должно быть что-то вроде:

```
wrtg starting on 0.0.0.0:8443 (front-ip=149.154.167.220, cf-workers=1, cf-proxies=0)
```

---

## Шаг 8. Проверить в логах

Откройте Telegram на устройстве в LAN. Смотрите логи:

```bash
logread -e wrtg -f
# или
logread -e wrtg | grep -iE 'CF worker|302|blacklist|connected'
```

**Успех** — строки для DC1/DC3/DC5:

```
DC1 -> trying CF worker wrtg-tg-bridge.username.workers.dev
DC1 -> WS connected via CF worker wrtg-tg-bridge.username.workers.dev
```

или через пул:

```
DC3 -> WS connected via CF worker pool (wrtg-tg-bridge.username.workers.dev)
```

---

## Troubleshooting

| Симптом | Что проверить |
|---------|---------------|
| `cf-workers=0` при старте | `CF_WORKER_DOMAIN` пуст или не передан — проверьте `/etc/wrtg/config`, сделайте **restart** |
| Нет строк `CF worker` в логах | DC2/DC4 могут идти через direct WS (302 нет) — проверьте DC1/DC3/DC5 |
| `CF worker connect failed` | DNS/connectivity к `workers.dev` — шаг 5; проверьте `CF_WORKER_DOMAIN`, firewall, upstream DNS |
| `relay init failed` | Worker не задеплоен или старый код — пересоберите Worker (шаг 3) |
| HTTP 426 в браузере на `/apiws` | Нормально — Worker ждёт WebSocket Upgrade |
| Всё ещё 302 → TCP | `WRTG_NO_CFPROXY=1` в конфиге — уберите |
| Изменили домен, reload не помог | Используйте `restart`, не `reload` |

### Ручная проверка Worker (с ПК)

```bash
# Должен вернуть 426 Expected websocket:
curl -i "https://ваш-worker.workers.dev/apiws?dst=149.154.175.50&dc=1"
```

### Отключить CF fallback (диагностика)

```sh
WRTG_NO_CFPROXY="1"
```

---

## Как wrtg использует Worker

**MTProto (DC1/3/5):** после неудачи direct WS (или при blacklist/ip_fail) →
`try_cf_fallback` открывает `wss://<worker>/apiws?dst=<real-dc-ip>&dc=<n>`.
`dst` — реальный IP DC (для DC вне `WRTG_FRONT_DCS`), поэтому воркер коннектится
к настоящему датацентру. Пул `cf_worker_pool` прогревает соединения при старте.

**Media / emoji / стикеры (v0.4.3):** такой трафик приходит как TLS или
MTProto-over-HTTP (`POST /api`) к media-DC и не является obfuscated2 — wrtg
туннелирует его через воркер (`?dst=<ip>&port=<80|443>`) к реальному DC, вместо
`blind_relay` во front (который отдаёт 302). Для этого воркер должен
поддерживать параметр `port` (см. код выше — `parseInt(...port...)`).
Отключить: `WRTG_NO_WORKER_PASSTHROUGH=1`.

---

*Последнее обновление: 2026-07-08, wrtg v0.4.3*
