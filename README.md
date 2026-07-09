# wrtg (Rust)

Прозрачный прокси Telegram на OpenWrt: DNAT трафика к IP Telegram → локальный демон `wrtg`, который перенаправляет MTProto через direct-bridge и WebSocket.

Работает **без TPROXY kernel module** (DNAT + `SO_ORIGINAL_DST`).

Go-версия (`wrtgo`) снята с поддержки **2026-07-07** — используйте только **wrtg** (Rust). Старый монорепозиторий `tg_wrt` (`legacy/tproxy-go`) также устарел.

Подробная архитектура и блок-схемы: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).  
Дневник разработки и текущее состояние: [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Возможности (v0.4.4)

- **Прозрачный DNAT** — клиентам не нужен прокси; nftables перенаправляет TCP 80/443/5222 к демону
- **Direct-bridge MTProto** — расшифровка obfuscated2, relay-init, AES-CTR в обе стороны
- **WebSocket bridge** — WSS на `FRONT_IP` с Host `kws{N}.web.telegram.org` / `kws{N}-1` (media)
- **CF fallback chain** — CF Worker pool → CF Proxy balancer → direct WS → TCP → blind relay
- **Worker passthrough** — TLS / MTProto-over-HTTP media (emoji/стикеры) через CF Worker к real DC:port
- **Self-learning IP→DC (`dc_learn`)** — запоминает `orig_ip → DC` из handshake; `/etc/wrtg/dc-ips.txt` + `dc-ips-learned.txt`
- **WS connection pool** — предустановленные соединения per (DC, media); warmup DC1–5 при старте
- **cf_worker_pool** — пул WSS через Cloudflare Worker
- **TTL blacklist** — DC с HTTP 302 на все WS-домены пропускаются до истечения TTL
- **ip_fail_until** — cooldown на FRONT_IP после таймаутов WS (пропуск direct WS)
- **Адаптивный front scope** — `FRONT_IP` применяется только к нужным DC (`WRTG_FRONT_DCS`, по умолчанию `2,4`); остальные идут напрямую / через CF Worker с корректным `dst`
- **Per-DC FRONT_IP** — `DC{N}_FRONT_IP` / `WRTG_DC_IPS`
- **Config hot-reload** — `kill -HUP` / `/etc/init.d/wrtg reload`
- **Health watchdog** — пересоздание listener при сбое сокета
- **TCP fallback** — `:443` на FRONT_IP или media CDN при неудаче WS
- **Blind relay** — TLS/HTTP passthrough для web.telegram.org и нераспознанного трафика
- **LuCI (ucode)** — status (вкл. dc_learn), config, logs, docs; unified `install.sh`
- **CI/CD** — `cargo test` + статические musl-бинарники (amd64/arm64/arm) в GitHub Releases

## Требования

- OpenWrt 23+ / 25+ с **nftables** (`nft`, `kmod-nft-nat`)
- `curl` или `wget`
- LAN-интерфейс с доступом клиентов (по умолчанию `eth0`)
- **Rust** (rustup) — только если собираете бинарник на ПК (`build-rust.sh` / `install.sh`)

## Скачать готовый бинарник

Готовые статические бинарники публикуются в [GitHub Releases](https://github.com/homelab/wrtg/releases).

| Архитектура роутера | Файл |
|---------------------|------|
| x86_64 / amd64 (ПК, VM, x86-роутер) | `wrtg-linux-amd64` |
| aarch64 / arm64 | `wrtg-linux-arm64` |
| armv7 / armv6 (большинство OpenWrt-роутеров) | `wrtg-linux-arm` |

Узнать архитектуру на роутере: `uname -m` (`x86_64`, `aarch64`, `armv7l`).

### Скачать на роутер

Подставьте нужную версию и архитектуру:

```bash
VER=v0.4.4
ARCH=arm64   # amd64 | arm64 | arm

wget -O /tmp/wrtg "https://github.com/homelab/wrtg/releases/download/${VER}/wrtg-linux-${ARCH}"
chmod +x /tmp/wrtg
```

Через `curl`:

```bash
curl -fsSL -o /tmp/wrtg "https://github.com/homelab/wrtg/releases/download/${VER}/wrtg-linux-${ARCH}"
chmod +x /tmp/wrtg
```

### Установка с готовым бинарником

Бинарник — только демон. Скрипты init/nft/CIDR — в репозитории. Проще всего клонировать репо и вызвать `install.sh` с уже скачанным файлом:

```bash
cd /tmp
git clone https://github.com/homelab/wrtg.git
cd wrtg
mkdir -p dist
wget -O "dist/wrtg-linux-${ARCH}" "https://github.com/homelab/wrtg/releases/download/${VER}/wrtg-linux-${ARCH}"
chmod +x "dist/wrtg-linux-${ARCH}"
SKIP_BUILD=1 sh install.sh
```

Вручную (минимум):

```bash
install -m 755 /tmp/wrtg /usr/sbin/wrtg
# затем скопируйте openwrt/* из репозитория и:
/etc/init.d/wrtg enable
/etc/init.d/wrtg start
```

С ПК (бинарник + конфиги по SSH):

```bash
scp dist/wrtg-linux-arm64 root@192.168.1.1:/usr/sbin/wrtg
ssh root@192.168.1.1 'chmod +x /usr/sbin/wrtg && /etc/init.d/wrtg restart'
```

## Быстрая установка

### С ПК (рекомендуется)

```bash
cd wrtg
ROUTER=root@192.168.20.254 sh install.sh
```

Скрипт соберёт бинарник под архитектуру роутера, загрузит файлы, установит LuCI (ucode), настроит nft, cron и запустит сервис.

Только LuCI (без пересборки демона):

```bash
ROUTER=root@192.168.20.254 sh install.sh --luci-only
# или:
ROUTER=root@192.168.20.254 sh openwrt/luci-app-wrtg/install-luci.sh
```

Без LuCI:

```bash
SKIP_LUCI=1 ROUTER=root@192.168.20.254 sh install.sh
```

### Прямо на роутере

```bash
cd /tmp/wrtg
sh install.sh
```

### Через Make

```bash
make build                  # dist/wrtg-linux-{amd64,arm64,arm}
ROUTER=root@192.168.1.1 make install
```

### Быстрый деплой (уже собранный бинарник)

```bash
sh build-rust.sh amd64
ROUTER=root@192.168.20.254 sh deploy-router.sh
```

## Проверка

```bash
/etc/init.d/wrtg status
logread -e wrtg | tail
nft list table inet tg_tproxy
```

Откройте Telegram на устройстве в LAN — в логах должны появиться строки `direct handshake OK` или `WS connected`.

## Настройка

Файл `/etc/wrtg/config`:

| Параметр | Описание | По умолчанию |
|----------|----------|--------------|
| `ROUTER_IP` | IP роутера для DNAT | авто (src route) |
| `LAN_IF` | LAN-интерфейс | `eth0` |
| `LISTEN` | Адрес демона | `0.0.0.0:8443` |
| `FRONT_IP` | Глобальный front IP для WS bridge и TLS passthrough | `149.154.167.220` |
| `WRTG_FRONT_DCS` | Каким DC применять `FRONT_IP`: `2,4` / `all` / `none` / список. Остальные DC → прямой IP | `2,4` |
| `DC{N}_FRONT_IP` | Per-DC override (напр. `DC1_FRONT_IP`); всегда важнее `WRTG_FRONT_DCS` | — |
| `WRTG_DC_IPS` | Per-DC overrides: `1:ip,2:ip` | — |
| `CF_WORKER_DOMAIN` | Cloudflare Worker — WS fallback (через запятую) | пусто |
| `WRTG_NO_WORKER_PASSTHROUGH` | Не туннелировать media passthrough через Worker (`1`) | выкл |
| `WRTG_DC_LEARN_FILE` | Файл learned IP→DC (append-only) | `/etc/wrtg/dc-ips-learned.txt` |
| `WRTG_DC_IPS_FILE` | Админский IP→DC файл | `/etc/wrtg/dc-ips.txt` |
| `CF_PROXY_DOMAIN` | Cloudflare-proxied домен — WS fallback (через запятую) | пусто → [автопул](docs/CF_PROXY.md) |
| `WRTG_CFPROXY_AUTO` | Автозагрузка пула CF Proxy с GitHub (`0` — выкл) | `1` если `CF_PROXY_DOMAIN` пуст |
| `WRTG_NO_CFPROXY` | Отключить CF Worker/Proxy fallback (`1`) | выкл |
| `WRTG_IP_FAIL_COOLDOWN_SEC` | Cooldown FRONT_IP после WS timeout (сек) | `3600` |
| `WRTG_WS_POOL_SIZE` | Размер пула предустановленных WS на (DC, media), макс. 8 | `2` |
| `WRTG_WS_POOL_TTL_SEC` | TTL соединений в пуле (сек) | `120` |
| `WRTG_CF_WORKER_POOL_SIZE` | Размер CF Worker pool per DC, макс. 4 | `2` |
| `WRTG_CF_WORKER_POOL_TTL_SEC` | TTL соединений CF Worker pool (сек) | `120` |
| `WRTG_WS_BLACKLIST_TTL_SEC` | TTL blacklist DC после HTTP 302 (сек) | `2700` (45 мин) |
| `CIDR_UPDATE_HOUR` | Час обновления CIDR | `4` |

Дополнительные подсети: `/etc/wrtg/cidr-extra.txt`.

После изменений:

```bash
/etc/init.d/wrtg reload    # SIGHUP — FRONT_IP, CF-домены, per-DC IP
/etc/init.d/wrtg restart   # полный рестарт — нужен для пулов/TTL, LISTEN, новых переменных
/etc/wrtg/update-cidr.sh   # только CIDR/nft
```

> Размеры/TTL пулов, cooldown и blacklist TTL кешируются при старте — меняются только через `restart`, не `reload`.

CF Worker: [пошаговая настройка](docs/CF_WORKER_SETUP.md) ([код Worker](openwrt/CfWorker.md), [шаблон конфига](openwrt/config.cfworker.template)). CF Proxy: [`openwrt/CfProxy.md`](openwrt/CfProxy.md).

## Удаление

```bash
sh uninstall.sh
# без подтверждения:
FORCE=1 sh uninstall.sh
```

## Docker

Для локальной проверки бинарника или запуска на Linux VPS с ручным nft DNAT:

```bash
cd docker
docker compose build
docker compose up -d
```

`docker-compose.yml` использует `network_mode: host` и `CAP_NET_ADMIN`.

## Структура

```
Cargo.toml              # workspace
crates/wrtg/            # Rust daemon
install.sh              # установка демона + LuCI (ROUTER=..., SKIP_LUCI=1, --luci-only)
openwrt/luci-app-wrtg/  # LuCI ucode app (status, config, logs)
docker/                 # Dockerfile + compose
```

## Ограничения

- **Голосовые/видеозвонки** — wrtg перехватывает только **TCP** (сигналинг). Медиа идёт по **UDP/WebRTC** и **не проксируется**; это вне scope wrtg (интеграция с zapret не планируется).
- **DC1/DC3/DC5** — при HTTP 302 на direct WS используйте **CF Worker** (`CF_WORKER_DOMAIN`) — нативный fallback wrtg, без zapret.
- CF Worker / CF Proxy fallback: [`openwrt/CfWorker.md`](openwrt/CfWorker.md), [`openwrt/CfProxy.md`](openwrt/CfProxy.md), [CF_WORKER_SETUP.md](docs/CF_WORKER_SETUP.md).
