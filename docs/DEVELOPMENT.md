# Разработка wrtg

**Current version:** 0.5.0  
**Last updated:** 2026-07-09

История релизов находится в [`CHANGELOG.md`](../CHANGELOG.md); этот файл содержит
только актуальное состояние и правила проверки.

## Принципы

- wrtg не зависит от zapret.
- Scope — TCP 80/443/5222; UDP/WebRTC не проксируется.
- CF Worker — основной fallback для DC, недоступных через front.
- `/etc/wrtg/config` применяется полным restart.
- Публичные Worker/Proxy endpoints не должны отключать TLS или становиться open proxy.

## Текущая архитектура

Подробно: [`ARCHITECTURE.md`](ARCHITECTURE.md).

```text
LAN → nft DNAT → wrtg :8443 → classify
  MTProto → direct WS pool → direct WS → CF Worker pool/direct
            → optional CF Proxy → TCP → blind relay
  TLS/HTTP → Worker passthrough → blind relay
```

Ключевые модули:

- `main.rs`, `handshake.rs`, `mtproto.rs` — accept, классификация, MTProto crypto;
- `bridge.rs`, `ws.rs`, `tls.rs` — relay, WebSocket framing, verified TLS;
- `ws_pool.rs`, `cf_worker_pool.rs` — ограниченные connection pools;
- `dc_learn.rs` — runtime IP→DC map;
- `config.rs`, `cf_balancer.rs`, `cf_proxy_domains.rs` — startup config/fallback;
- `sockopt/`, `watchdog.rs` — Linux transparent socket и accept recovery.

## Изменения 0.5.0

### Security

- TLS certificate validation включена для WSS и GitHub HTTPS.
- WebSocket upgrade проверяет `101`, `Upgrade`, `Connection` и
  `Sec-WebSocket-Accept`; parser поддерживает fragmentation и лимиты.
- Worker вынесен в `openwrt/cf-worker.js`, ограничен Telegram CIDR и портами
  80/443/5222; добавлен optional secret `WRTG_TOKEN` /
  `WRTG_CF_WORKER_TOKEN`.
- Bootstrap проверяет SHA256 release bundle.
- LuCI service actions переведены на POST + auth token.

### Reliability

- Одностороннее закрытие завершает WS/TCP relay; зависшие половины отменяются.
- Ошибка initial send в worker passthrough возвращает client для следующего Worker.
- Direct pool создаёт только используемые non-media front connections.
- Worker pool ограничен общим размером per `(DC, media)`, независимо от числа доменов.
- Public CF Proxy pool выключен по умолчанию; за соединение не более трёх попыток.
- `dc_learn` использует строгий IPv4 parser; admin-файл имеет приоритет.

### OpenWrt

- LAN interface определяется через UCI/`br-lan`; `ROUTER_IP` берётся с LAN_IF.
- CIDR-кандидат валидируется, nft replacement выполняется atomic batch.
- `reload` является alias для restart и действительно применяет config.
- zapret/calls helpers остаются community-файлами, но не устанавливаются и не
  вызываются core setup.
- Удалены дублирующие `wrtg.nft`, Worker/Proxy markdown и config template.
- Uninstall удаляет LuCI menu/templates/ACL.

## Конфигурация и restart

Все daemon-переменные задаются в `/etc/wrtg/config`. После изменения:

```sh
/etc/init.d/wrtg restart
```

`reload` оставлен для совместимости OpenWrt и вызывает тот же restart.
`update-cidr.sh` обновляет только CIDR и nft.

## Проверки перед релизом

```sh
cargo fmt --all -- --check
cargo clippy -p wrtg --all-targets -- -D warnings
cargo test -p wrtg
shellcheck -x install.sh bootstrap.sh uninstall.sh build-rust.sh \
  openwrt/*.sh openwrt/wrtg.init openwrt/luci-app-wrtg/install-luci.sh
sh build-rust.sh amd64
```

Дополнительно:

1. `VERSION` равен версии `crates/wrtg/Cargo.toml` и tag.
2. Worker source прошёл `node --check openwrt/cf-worker.js`.
3. На роутере service running, nft table загружена.
4. Логи показывают direct WS и Worker passthrough без front fallback для media.
5. Число idle Worker connections не превышает
   `DC × media variants × WRTG_CF_WORKER_POOL_SIZE`.

## Известные ограничения

- Голосовые/видеозвонки используют UDP/WebRTC и вне scope.
- `SO_ORIGINAL_DST` реализован только для IPv4.
- Изменение Worker source требует отдельного deploy в Cloudflare.
- Public CF Proxy pool не контролируется проектом и используется только opt-in.
