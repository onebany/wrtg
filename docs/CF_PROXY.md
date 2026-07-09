# CF Proxy — общий пул доменов (как Flowseal)

wrtg может использовать **общий пул CF Proxy доменов** из репозитория [Flowseal/tg-ws-proxy](https://github.com/Flowseal/tg-ws-proxy) — тот же список, что и в TgWsProxy.

Подключение: `wss://kws{N}.<domain>/apiws` через Cloudflare (orange cloud).

## Автозагрузка (по умолчанию)

Если `CF_PROXY_DOMAIN` **не задан**, wrtg:

1. При старте загружает **встроенный** список (20 доменов, декодированных из `cfproxy-domains.txt`).
2. Сразу и **каждый час** скачивает актуальный список:
   `https://raw.githubusercontent.com/Flowseal/tg-ws-proxy/main/.github/cfproxy-domains.txt`
3. Декодирует домены (Caesar cipher, суффикс `.co.uk`).
4. Обновляет пул только если после валидации **≥ 3** доменов.

В логах: `CF proxy domain pool updated from GitHub (N domains)`.

## Переменные окружения

| Переменная | Описание | По умолчанию |
|------------|----------|--------------|
| `CF_PROXY_DOMAIN` | Свой домен(ы) через запятую — **отключает** автозагрузку | пусто |
| `WRTG_CFPROXY_AUTO` | `1` / `true` — включить автозагрузку; `0` — выкл | `1` (если `CF_PROXY_DOMAIN` пуст) |
| `WRTG_NO_CFPROXY` | `1` — отключить весь CF fallback (Worker + Proxy) | выкл |

## Свой домен

Рекомендуется для стабильности (лимиты Cloudflare на общий пул).

1. Добавьте домен в Cloudflare, режим SSL **Flexible**.
2. DNS A-записи: `kws1`…`kws5`, `kws203` → IP Telegram DC (см. [openwrt/CfProxy.md](../openwrt/CfProxy.md)).
3. В `/etc/wrtg/config`:

```sh
CF_PROXY_DOMAIN="your-domain.example.com"
/etc/init.d/wrtg reload
```

При заданном `CF_PROXY_DOMAIN` автозагрузка с GitHub **не выполняется**.

## Troubleshooting автопула

Общий пул доменов (Flowseal/tg-ws-proxy) **не гарантирован**: отдельные домены могут умереть, быть заблокированы ISP или не резолвиться с вашего DNS.

| Симптом | Что проверить |
|---------|---------------|
| `CF proxy connect failed` | DNS: `nslookup kws1.<domain>` с роутера; curl к `https://kws1.<domain>/apiws` |
| Пул обновился, но fallback не работает | После валидации нужно ≥ 3 доменов — смотрите лог `CF proxy domain pool updated` |
| Все домены пула мёртвы | Переключитесь на **CF Worker** ([CF_WORKER_SETUP.md](CF_WORKER_SETUP.md)) или задайте **свой** `CF_PROXY_DOMAIN` |

wrtg **не требует** zapret. Проблемы CF Proxy — это DNS/connectivity и качество пула, а не allowlist сторонних обходчиков.

## Порядок fallback

```
WS blacklist / ip_fail → pool WS → direct WS
→ CF Worker pool → CF Worker
→ CF Proxy (balancer, общий пул или CF_PROXY_DOMAIN)
→ TCP → blind relay
```

Подробнее: [openwrt/CfProxy.md](../openwrt/CfProxy.md), [ARCHITECTURE.md](ARCHITECTURE.md).
