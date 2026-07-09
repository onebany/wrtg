# Дневник разработки wrtg

Живой документ: состояние проекта, история изменений, решённые проблемы и открытые задачи.  
Обновлять после каждой значимой сессии разработки (см. `.cursor/rules/docs-sync.mdc`).

**Локальный AI-контекст:** скопируйте `.cursor/AI_CONTEXT.example.md` → `.cursor/AI_CONTEXT.md` (файл в `.gitignore`, не коммитить).

---

## Принципы разработки

| Принцип | Смысл |
|---------|-------|
| **Без zapret** | wrtg не зависит от zapret; не требуем его в гайдах и не предполагаем установку |
| **TCP-only scope** | Проксируем только TCP (MTProto, WS, media HTTP). UDP/WebRTC (звонки) — вне scope |
| **CF Worker — нативный обход 302** | DC1/DC3/DC5 и emoji CDN (DC5): `CF_WORKER_DOMAIN`, не сторонние DPI-обходчики |
| **CF Proxy — опциональный fallback** | Общий пул может быть нестабилен; альтернатива — свой Worker или `CF_PROXY_DOMAIN` |
| **Документация = код** | После изменений в `crates/wrtg/` — синхронизация README, ARCHITECTURE, config.default |

Подробнее: `.cursor/rules/no-zapret.mdc`.

---

## 1. Текущее состояние (v0.4.4)

**Версия:** `0.4.4` (`VERSION`, `crates/wrtg/Cargo.toml`)

### Что работает

| Компонент | Статус |
|-----------|--------|
| Прозрачный DNAT (nftables `tg_tproxy`) | ✅ |
| Direct-bridge MTProto (obfuscated2, relay-init, AES-CTR) | ✅ |
| WS bridge на `kws{N}.web.telegram.org` | ✅ DC2/DC4 на `FRONT_IP` |
| `ws_pool` — предустановленные WS per (DC, media) | ✅ |
| TTL `ws_blacklist` после HTTP 302 | ✅ (45 мин по умолчанию) |
| CF fallback chain (Worker → Proxy → direct WS → TCP) | ✅ |
| `cf_worker_pool` | ✅ (`CF_WORKER_DOMAIN` на `.254`) |
| Worker passthrough (media/emoji TLS/HTTP) | ✅ (v0.4.3+; fix procd env + multi-worker в 0.4.4) |
| `dc_learn` — self-learning IP→DC | ✅ (v0.4.4) |
| `ip_fail_until` per-DC cooldown | ✅ |
| Config hot-reload (SIGHUP) | ✅ для env, уже переданных procd |
| Health watchdog (пересоздание listener) | ✅ |
| Per-DC `FRONT_IP` (`DC{N}_FRONT_IP`, `WRTG_DC_IPS`) | ✅ |
| Media TCP после blacklist | ✅ |
| LuCI (`luci-app-wrtg`, ucode) | ✅ status показывает dc_learn |
| CI: `cargo test` + musl-бинарники | ✅ |

### Что не работает / ограничено

| Проблема | Причина | Обход |
|----------|---------|-------|
| **DC1/DC3/DC5 — HTTP 302** на direct WS | Блокировка `kws1`…`kws5` на `FRONT_IP` (149.154.167.220) | **CF Worker** ([CF_WORKER_SETUP.md](CF_WORKER_SETUP.md)) — настроен на `.254` |
| **Голос/видеозвонки** | wrtg проксирует только TCP; WebRTC — UDP | **Вне scope** wrtg |
| **SIGHUP reload** не подхватывает новые env из файла | procd задаёт env при старте; HUP читает уже установленные переменные | После смены `CF_WORKER_DOMAIN` — **`/etc/init.d/wrtg restart`** |

### Статус деплоя на роутерах

| Роутер | Версия | Примечание |
|--------|--------|------------|
| `192.168.20.254` | wrtg **0.4.4** (Rust) | DC2/DC4 через front; DC1/3/5 через CF Worker; media через worker passthrough; dc_learn |
| `192.168.30.253` | wrtg **0.4.4** | деплой 2026-07-09 |
| `192.168.88.254` | wrtg **0.4.4** | деплой 2026-07-09 |

**Типичный конфиг на `.254`:**

