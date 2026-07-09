# wrtg — AI Context template

**Last updated:** 2026-07-09  
**Current version:** 0.5.0

Скопируйте этот файл в gitignored `.cursor/AI_CONTEXT.md` и добавляйте туда
только локальное состояние deployment.

## Назначение

wrtg — прозрачный TCP proxy Telegram для OpenWrt:

```text
LAN → nftables DNAT TCP 80/443/5222 → wrtg :8443
  → direct WSS / CF Worker / optional CF Proxy / TCP / blind relay
```

- DNAT + `SO_ORIGINAL_DST`, без kernel TPROXY.
- Direct obfuscated2 MTProto bridge с AES-CTR.
- Worker passthrough для media HTTP/TLS.
- `dc_learn` хранит IP→DC в `dc-ips-learned.txt`; admin override — `dc-ips.txt`.
- UDP/WebRTC вне scope.
- wrtg не зависит от zapret.

## 0.5.0

- verified TLS и строгий WebSocket handshake/framing;
- Worker Telegram CIDR/port allowlist + optional secret;
- relay teardown при закрытии любой стороны;
- bounded direct/Worker pools;
- public CF Proxy pool opt-in;
- atomic CIDR/nft update;
- config применяется restart (`reload` = restart);
- LuCI POST/CSRF и полный uninstall.

## Источники истины

- `README.md` — установка и пользовательская конфигурация;
- `docs/ARCHITECTURE.md` — поток и модули;
- `docs/DEVELOPMENT.md` — текущее состояние и release checks;
- `CHANGELOG.md` — история;
- `openwrt/config.default` — OpenWrt config;
- `openwrt/cf-worker.js` — единственный исходник Worker.

## Локальный deployment

Заполните в `.cursor/AI_CONTEXT.md`:

```text
Router:
Version:
FRONT_IP:
CF_WORKER_DOMAIN:
Last verification:
Known local constraints:
```

После изменений в коде следуйте `.cursor/rules/docs-sync.mdc` и
`.cursor/rules/no-zapret.mdc`.
