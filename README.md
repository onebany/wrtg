# wrtg (Rust)

Прозрачный прокси Telegram на OpenWrt: DNAT трафика к IP Telegram → локальный демон `wrtg`, который перенаправляет MTProto через direct-bridge и WebSocket.

Работает **без TPROXY kernel module** (DNAT + `SO_ORIGINAL_DST`).

Go-версия (`wrtgo`) снята с поддержки **2026-07-07** — используйте только **wrtg** (Rust). Старый монорепозиторий `tg_wrt` (`legacy/tproxy-go`) также устарел.

Полное руководство (архитектура, CF Worker/Proxy, конфигурация): [docs/GUIDE.md](docs/GUIDE.md).

## Возможности (v0.5.5)

- **Прозрачный DNAT** — клиентам не нужен прокси; nftables перенаправляет TCP 80/443/5222 к демону
- **Direct-bridge MTProto** — расшифровка obfuscated2, relay-init, AES-CTR в обе стороны
- **WebSocket bridge** — WSS на `FRONT_IP` с Host `kws{N}.web.telegram.org` / `kws{N}-1` (media)
- **Fallback chain** — direct WS pool → direct WS → TLS fronting (opt-in) → CF Worker pool/direct → optional CF Proxy → TCP → blind relay
- **Worker passthrough** — TLS / MTProto-over-HTTP media (emoji/стикеры) через CF Worker к real DC:port
- **Self-learning IP→DC (`dc_learn`)** — запоминает `orig_ip → DC` из handshake; `/etc/wrtg/dc-ips.txt` + `dc-ips-learned.txt`
- **WS connection pool** — non-media WSS для DC с настроенным front
- **cf_worker_pool** — общий лимит WSS per (DC, media) через все Worker-домены
- **TTL blacklist** — DC с HTTP 302 на все WS-домены пропускаются до истечения TTL
- **ip_fail_until** — cooldown на FRONT_IP после таймаутов WS (пропуск direct WS)
- **`wrtg --check`** — диагностика DNS / WSS / CF Worker / CF Proxy перед деплоем
- **Адаптивный front scope** — `FRONT_IP` применяется только к нужным DC (`WRTG_FRONT_DCS`, по умолчанию `2,4`); остальные идут напрямую / через CF Worker с корректным `dst`
- **Per-DC FRONT_IP** — `DC{N}_FRONT_IP` / `WRTG_DC_IPS`
- **Предсказуемое применение config** — `/etc/init.d/wrtg restart`; `reload` является безопасным alias
- **Health watchdog** — пересоздание listener при сбое сокета
- **TCP fallback** — `:443` на FRONT_IP или media CDN при неудаче WS
- **Blind relay** — TLS/HTTP passthrough для web.telegram.org и нераспознанного трафика
- **LuCI (ucode)** — status (вкл. dc_learn), config, logs, docs; unified `install.sh`
- **CI/CD** — `cargo test` + статические musl-бинарники (amd64/arm64/arm) в [Gitea Releases](https://git.onebany.dedyn.io/bany/wrtg/releases) и [GitHub Releases](https://github.com/onebany/wrtg/releases)

## Требования

- OpenWrt 23+ / 25+ с **nftables** (`nft`, `kmod-nft-nat`)
- `curl` или `wget`
- LAN-интерфейс с доступом клиентов (автоопределение UCI `network.lan` / `br-lan`)
- **Rust** (rustup) — только если собираете бинарник на ПК (`build-rust.sh` / `install.sh`)

## Скачать готовый бинарник

Статические бинарники публикуются в [Gitea Releases](https://git.onebany.dedyn.io/bany/wrtg/releases) (основной) и [GitHub Releases](https://github.com/onebany/wrtg/releases).

| Архитектура роутера | Файл |
|---------------------|------|
| x86_64 / amd64 (ПК, VM, x86-роутер) | `wrtg-linux-amd64` |
| aarch64 / arm64 | `wrtg-linux-arm64` |
| armv7 / armv6 (большинство OpenWrt-роутеров) | `wrtg-linux-arm` |

Узнать архитектуру на роутере: `uname -m` (`x86_64`, `aarch64`, `armv7l`).

```bash
VER=v0.5.5
ARCH=arm64   # amd64 | arm64 | arm
BASE=https://git.onebany.dedyn.io/bany/wrtg

wget -O /tmp/wrtg "${BASE}/releases/download/${VER}/wrtg-linux-${ARCH}"
chmod +x /tmp/wrtg
```

## Установка

### На роутере (рекомендуется) — bootstrap one-liner

Скачивает релиз и запускает `install.sh` без git и Rust:

```bash
wget -qO- https://git.onebany.dedyn.io/bany/wrtg/raw/branch/main/bootstrap.sh | sh
```

Конкретная версия или GitHub:

```bash
VER=v0.5.5 wget -qO- https://git.onebany.dedyn.io/bany/wrtg/raw/branch/main/bootstrap.sh | sh
WRTG_REPO=onebany/wrtg wget -qO- https://raw.githubusercontent.com/onebany/wrtg/main/bootstrap.sh | sh
```

Опции `install.sh` передаются через env: `ASSUME_YES=1`, `SKIP_LUCI=1`, `CF_WORKER_DOMAIN=…`.

### С ПК (разработчик) — clone + SSH deploy

```bash
git clone https://git.onebany.dedyn.io/bany/wrtg.git
cd wrtg
ROUTER=root@192.168.20.254 sh install.sh
```

Собирает бинарник под архитектуру роутера, загружает файлы, LuCI, nft, cron и запускает сервис.

Только LuCI:

```bash
ROUTER=root@192.168.20.254 sh install.sh --luci-only
```

Без LuCI:

```bash
SKIP_LUCI=1 ROUTER=root@192.168.20.254 sh install.sh
```

### Обновить только демон (уже установлен)

```bash
VER=v0.5.5 ARCH=arm64
wget -O /tmp/wrtg "https://git.onebany.dedyn.io/bany/wrtg/releases/download/${VER}/wrtg-linux-${ARCH}"
install -m 755 /tmp/wrtg /usr/sbin/wrtg
/etc/init.d/wrtg restart
```

С ПК:

```bash
scp dist/wrtg-linux-arm64 root@192.168.20.254:/usr/sbin/wrtg
ssh root@192.168.20.254 '/etc/init.d/wrtg restart'
```

### С ПК без Rust — release binary + install.sh

```bash
VER=v0.5.5 ARCH=arm64
BASE=https://git.onebany.dedyn.io/bany/wrtg
git clone https://git.onebany.dedyn.io/bany/wrtg.git
cd wrtg
mkdir -p dist
wget -O "dist/wrtg-linux-${ARCH}" "${BASE}/releases/download/${VER}/wrtg-linux-${ARCH}"
chmod +x "dist/wrtg-linux-${ARCH}"
SKIP_BUILD=1 ROUTER=root@192.168.20.254 sh install.sh
```

### Прямо на роутере (из клона)

```bash
cd /tmp/wrtg
sh install.sh
```

### Через Make

```bash
make build                  # dist/wrtg-linux-{amd64,arm64,arm}
make install-amd64          # только amd64, локально
ROUTER=root@192.168.1.1 make install
make bundle                 # dist/wrtg-openwrt.tar.gz + SHA256SUMS для релиза
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
| `ROUTER_IP` | LAN IP роутера для DNAT | адрес `LAN_IF` |
| `LAN_IF` | LAN-интерфейс | UCI `network.lan` → `br-lan` → `eth0` |
| `LISTEN` | Адрес демона | `0.0.0.0:8443` |
| `FRONT_IP` | Глобальный front IP для WS bridge и TLS passthrough | `149.154.167.220` |
| `WRTG_FRONT_DCS` | Каким DC применять `FRONT_IP`: `2,4` / `all` / `none` / список. Остальные DC → прямой IP | `2,4` |
| `DC{N}_FRONT_IP` | Per-DC override (напр. `DC1_FRONT_IP`); всегда важнее `WRTG_FRONT_DCS` | — |
| `WRTG_DC_IPS` | Per-DC overrides: `1:ip,2:ip` | — |
| `CF_WORKER_DOMAIN` | Cloudflare Worker — WS fallback (через запятую) | пусто |
| `WRTG_CF_WORKER_TOKEN` | Secret, совпадающий с Worker secret `WRTG_TOKEN` | пусто |
| `WRTG_NO_WORKER_PASSTHROUGH` | Не туннелировать media passthrough через Worker (`1`) | выкл |
| `WRTG_DC_LEARN_FILE` | Файл learned IP→DC (append-only) | `/etc/wrtg/dc-ips-learned.txt` |
| `WRTG_DC_IPS_FILE` | Админский IP→DC файл | `/etc/wrtg/dc-ips.txt` |
| `CF_PROXY_DOMAIN` | Cloudflare-proxied домен — WS fallback (через запятую) | пусто → [автопул](docs/GUIDE.md#cf-proxy-fallback) |
| `WRTG_CFPROXY_AUTO` | Автозагрузка публичного CF Proxy pool (`1` — вкл) | `0` |
| `WRTG_NO_CFPROXY` | Отключить CF Worker/Proxy fallback (`1`) | выкл |
| `WRTG_IP_FAIL_COOLDOWN_SEC` | Cooldown FRONT_IP после WS timeout (сек) | `3600` |
| `WRTG_FRONTING_SNI` | Opt-in TLS fronting SNI (пусто = выкл) | пусто |
| `WRTG_FRONTING_COOLDOWN_SEC` | Cooldown после неудачи fronting (сек) | `1800` |
| `WRTG_DC_FAIL_COOLDOWN_SEC` | Cooldown адаптивного WS timeout per DC (сек) | `60` |
| `WRTG_WS_FAIL_TIMEOUT_SEC` | Обычный WS connect timeout (сек) | `5` |
| `WRTG_WS_FAIL_TIMEOUT_FAST_SEC` | Быстрый WS timeout после fail DC (сек) | `2` |
| `WRTG_WS_POOL_SIZE` | Non-media direct WS pool per fronted DC, макс. 8 | `2` |
| `WRTG_WS_POOL_TTL_SEC` | TTL соединений в пуле (сек) | `120` |
| `WRTG_CF_WORKER_POOL_SIZE` | Общий Worker pool per (DC, media), макс. 4 | `2` |
| `WRTG_CF_WORKER_POOL_TTL_SEC` | TTL соединений CF Worker pool (сек) | `120` |
| `WRTG_WS_BLACKLIST_TTL_SEC` | TTL blacklist DC после HTTP 302 (сек) | `2700` (45 мин) |
| `WRTG_CFPROXY_429_COOLDOWN_SEC` | Начальный cooldown CF proxy после HTTP 429 (сек) | `45` |
| `WRTG_CFPROXY_429_MAX_COOLDOWN_SEC` | Макс. cooldown CF proxy после 429 (сек) | `300` |
| `WRTG_CFPROXY_PARALLEL` | Параллельные попытки CF proxy fallback | `2` |
| `WRTG_DOH_CACHE_SEC` | TTL кеша DoH-резолва для CF proxy (сек) | `300` |
| `WRTG_WS_PING_SEC` | Интервал idle WebSocket ping в bridge (сек) | `30` |
| `WRTG_TCP_KEEPALIVE_SEC` | TCP keepalive на relay-сокетах (сек) | `30` |
| `CIDR_UPDATE_HOUR` | Час обновления CIDR | `4` |

Дополнительные подсети: `/etc/wrtg/cidr-extra.txt`.

После изменений:

```bash
/etc/init.d/wrtg restart   # применить изменения daemon config
/etc/init.d/wrtg reload    # alias для restart
/etc/wrtg/update-cidr.sh   # только CIDR/nft
```

> Размеры/TTL пулов, cooldown и blacklist TTL кешируются при старте — меняются только через `restart`, не `reload`.

CF Worker / CF Proxy: [docs/GUIDE.md](docs/GUIDE.md), исходник Worker — [`openwrt/cf-worker.js`](openwrt/cf-worker.js).

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
install.sh              # установка демона + LuCI (bootstrap, ROUTER=..., SKIP_LUCI=1)
bootstrap.sh            # one-liner: release bundle или binary+source → install.sh
openwrt/luci-app-wrtg/  # LuCI ucode app (status, config, logs)
docker/                 # Dockerfile + compose
```

## Ограничения

- **Голосовые/видеозвонки** — wrtg перехватывает только **TCP** (сигналинг). Медиа идёт по **UDP/WebRTC** и **не проксируется**; это вне scope wrtg (интеграция с zapret не планируется).
- **DC1/DC3/DC5** — при HTTP 302 на direct WS используйте **CF Worker** (`CF_WORKER_DOMAIN`) — нативный fallback wrtg, без zapret.
- CF Worker / CF Proxy fallback: [docs/GUIDE.md](docs/GUIDE.md).