```sh
FRONT_IP="149.154.167.220"
CF_WORKER_DOMAIN="square-thunder-aa06....workers.dev,..."  # 3 workers
WRTG_CF_WORKER_POOL_SIZE="4"
# /etc/wrtg/dc-ips.txt — admin IP→DC; dc-ips-learned.txt — auto
```

---

## 2. Архитектура

Полное описание с mermaid-диаграммами: **[docs/ARCHITECTURE.md](ARCHITECTURE.md)**

**Кратко:** клиенты в LAN → nftables DNAT TCP 80/443/5222 к `wrtg:8443` → `SO_ORIGINAL_DST` → классификация (MTProto / TLS / HTTP) → для MTProto: расшифровка handshake → цепочка fallback до Telegram upstream.

**Цепочка fallback (v0.3.0):**

```
skip WS (blacklist / ip_fail)
  → ws_pool acquire
  → direct WS (kws{N}.web.telegram.org)
  → mark blacklist (если все 302)
  → cf_worker_pool / CF Worker direct
  → CF Proxy balancer
  → TCP fallback
  → blind relay
```

---

## 3. История изменений

### v0.1.0 — начальный Rust-порт

- Полный порт Go `tg-tproxy` → Rust `wrtg` (workspace `crates/wrtg`)
- Direct-bridge MTProto, WS bridge, TCP fallback, blind relay
- OpenWrt init/nft/CIDR скрипты, `install.sh`, musl cross-compile
- Переименование `tg-tproxy` → `wrtg`, отдельный репозиторий от `tg_wrt`

### Сессия: WS deadlock + потеря prefix (до v0.2.0)

- **Deadlock** в `bridge_ws` / `bridge_tcp`: взаимная блокировка `Mutex` на `CryptoCtx` при split read/write
  - **Fix:** `CryptoCtx::split()` — отдельные ключи для up/down
- **Потеря prefix:** `into_inner()` после handshake отбрасывал байты буфера
  - **Fix:** `tokio::io::split` + `PrefixedStream` сохраняет остаток после 64-байт handshake

### v0.2.0 — ws_pool + TTL blacklist

- **`ws_blacklist`** с TTL (`WRTG_WS_BLACKLIST_TTL_SEC`, default 2700 с) вместо постоянного blacklist до рестарта
- **`ws_pool`** — пул предустановленных WSS per (DC, media), warmup DC1–5 при старте
- GitHub Release v0.2.0, деплой на роутер

### v0.3.0 — CF fallback, ip_fail, media, стабилизация

| Изменение | Файлы / модули |
|-----------|----------------|
| CF Worker wire-up | `ws.rs`, `bridge.rs`, `config.rs`, `cf_balancer.rs` |
| `cf_worker_pool` | `cf_worker_pool.rs` |
| `ip_fail_until` per-DC | `ip_fail.rs`, `bridge.rs` |
| Media DC1 после blacklist | `mtproto.rs` `tcp_fallback_targets`, `bridge.rs` |
| Media IP fix (167.151) | `DC2_FRONT_IP="149.154.167.151"` для media DC2 на сети пользователя |
| Video stutter fix | `bridge.rs` mpsc 256, `ws.rs` `send_batch`, увеличен `MAX_WS_PAYLOAD` |
| Emergency ip_fail fix | Откат агрессивных изменений ws_pool/media; стабилизация ip_fail cooldown |
| CF Proxy balancer | `cf_balancer.rs`, `cf_proxy.rs`, `cf_proxy_domains.rs` |
| Config hot-reload SIGHUP | `config.rs`, `main.rs`, `wrtg.init reload_service` |
| Per-DC FRONT_IP | `mtproto.rs`, `config.rs` |
| Health watchdog | `watchdog.rs` |
| 91.108.x в CIDR | `openwrt/update-cidr.sh`, `lib.sh` |
| LuCI minimal | `openwrt/luci-app-wrtg/` |

### Установка (демон + LuCI ucode)

Единый `install.sh` ставит **бинарник wrtg и LuCI** (ucode `.ut` templates, без Lua):

```bash
# с ПК — всё сразу:
ROUTER=root@192.168.20.254 sh install.sh

# только LuCI:
ROUTER=root@192.168.20.254 sh install.sh --luci-only
# или:
ROUTER=root@192.168.20.254 sh openwrt/luci-app-wrtg/install-luci.sh

# без LuCI:
SKIP_LUCI=1 ROUTER=root@192.168.20.254 sh install.sh
```

