# wrtg — AI Context (шаблон)

> **Шаблон для новых клонов.** Скопируйте в локальный файл:
> ```bash
> cp .cursor/AI_CONTEXT.example.md .cursor/AI_CONTEXT.md
> ```
> Файл `.cursor/AI_CONTEXT.md` в `.gitignore` — не коммитить.

**Last updated:** 2026-07-09

---

## 1. Purpose / Назначение

Живой контекст для AI-ассистентов, работающих с репозиторием **wrtg** (прозрачный прокси Telegram на OpenWrt).

Полная документация:
- [docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md)
- [docs/DEVELOPMENT.md](../docs/DEVELOPMENT.md)
- `.cursor/rules/docs-sync.mdc`

---

## 2. Architecture Summary / Архитектура

```
LAN-клиент → nftables DNAT → ROUTER_IP:8443 → wrtg → WSS / TCP / worker passthrough / blind relay
```

- Порты: TCP 80, 443, 5222
- Front scope: `WRTG_FRONT_DCS` default `2,4`
- Worker passthrough: media TLS/HTTP через CF Worker (`?port=`)
- dc_learn: IP→DC из handshake → `/etc/wrtg/dc-ips-learned.txt`; admin `/etc/wrtg/dc-ips.txt`
- UDP/звонки — вне scope

### Fallback chain

```
skip WS → ws_pool → direct WS → CF Worker → CF Proxy → TCP → blind_relay (worker passthrough → front)
```

### Key modules

`main`, `handshake`, `mtproto`, `dc_learn`, `bridge`, `ws`, `ws_pool`, `cf_worker_pool`, `cf_proxy`/`cf_balancer`, `ws_blacklist`, `ip_fail`, `media`, `tls_sni`, `config`, `watchdog`

---

## 3. Current Version & State / Текущее состояние

| Параметр | Значение |
|----------|----------|
| **Версия** | `0.4.4` |
| **Production** | `192.168.20.254` — wrtg 0.4.4 |
| **Другие** | `192.168.30.253`, `192.168.88.254` |
| **FRONT_IP** | `149.154.167.220` |
| **CF_WORKER_DOMAIN** | настроен на `.254` (3 workers) |

### Критический баг (fixed 0.4.4)

`procd_set_param env` вызывался многократно и **затирал** предыдущие env → `CF_WORKER_DOMAIN` не доходил до демона (`cf-workers=0`). Fix: `procd_set_param` + `procd_append_param` в `wrtg.init`.

---

## 4. Principles

См. `.cursor/rules/no-zapret.mdc` — без zapret, TCP-only, CF Worker нативный.

---

## 5. Environment (основные)

| Var | Default |
|-----|---------|
| `FRONT_IP` | `149.154.167.220` |
| `WRTG_FRONT_DCS` | `2,4` |
| `CF_WORKER_DOMAIN` | пусто |
| `WRTG_NO_WORKER_PASSTHROUGH` | выкл |
| `WRTG_DC_LEARN_FILE` | `/etc/wrtg/dc-ips-learned.txt` |
| `WRTG_DC_IPS_FILE` | `/etc/wrtg/dc-ips.txt` |

---

## 6. INSTRUCTION FOR AI

После изменений в `crates/wrtg/` или `openwrt/` обновить:
1. `.cursor/AI_CONTEXT.md` (локальный)
2. `docs/DEVELOPMENT.md`
3. `docs/ARCHITECTURE.md`

При изменении шаблона — также этот файл (коммитится).
