# wrtg — AI Context template

**Last updated:** 2026-07-10  
**Current version:** 0.5.6

Скопируйте в gitignored `.cursor/AI_CONTEXT.md` и добавляйте только локальное состояние deployment.

## Назначение

wrtg — прозрачный TCP proxy Telegram для OpenWrt:

```text
LAN → nftables DNAT TCP 80/443/5222 → wrtg :8443
  → direct WSS / CF Worker / optional CF Proxy / TCP / blind relay
```

## Источники истины

| Файл | Содержание |
|------|------------|
| `README.md` | Краткий landing + bootstrap one-liner |
| `docs/GUIDE.md` | Архитектура, настройка, CF Worker/Proxy, диагностика |
| `CHANGELOG.md` | История релизов |
| `openwrt/config.default` | Шаблон `/etc/wrtg/config` |
| `openwrt/cf-worker.js` | Исходник CF Worker |

## Локальный deployment

```text
Router:
Version:
FRONT_IP:
CF_WORKER_DOMAIN:
Last verification:
Known local constraints:
```

После изменений в коде — `.cursor/rules/docs-sync.mdc`.