На LuCI **25+/26.x** (ucode dispatcher) маршруты — **`/usr/share/luci/menu.d/*.json`**, шаблоны — **`/usr/share/ucode/luci/template/wrtg/*.ut`**. Старые lua-файлы удаляются при установке. **Не требует** `opkg install lua*`.

| Файл | Путь на роутере |
|------|-----------------|
| menu.d | `/usr/share/luci/menu.d/luci-app-wrtg.json` |
| ucode views | `/usr/share/ucode/luci/template/wrtg/{status,config,logs,action,docs}.ut` |
| docs on router | `/etc/wrtg/docs/{ARCHITECTURE,DEVELOPMENT,CF_WORKER_SETUP,CF_PROXY}.md` |
| ACL | `/usr/share/rpcd/acl.d/luci-app-wrtg.json` |

Страницы: **Status**, **Settings** (`/etc/wrtg/config`), **Logs**.

URL: **Services → wrtg → Status** (`/cgi-bin/luci/admin/services/wrtg/status`), **Documentation** (`/cgi-bin/luci/admin/services/wrtg/docs`).

**Известные проблемы:**

- DC1/DC5 (и часто DC3) — HTTP 302 на direct WS → нужен CF Worker (на `.254` настроен)
- Worker должен поддерживать `?port=` для media HTTP :80 (код в `CfWorker.md`)

---

## 4. Как устроено

### Модули (`crates/wrtg/src/`)

| Модуль | Назначение |
|--------|------------|
| `main.rs` | Bind + `serve`, SIGHUP, оркестрация fallback |
| `handshake.rs` | Чтение init, HTTP/TLS early-detect |
| `mtproto.rs` | Handshake parse, crypto, DC IPs, `ws_target_ip` |
| `bridge.rs` | WS/TCP/CF bridge, `try_ws_bridge`, `try_cf_fallback` |
| `ws.rs` | Raw WebSocket, `connect_cf_worker_ws` |
| `ws_pool.rs` | Пул direct WS |
| `cf_worker_pool.rs` | Пул CF Worker WS |
| `cf_balancer.rs` | Round-robin по доменам Worker/Proxy |
| `ws_blacklist.rs` | TTL blacklist per (DC, media) |
| `ip_fail.rs` | Cooldown FRONT_IP per DC после WS timeout |
| `config.rs` | Загрузка env, `apply_config`, `reload_from_env` |
| `watchdog.rs` | `serve`: self-healing accept loop, rebind после серии accept-ошибок |
| `media.rs`, `tls_sni.rs` | Media HTTP :80, SNI passthrough |
| `dc_learn.rs` | Self-learning IP→DC (`dc-ips.txt` / `dc-ips-learned.txt`) |

### Переменные окружения (основные)

| Переменная | Описание | По умолчанию |
|------------|----------|--------------|
| `FRONT_IP` | Глобальный front IP для WS/TCP/passthrough | `149.154.167.220` |
| `WRTG_FRONT_DCS` | Скоуп FRONT_IP: `2,4`/`all`/`none`/список; прочие DC → real IP | `2,4` |
| `DC{N}_FRONT_IP` | Per-DC override (важнее WRTG_FRONT_DCS) | — |
| `WRTG_DC_IPS` | `1:ip,2:ip` | — |
| `CF_WORKER_DOMAIN` | Worker домен(ы), через запятую | пусто |
| `CF_PROXY_DOMAIN` | CF-proxied домен(ы) | пусто → автопул с GitHub |
| `WRTG_CFPROXY_AUTO` | `1` — автозагрузка пула CF Proxy; `0` — выкл | `1` если `CF_PROXY_DOMAIN` пуст |
| `WRTG_NO_CFPROXY` | `1` — отключить CF fallback | выкл |
| `WRTG_NO_WORKER_PASSTHROUGH` | `1` — media passthrough на front, не через Worker | выкл |
| `WRTG_DC_LEARN_FILE` | Persist learned IP→DC | `/etc/wrtg/dc-ips-learned.txt` |
| `WRTG_DC_IPS_FILE` | Admin IP→DC | `/etc/wrtg/dc-ips.txt` |
| `WRTG_IP_FAIL_COOLDOWN_SEC` | Cooldown после WS timeout | `3600` |
| `WRTG_WS_POOL_SIZE` | Размер ws_pool per (DC, media) | `2` (max 8) |
| `WRTG_WS_POOL_TTL_SEC` | TTL ws_pool | `120` |
| `WRTG_CF_WORKER_POOL_SIZE` | Размер cf_worker_pool | `2` (max 4) |
| `WRTG_CF_WORKER_POOL_TTL_SEC` | TTL cf_worker_pool | `120` |
| `WRTG_WS_BLACKLIST_TTL_SEC` | TTL blacklist после 302 | `2700` |
| `WRTG_LISTEN` | Listen address | `0.0.0.0:8443` |

