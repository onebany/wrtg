# Cloudflare Proxy для wrtg

Fallback через домен, проксируемый Cloudflare: `wss://kws{N}.<your-domain>/apiws`.

Cloudflare терминирует TLS и пересылает WebSocket-трафик к Telegram.

## Настройка

**Вариант A — автопул (по умолчанию):** ничего не задавайте. wrtg сам подтянет список с GitHub (см. [docs/CF_PROXY.md](../docs/CF_PROXY.md)).

**Вариант B — свой домен:**

1. Добавьте домен в Cloudflare (orange cloud / proxied).
2. DNS: `kws1`, `kws2`, … `kws5` (и `kws1-1` … для media) → любой placeholder или CNAME.
3. В `/etc/wrtg/config`:

```sh
CF_PROXY_DOMAIN="your-domain.example.com"
# Несколько доменов (балансировка):
# CF_PROXY_DOMAIN="d1.example.com,d2.example.com"
/etc/init.d/wrtg reload
```

## Troubleshooting (автопул)

Домены из общего пула (GitHub) могут быть **мёртвы**, **заблокированы ISP** или перестать резолвиться. Симптомы: `CF proxy connect failed`, нет строк `WS connected via CF proxy` в логах.

**Диагностика с роутера:**

```bash
nslookup kws1.<domain-from-pool>
curl -i "https://kws1.<domain>/apiws"
```

**Альтернативы (рекомендуется при нестабильном пуле):**

1. **CF Worker** на `*.workers.dev` — см. [docs/CF_WORKER_SETUP.md](../docs/CF_WORKER_SETUP.md)
2. **Свой домен** — `CF_PROXY_DOMAIN="your-domain.example.com"` (отключает автопул)

wrtg не требует zapret для CF Proxy.

## Порядок fallback (v0.3.0)

```
WS blacklist / ip_fail → skip direct WS
→ pool WS → direct WS
→ CF Worker pool → CF Worker
→ CF Proxy (balancer)
→ TCP → blind relay
```

Отключить CF fallback: `WRTG_NO_CFPROXY=1`.