Полная таблица: [ARCHITECTURE.md § H](ARCHITECTURE.md#h-переменные-окружения).

### OpenWrt

- Конфиг: `/etc/wrtg/config` → `openwrt/lib.sh` `load_config` → `wrtg.init` передаёт env в procd
- Rust читает `CF_WORKER_DOMAIN` из env при старте (`config.rs::load_worker_domains`)
- **Все tunable-переменные проброшены** через `load_config` + `wrtg.init` (включая `WRTG_WS_POOL_TTL_SEC`, `WRTG_CF_WORKER_POOL_SIZE/TTL_SEC` — ранее были только в комментариях `config.default`, но не доходили до демона)
- `FRONT_IP` экспортируется и как env (не только `-front-ip` CLI): иначе SIGHUP reload сбрасывал кастомный front на default
- Пулы/TTL/cooldown кешируются через `OnceLock` → меняются только через **restart**, не reload

---

## 5. Решённые проблемы

| Симптом | Причина | Решение |
|---------|---------|---------|
| Telegram «Connecting…» бесконечно после Rust-деплоя | Deadlock в `bridge_ws` | `CryptoCtx::split()` |
| Сессии обрываются сразу после handshake | Потеря байт prefix в буфере | `PrefixedStream` + `tokio::io::split` |
| DC с 302 блокируется навсегда | Постоянный ws_blacklist | TTL blacklist (v0.2.0) |
| Медленный WS connect на каждый запрос | Новый WSS handshake каждый раз | `ws_pool` (v0.2.0) |
| DC1/3/5 только TCP, media не грузится | HTTP 302 на `kws{N}` | CF Worker fallback (v0.3.0), нужен деплой Worker |
| Повторные 5 с timeout на FRONT_IP | Нет cooldown | `ip_fail_until` (v0.3.0) |
| Видео грузится с зависаниями | mpsc backpressure, мелкие WS frames | mpsc 256, `send_batch`, MAX_WS_PAYLOAD |
| «Вообще не работает» после video-fix | Регрессия ws_pool/media | Emergency rollback, стабилизация |
| Media DC2 нестабилен | Неверный target IP для WS | `DC2_FRONT_IP=149.154.167.151` |
| Анимированные emoji не работали | HTTP :80 media CDN Host | Host rewrite + route через FRONT_IP (ранние сессии) |
| Анимированные emoji/stickers (Desktop) — синие круги | `91.108.56.155` (DC5 emoji CDN) не в `dc_alt_ips` → HTTP passthrough без rewrite `Host: kws5-1.web.telegram.org` | Добавлен IP в `dc_alt_ips`; passthrough на `FRONT_IP:80` с media Host. Для WS/302 на DC5 — **CF Worker** (нативный fallback wrtg) |

---

## 6. Открытые задачи

| # | Задача | Приоритет |
|---|--------|-----------|
| 2 | Метрики `/debug/stats` или Prometheus | средний |
| 5 | Исправить reload: re-source `/etc/wrtg/config` или документировать restart | низкий |

**Закрыто 2026-07-09:** CF Worker настроен на `.254` (3 workers); worker passthrough + procd env fix (0.4.4); dc_learn; LuCI dashboard (0.4.3).

---

## 7. Дневник (Jul 6–7, 2026)

### 2026-07-06 — v0.2.0: ws_pool + TTL blacklist

- Реализованы `ws_pool` и TTL `ws_blacklist` по образцу Flowseal/tg-ws-proxy
- Составлен gap-анализ: CF Worker — главный недостающий компонент для DC1/3/5
- Решение: следующий batch → v0.3.0

### 2026-07-06 — План v0.3.0

- Утверждён порядок CF fallback: **Worker → Proxy → Direct WS → TCP**
- LuCI: minimal scope (status, config, logs)
- Версия 0.3.0 для feature batch (0.2.0 уже на роутере)

### 2026-07-07 — Реализация и деплой v0.3.0

- Реализован полный v0.3.0 batch, деплой на `192.168.20.254`
- DC2/DC4 работают через direct WS; DC1/3/5 → 302 без CF Worker
- Пользователю предложено настроить CF Worker (`openwrt/CfWorker.md`)

### 2026-07-07 — Видео: stutter → emergency fix

- Видео на телефоне (`192.168.200.100`) грузилось с зависаниями (media DC2, `149.154.167.151`)
- Фиксы: увеличены буферы mpsc, batch send WS, MAX_WS_PAYLOAD
- Регрессия «вообще не работает» → emergency rollback части изменений
- Стабилизирован `ip_fail` cooldown

### 2026-07-07 — LuCI 404 fix (menu.d)

- **Симптом:** `No page is registered at '/admin/services/wrtg/status'` на OpenWrt 25 / LuCI 26 (HEAD `067535e`)
- **Причина:** LuCI перешёл на ucode dispatcher + `menu.d` JSON; lua `entry()` в контроллере больше не регистрирует страницы
- **Fix:** добавлен `root/usr/share/luci/menu.d/luci-app-wrtg.json`, `install-luci.sh`

### 2026-07-07 — LuCI ucode rewrite (без Lua)

- **Симптом:** `No Lua runtime installed` на OpenWrt 25.12.2 (LuCI без lua-пакетов)
- **Причина:** старый `luci-app-wrtg` использовал lua controller, CBI и `.htm` templates
- **Fix:** полный rewrite на JS views (`htdocs/luci-static/resources/view/wrtg/`), `menu.d` type `view`, ACL с `cgi-io`/`file`/`ubus`; lua-файлы удалены

### 2026-07-07 — Документация и CF Worker guide

- Создан `docs/DEVELOPMENT.md` (этот файл)
- Создан `docs/CF_WORKER_SETUP.md` — пошаговая настройка workers.dev
- Шаблон конфига: `openwrt/config.cfworker.template`
- Проверено: `CF_WORKER_DOMAIN` корректно читается (`config.rs` → `cf_balancer` → `bridge.rs`)

### 2026-07-07 — Анимированные emoji/stickers (Telegram Desktop)

- **Симптом:** стандартные анимированные emoji и часть стикеров — синий градиентный placeholder на ПК (`192.168.20.2`)
- **Логи:** `POST /api` на `91.108.56.155:80` с `Host: 91.108.56.155:80` → `passthrough -> 149.154.167.220:80` без `media-http` (Host не переписан)
- **Причина:** IP emoji CDN DC5 отсутствовал в `dc_alt_ips` (в отличие от `91.108.56.102/128/151`); `http_front_host()` не срабатывал
- **Fix:** `91.108.56.155` → `{dc:5, is_media:true}` в `mtproto.rs`; тест `rewrite_http_front_host_dc5_emoji_cdn`
- **Не затронуто:** `cdn1.telesco.pe` (149.154.175.204/205) в логах не появлялся — Desktop грузит emoji через `91.108.56.155`, не telesco.pe
- **Деплой:** пересборка + `install.sh` на `192.168.20.254`

### 2026-07-07 — Принцип «без zapret»

- Разработка wrtg **без интеграции с zapret** (на текущем этапе)
- Документация: CF Worker/Proxy troubleshooting через DNS/connectivity; звонки UDP — out of scope
- `.cursor/rules/no-zapret.mdc`, обновлены README, CF_* guides, CfWorker/CfProxy

### 2026-07-07 — Аудит: чистка кода, flaky-тест, редеплой

- **Аудит кодовой базы** — прогон `cargo test` + `cargo clippy --all-targets`
- **Flaky-тест `ip_fail_expiry`:** проходил в изоляции, падал в полном наборе. Причина — `cooldown()` кешировал значение через `OnceLock`, а `WRTG_IP_FAIL_COOLDOWN_SEC=1` из теста применялся только если тест выполнялся первым. Fix: `#[cfg(test)]`-ветка перечитывает env (как в `ws_blacklist::blacklist_ttl`)
- **CF Proxy round-robin:** `proxy_domains_for_dc` продвигал `WORKER_RR` вместо `PROXY_RR` — общий `ordered_domains` теперь принимает счётчик явным аргументом
- **Clippy:** убраны все предупреждения (мёртвые присваивания `all_blocked`, лишние `mut`, `while let`, `clamp`, упрощение boolean) — сборка без warnings
- **Редеплой:** на `192.168.20.254` крутился **0.1.0** (stale) — пересобран musl amd64 и задеплоен **0.3.0**. Проверено: ws_pool warmup 8 conn, DC2/DC4 direct WS OK, DC1/3/5 → 302 → TCP fallback (CF proxy автопул `.co.uk` не резолвится — известное ограничение)

### 2026-07-07 — Watchdog без lock + проброс всех tunable env

- **Accept loop / watchdog:** раньше `main.rs` держал `Arc<Mutex<TcpListener>>` и брал лок на всё время `accept().await` — под простоем watchdog не мог взять лок для rebind. Переписано: `watchdog::serve` владеет listener'ом единолично, self-healing — при серии из 5 accept-ошибок пересоздаёт транспарентный сокет (с backoff 200ms), без busy-loop. Убраны `SharedListener`, `run_watchdog`, поллинг `local_addr()`
- **Проброс env:** `WRTG_WS_POOL_TTL_SEC`, `WRTG_CF_WORKER_POOL_SIZE`, `WRTG_CF_WORKER_POOL_TTL_SEC` были в `config.default` (закомментированы), но **не доходили до демона** — добавлены в `lib.sh load_config` + `wrtg.init` procd env
- **SIGHUP-баг:** `FRONT_IP` передавался только CLI-аргументом `-front-ip`; reload читает env, где его не было → кастомный front откатывался на default. Теперь `FRONT_IP` экспортируется и как env
- **config.default:** переписан, сгруппирован (Core / CF / Tuning / CIDR), явно помечено что требует restart vs reload
- Тесты 35/35, clippy чисто

### 2026-07-07 — Сетевая диагностика `.254`: почему DC1/3/5 не поднимаются

Эмпирическая проверка на `192.168.20.254` (raw TCP + WSS-пробы, не со слов контекста):

| Проверка | Результат |
|----------|-----------|
| TCP-connect ко **всем** IP Telegram (DC1-5, вкл. DC2 `167.51`, DC4 `167.91`) | **timeout** — заблокировано ISP |
| TCP-connect к `149.154.167.220` | 45 ms — **единственный доступный** IP |
| `kws2/kws4` via `167.220` (WSS) | работают (101/404) |
| `kws1/kws3/kws5` via `167.220` | **HTTP 302 → `https://core.telegram.org`** (`x-redirect-host` подтверждает) |
| TCP fallback DC1/DC5 → `167.220:443` | сессия рвётся **<1 сек**, клиент ретраит ~8 сек → мёртвый релей |
| `workers.dev` / Cloudflare edge | доступны, <200 ms |
| CF-proxy автопул (`.co.uk`) | **NXDOMAIN глобально** (ISP/1.1.1.1/8.8.8.8) |

**Вывод:** на этой сети DC1/DC3/DC5 (вкл. emoji/стикеры DC5) достижимы **только через CF Worker** — прямые IP закрыты, единственный фронт 302-ит, автопул мёртв, Cloudflare доступен.

**Сверка с эталоном (Flowseal/tg-ws-proxy, amurcanov/…-ANDROID):** одинаковый механизм (MTProto→WSS via CF). Декодер CF-доменов `_dd` и список доменов **побайтово совпадают** с wrtg. Отличия Flowseal: (1) `dc_redirects` только для DC2/DC4 (wrtg фронтил все DC); (2) пиннинг `raw.githubusercontent.com→185.199.109.133` для обхода DNS-блокировки при фетче списка; (3) `ws_domains` учитывает `is_media` — **исправлено** (media → `kws{N}-1` первым).

### 2026-07-07 — v0.4.2: LuCI documentation page

- **`docs.ut`** — вкладки Architecture / Development / CF Worker Setup / CF Proxy; markdown как preformatted text (читаемо в тёмной теме)
- **`install.sh`** — копирует `docs/*.md` в `/etc/wrtg/docs/` при установке LuCI (в т.ч. `--luci-only`)
- ACL: read для `/etc/wrtg/docs/*.md`
- Версия `0.4.1 → 0.4.2`

### 2026-07-07 — v0.4.1: unified installer + LuCI ucode

- **`install.sh`** — единая установка демона и LuCI; флаги `SKIP_LUCI=1`, `--luci-only`, `install-luci.sh` как обёртка
- **LuCI без Lua** — ucode templates (`status`, `config`, `logs`, `action`) в `/usr/share/ucode/luci/template/wrtg/`; cleanup legacy lua при установке
- Версия `0.4.0 → 0.4.1`

### 2026-07-07 — v0.4.0: адаптивный front scope (`WRTG_FRONT_DCS`)

- **Проблема:** глобальный `FRONT_IP` применялся ко **всем** DC (`dc_front_ip` всегда возвращал front). Следствия: (а) на менее заблокированных сетях DC1/3/5 насильно шли на `167.220` → 302, хотя работали бы напрямую; (б) **баг CF Worker** — `try_cf_fallback` передавал воркеру `dst=FRONT_IP` для всех DC, т.е. для DC1/3/5 воркер коннектился к `167.220` (который их не маршрутизирует) → и CF-путь дох.
- **Fix:** `WRTG_FRONT_DCS` (default `2,4`, как `dc_redirects` у Flowseal). `dc_front_ip(dc)`: per-DC override → глобальный front только если DC в скоупе → иначе `""` (real IP через `ws_target_ip`). Значения `all`/`none`/список. Проброшен через `lib.sh`+`wrtg.init`; баннер и reload логируют `front-dcs`.
- **Проверено на `.254`:** баннер `front-dcs=[2, 4]`; DC2/DC4 по-прежнему через front (WS OK); DC1/DC5 теперь идут на **реальные IP** (`kws1 via 149.154.175.58`, `kws5 via 91.108.56.155`) — это и есть корректный `dst`, который получит CF Worker.
- Версия `0.3.0 → 0.4.0`. Тесты 39/39, clippy чисто. Скоуп Flowseal у wrtg теперь дефолт (пункт (1) закрыт).

### 2026-07-07 — CF Worker настроен: DC1/3/5 заработали + GitHub-IP pinning

- **GitHub-IP pinning (пункт (2) от Flowseal):** `https_get` коннектится к пиннингованным Fastly IP (`185.199.108-111.133`) для `*.githubusercontent.com`, потом system DNS как fallback (SNI/Host = реальный хост). Обходит DNS-poisoning при фетче CF-proxy списка. Тест `github_host_pinning`.
- **Flaky `ip_fail` — настоящий фикс:** два теста гонялись за глобальный map + `reset_all()`, плюс `std::env::set_var` небезопасен между потоками. Сериализованы мьютексом; `ip_fail_expiry` детерминирован (вставка past/future `Instant` напрямую, без env/sleep). Прогон 5×5 зелёный. Убран прежний `#[cfg(test)]`-хак в `cooldown()`.
- **CF Worker развёрнут** (пользовательский `*.workers.dev`, WS→TCP bridge из `openwrt/CfWorker.md`), `CF_WORKER_DOMAIN` прописан в `/etc/wrtg/config`.
- **Проверено на `.254`:** `cf-workers=1`, пул прогрел 20 conn; клиентские DC1 (`149.154.175.53/58`) и DC5 (`91.108.56.155`, emoji CDN) → `WS connected via CF worker`, **tcp_fallback=0**, ошибок нет. Мёртвый TCP-релей и retry-loop устранены — стикеры/анимированные emoji грузятся.
- Тесты 40/40, clippy 0.

### 2026-07-07 — wrtgo (Go) снят с поддержки

- Репозиторий **`homelab/wrtgo`** удалён; единственная поддерживаемая реализация — **wrtg** (Rust).
- На роутерах `.254` и `.253` wrtgo не установлен (проверка: нет `/usr/sbin/wrtgo`, `/etc/init.d/wrtgo`, `/etc/wrtgo`).
- Старый монорепозиторий **`tg_wrt`** (`legacy/tproxy-go`) устарел и заменён wrtg; не использовать для новых деплоев.

### 2026-07-08 — v0.4.3: worker passthrough (emoji), LuCI dashboard

- **Диагностика emoji:** на `.254` не грузились emoji/стикеры. Логи: media идёт как **TLS :443** и **MTProto-over-HTTP :80** (`POST /api`) к media DC (162.123, 167.255, 91.108.56.155). Это не obfuscated2 → wrtg делает `blind_relay` → `passthrough -> 149.154.167.220:80` → **HTTP 302** (front не отдаёт media). Проверено: `167.220:80` connect OK но 302; media DC :80/:443 — `000` (ISP-blocked). Только Worker достаёт real DC.
- **Fix — worker passthrough:** `blind_relay` для заблокированных media туннелирует raw байты через Worker к `real_ip:orig_port` (`try_worker_passthrough` + `relay_via_worker`, без crypto). Fallback на front при недоступности воркера. Off: `WRTG_NO_WORKER_PASSTHROUGH=1`.
- **Worker `?port=`:** воркер теперь коннектится к `dst:port` (default 443), не только :443 — нужно для :80 media. Код обновлён в `openwrt/CfWorker.md`. Backward-compatible. **Требует редеплоя воркера пользователем.**
- **relay teardown:** туннель закрывается как только одна из сторон завершилась (select + abort) — без утечки при зависании upstream.
- **Сверка с `valnesfjord/tg-ws-proxy-rs`:** тот же класс (Rust MTProto→WS, CF Worker/Proxy, Flowseal), но это **локальный MTProxy** (`127.0.0.1:1443` + secret, FakeTLS), а не transparent. Поэтому у него нет passthrough-проблемы (клиент шлёт всё как MTProto). wrtg остаётся transparent — worker passthrough это наш эквивалент.
- **LuCI dashboard:** `status.ut` переписан (карточки service/routing/worker, per-DC health, счётчики, auto-refresh); `config.ut` — quick-set форма + raw editor + Save&Restart; `logs.ut` — фильтр/подсветка/auto-refresh. Фикс: `action.ut` импортировал `system` из `fs` (не экспортируется) → кнопки start/stop были сломаны, теперь глобальный builtin. Удалены мёртвые `.js` views.
- Версия `0.4.2 → 0.4.3`. Тесты 40/40, clippy 0.

### 2026-07-08 — Release prep: дружелюбный установщик + чистка

- **`install.sh`** переписан: цветной вывод, проверка зависимостей, интерактивные вопросы (LAN_IF / FRONT_IP / CF_WORKER_DOMAIN, только при TTY и свежей установке), `mv`-into-place для бинарника (иначе ETXTBSY на запущенном), пост-проверка (running + nft) и понятный итог. Флаги `-y`/`ASSUME_YES`, `FRONT_IP=`/`CF_WORKER_DOMAIN=` для preseed.
- **`bootstrap.sh`** — установка в одну команду на роутере: качает release-бандл (`wrtg-openwrt.tar.gz` = бинарники + service + LuCI + docs) и запускает `install.sh` с `SKIP_BUILD=1`. `release.yml` собирает и публикует бандл.
- **Удалено:** `deploy-router.sh`, `fix-router-config.sh`, `build-musl-local.sh` (хардкод IP / дубль `build-rust.sh`), мёртвые LuCI `.js`.
- **Docs:** README переписан (quickstart-first), CF_WORKER_SETUP — про media/emoji passthrough, ARCHITECTURE — ветка worker-passthrough. Repo-плейсхолдер `onebany/wrtg` (заменить при публикации).

### 2026-07-09 — v0.4.4: dc_learn + worker passthrough fix

- **`dc_learn`:** self-learning IP→DC из handshake; persist `/etc/wrtg/dc-ips-learned.txt`; admin `/etc/wrtg/dc-ips.txt`; env `WRTG_DC_LEARN_FILE` / `WRTG_DC_IPS_FILE`. DC2 `149.154.167.35` в curated table (Android/Pixel).
- **Root cause media passthrough:** на `.254` в конфиге 3 Worker'а, но баннер `cf-workers=0`. Причина — **procd**: каждый `procd_set_param env` **заменяет** предыдущий список; оставался только `WRTG_CF_WORKER_POOL_SIZE`. Fix: `procd_set_param` + `procd_append_param` в `wrtg.init`.
- **`try_worker_passthrough`:** пробовать все Worker'ы (не только первый); INFO на попытку, WARN на fail/skip (раньше debug → в logread не видно).
- **LuCI:** status показывает count/preview learned mappings + note про `dc-ips.txt`.
- **`install.sh`:** деплоит шаблон `dc-ips.txt`, создаёт пустой `dc-ips-learned.txt`.
- Деплой: `.254`, `.253`, `.88.254`. Версия `0.4.3 → 0.4.4`.

---

*Последнее обновление: 2026-07-09*
