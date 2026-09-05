# wrtg

Прозрачный прокси Telegram для OpenWrt. nftables на роутере перенаправляет TCP к адресам Telegram в демон wrtg, а тот доводит трафик до дата-центра через WebSocket на фронт Telegram, Cloudflare Worker или CF Proxy. Телефоны и ПК в LAN настраивать не нужно.

[![Release](https://img.shields.io/github/v/release/onebany/wrtg)](https://github.com/onebany/wrtg/releases)
[![License: MIT](https://img.shields.io/github/license/onebany/wrtg)](LICENSE)
[![Build](https://github.com/onebany/wrtg/actions/workflows/build.yml/badge.svg)](https://github.com/onebany/wrtg/actions/workflows/build.yml)
[![Platform](https://img.shields.io/badge/platform-OpenWrt-00B5E2)](https://openwrt.org)

**Version:** 1.3.1 · **Last updated:** 2026-09-05

[Релизы](https://github.com/onebany/wrtg/releases) · [CHANGELOG.md](CHANGELOG.md) · [Исходник CF Worker](openwrt/cf-worker.js) · [Issues](https://github.com/onebany/wrtg/issues)

---

## Оглавление

- [Возможности](#возможности)
- [Установка](#установка)
- [Настройка](#настройка)
- [CF Worker](#cf-worker)
- [CF Proxy](#cf-proxy)
- [Диагностика](#диагностика)
- [Ограничения](#ограничения)
- [Как это работает](#как-это-работает)
- [Разработчикам](#разработчикам)

---

## Возможности

- **Перехват без настройки клиентов.** nftables DNAT забирает TCP к адресам Telegram на портах 80, 443 и 5222.
- **Цепочка запасных путей:** пул WebSocket на фронт, новый WebSocket, TLS fronting по желанию, CF Worker, CF Proxy, прямой TCP, blind relay. Мёртвый путь получает cooldown, и следующая сессия его пропускает.
- **CF Proxy из коробки.** Фронт Telegram отвечает HTTP 302 для DC1, DC3 и DC5, им нужен туннель через Cloudflare. Общий пул доменов включён по умолчанию; свой Worker быстрее и не зависит от чужих доменов.
- **Демон учит соответствие адреса и DC** по рукопожатиям клиентов.
- **LuCI:** статус, настройки, логи, обновление, документация. По-русски и по-английски.
- **Один статический бинарник** около 3 МБ: `x86_64`, `aarch64`, `armv7`, `mipsel` (MT7621).
- **Автообновление** раз в сутки с проверкой sha256.

---

## Установка

На роутере:

```sh
wget -qO- https://raw.githubusercontent.com/onebany/wrtg/main/bootstrap.sh | sh
```

`bootstrap.sh` скачивает релиз, сверяет sha256 и запускает `install.sh`: бинарник, конфиг, nft, cron, LuCI. Переменные окружения: `VER=vX.Y.Z` (конкретный релиз), `WRTG_REPO=owner/repo`, `ASSUME_YES=1`, `SKIP_LUCI=1`, `CF_WORKER_DOMAIN=…`.

Проверка:

```sh
/etc/init.d/wrtg status
wrtg --check          # DNS и WSS-пробы по каждому DC, exit 0 = OK
wrtg --stats          # счётчики: после первых сессий растут accepted и ws_pool_hit или cf_proxy
```

LuCI: **Services → wrtg**. Status (управление службой, версии, обновление), Settings (язык, датацентры, перехват, CF Proxy, CF Worker, логи и лимиты, автообновление, проверка связности, обучение DC, редактор конфига), Logs (фильтр по подстроке, обновление раз в 5 с), Documentation (этот файл).

### Офлайн-установка

Хосты раздачи релизов GitHub живут в диапазоне Fastly `185.199.108-111.0/24`. Часть провайдеров его режет, и тогда `bootstrap.sh`, кнопка Update и автообновление не скачают ничего. Принесите бандл сами. На машине с доступом к GitHub:

```sh
wget https://github.com/onebany/wrtg/releases/latest/download/wrtg-openwrt.tar.gz
wget https://github.com/onebany/wrtg/releases/latest/download/SHA256SUMS
tar -czf - wrtg-openwrt.tar.gz SHA256SUMS | ssh root@<роутер> 'tar -xzf - -C /tmp'
```

На роутере:

```sh
cd /tmp
grep wrtg-openwrt.tar.gz SHA256SUMS | sha256sum -c -
tar -xzf wrtg-openwrt.tar.gz
SKIP_BUILD=1 sh wrtg/install.sh
```

Тот же бандл и тот же `install.sh`, что у `bootstrap.sh`. С рабочей машины ставится по SSH из распакованного бандла или клона: `ROUTER=root@<роутер> SKIP_BUILD=1 sh install.sh`.

### Обновление

- **Само.** Раз в сутки, по умолчанию в 06:00 по часам роутера, `auto-update.sh` проверяет GitHub и ставит новую версию тем же путём, что кнопка Update: sha256, сохранение конфига, перезапуск службы. Выключатель и время: **Settings → Автообновление** или `WRTG_AUTO_UPDATE`, `WRTG_AUTO_UPDATE_TIME`. Итог последнего запуска виден там же, на странице статуса и в syslog строкой `wrtg: auto-update: …`.
- **LuCI:** Status → **Check for updates / Update**.
- **CLI:** `/etc/wrtg/check-update.sh update`.

Конфиг и файл обучения DC при обновлении остаются.

### Удаление

```sh
sh uninstall.sh        # FORCE=1 пропускает вопросы
```

---

## Настройка

Файл `/etc/wrtg/config`, в LuCI страница **Settings**. Применение:

| Команда | Что применяет |
|---|---|
| `/etc/init.d/wrtg reload` | Фронт, домены, обучение DC, `WRTG_SKIP_SRC`, `LAN_IF`, расписание cron. SIGHUP демону и атомарная пересборка nft, сессии не рвутся. |
| `/etc/init.d/wrtg restart` | `LISTEN`, токен Worker, пулы и остальной тюнинг: демон читает их из окружения при старте. |
| `/etc/wrtg/update-cidr.sh` | Только список сетей Telegram и nft. |

### Что обещает версия

| Уровень | Переменные | Обещание |
|---|---|---|
| Стабильные | `FRONT_IP`, `WRTG_FRONT_DCS`, `WRTG_DC_IPS`, `CF_WORKER_DOMAIN`, `WRTG_CF_WORKER_TOKEN`, `CF_PROXY_DOMAIN`, `WRTG_CFPROXY_AUTO`, `WRTG_NO_CFPROXY`, `WRTG_NO_WORKER_PASSTHROUGH`, `WRTG_SKIP_SRC`, `LISTEN`, `ROUTER_IP`, `LAN_IF` | Имя и смысл держатся в пределах 1.x. Переименование только в мажорной версии, после релиза, где старое имя ещё работает и предупреждает. |
| Тюнинг | таймауты, пулы, cooldown'ы, `WRTG_CFPROXY_*`, `WRTG_WS_*`, `WRTG_CF_WORKER_*`, `WRTG_DOH_CACHE_SEC` | Имена держатся, умолчания могут меняться в минорных версиях по замерам на живых сетях. Заданное вами значение остаётся. |
| Внутренние | `WRTG_STATS_SOCKET`, `WRTG_DC_LEARN_FILE`, `WRTG_DC_IPS_FILE`, `WRTG_CONFIG_FILE`, `RUST_LOG` | Могут измениться в любом релизе. |

Формат `--stats` не заморожен: читайте его по имени строки, а не по позиции.

### Основное

| Переменная | Описание | По умолчанию |
|---|---|---|
| `ROUTER_IP` | LAN IP роутера, цель DNAT | определяется при установке |
| `LAN_IF` | Интерфейсы LAN для перехвата, через пробел | `br-lan` |
| `LISTEN` | Адрес демона | `0.0.0.0:8443` |
| `FRONT_IP` | Фронт Telegram для WebSocket и TCP fallback | `149.154.167.220` |
| `WRTG_FRONT_DCS` | Каким DC ходить через фронт: `2,4`, `all`, `none` или список | `2,4` |
| `DC{N}_FRONT_IP` | Свой фронт для одного DC, важнее `WRTG_FRONT_DCS` | пусто |
| `WRTG_DC_IPS` | То же одной строкой: `1:ip,2:ip` | пусто |
| `WRTG_SKIP_SRC` | Хосты LAN (IP или CIDR через пробел), которые DNAT не трогает. Для клиента со своим обходом DPI | пусто |
| `WRTG_LANG` | Язык LuCI: `auto`, `en`, `ru` | `auto` |
| `WRTG_AUTO_UPDATE` | Ежедневное автообновление, `0` выключает | `1` |
| `WRTG_AUTO_UPDATE_TIME` | Время запуска, ЧЧ:ММ по часам роутера | `06:00` |
| `CIDR_URL` | Источник списка сетей Telegram | `https://core.telegram.org/resources/cidr.txt` |
| `CIDR_UPDATE_HOUR` | Час ежедневного обновления списка | `4` |

Обучение DC. Клиенты, которые пишут номер DC в рукопожатии, учат демон, какому DC принадлежит адрес; клиентам без номера по тому же адресу подставляется выученный DC. Файлы: `/etc/wrtg/dc-ips-learned.txt` пишет демон, первая запись про адрес остаётся; `/etc/wrtg/dc-ips.txt` ваш и важнее. Формат `<IP> <DC> [media]`, пути меняются через `WRTG_DC_LEARN_FILE` и `WRTG_DC_IPS_FILE`.

### Cloudflare

| Переменная | Описание | По умолчанию |
|---|---|---|
| `CF_WORKER_DOMAIN` | Хосты Worker через запятую | пусто |
| `WRTG_CF_WORKER_TOKEN` | Тот же секрет, что `WRTG_TOKEN` в Worker | пусто |
| `CF_PROXY_DOMAIN` | Свои домены за Cloudflare через запятую | пусто |
| `WRTG_CFPROXY_AUTO` | Общий пул доменов, `0` выключает | включён, пока нет своего `CF_PROXY_DOMAIN` |
| `WRTG_CFPROXY_DOMAINS_URL` | Откуда обновлять список пула; зеркало, если провайдер режет `raw.githubusercontent.com` | список Flowseal/tg-ws-proxy |
| `WRTG_CFPROXY_REFRESH_SEC` | Период обновления списка, не меньше 300 | `3600` |
| `WRTG_CFPROXY_MAX_ATTEMPTS` | Доменов пула на сессию: первый последовательно, остальные гонкой | `3` |
| `WRTG_CFPROXY_PARALLEL` | Одновременных дозвонов до CF Proxy на весь демон | `2` |
| `WRTG_CFPROXY_429_COOLDOWN_SEC`, `WRTG_CFPROXY_429_MAX_COOLDOWN_SEC` | Пауза после HTTP 429 от домена пула, с удвоением до максимума | `45`, `300` |
| `WRTG_CF_WORKER_429_COOLDOWN_SEC`, `WRTG_CF_WORKER_429_MAX_COOLDOWN_SEC` | То же для Worker; квота бесплатного плана сбрасывается в 00:00 UTC | `60`, `900` |
| `WRTG_NO_CFPROXY` | Выключить ступени Worker и CF Proxy целиком | пусто |
| `WRTG_NO_WORKER_PASSTHROUGH` | Не туннелировать TLS и media через Worker | пусто |

### Тюнинг

| Переменная | Описание | По умолчанию |
|---|---|---|
| `WRTG_WS_POOL_SIZE` | Тёплых WebSocket на каждый DC с фронтом, не больше 8 | `2` |
| `WRTG_WS_POOL_TTL_SEC` | Срок жизни соединения в пуле | `120` |
| `WRTG_CF_WORKER_POOL_SIZE` | Тёплых соединений через Worker на пару DC и media, не больше 4 | `2` |
| `WRTG_CF_WORKER_POOL_TTL_SEC` | Срок жизни | `120` |
| `WRTG_WS_BLACKLIST_TTL_SEC` | Пауза для DC после HTTP 302 на всех его WebSocket-хостах | `2700` |
| `WRTG_IP_FAIL_COOLDOWN_SEC` | Пауза для адреса после таймаута WebSocket или TCP fallback | `3600` |
| `WRTG_DC_FAIL_COOLDOWN_SEC` | Сколько держать короткий таймаут для DC после неудачи | `60` |
| `WRTG_WS_FAIL_TIMEOUT_SEC`, `WRTG_WS_FAIL_TIMEOUT_FAST_SEC` | Таймаут подключения WebSocket: обычный и после неудачи | `5`, `2` |
| `WRTG_FRONTING_SNI` | SNI для TLS fronting; пусто выключает ступень | пусто |
| `WRTG_FRONTING_COOLDOWN_SEC` | Пауза после неудачи fronting | `1800` |
| `WRTG_DOH_CACHE_SEC` | Кэш DNS-over-HTTPS для доменов CF Proxy | `300` |
| `WRTG_WS_PING_SEC` | Ping простаивающего WebSocket | `30` |
| `WRTG_TCP_KEEPALIVE_SEC` | TCP keepalive на сокетах ретрансляции | `30` |
| `WRTG_MAX_CONNS` | Одновременных сессий | `1024` |
| `WRTG_SESSION_IDLE_SEC` | Сессия без байтов в обе стороны закрывается через столько секунд, `0` выключает | `600` |
| `WRTG_STATS_SOCKET` | Unix-сокет для `wrtg --stats` | `/var/run/wrtg.sock` |
| `RUST_LOG` | `debug` включает подробный лог | `info` |

Пулы греются по спросу: только слоты, к которым обращались за последние 10 минут. Дозвон до Cloudflare ограничен 4 секундами, здоровый занимает около 0,2 с.

---

## CF Worker

Worker обслуживает DC1, DC3 и DC5 и туннелирует TLS и media к реальным адресам Telegram. Исходник: `openwrt/cf-worker.js`.

1. [Cloudflare Dashboard](https://dash.cloudflare.com) → **Workers & Pages** → **Create Worker**.
2. **Edit code**, вставьте `openwrt/cf-worker.js`, **Deploy**. В LuCI на странице Documentation код показан с кнопкой копирования.
3. **Settings → Variables and Secrets**: секрет `WRTG_TOKEN`, например `openssl rand -hex 32`. Без него Worker отвечает 503 на любой запрос: раньше он в этом случае становился открытым реле, которое находили сканеры и сжигали квоту.
4. На роутере:

```sh
CF_WORKER_DOMAIN="name.username.workers.dev"
WRTG_CF_WORKER_TOKEN="<то же значение>"
/etc/init.d/wrtg restart
```

Worker пропускает только IPv4 из сетей Telegram на порты 80, 443 и 5222. Несколько Worker перечисляются через запятую, демон обходит их по кругу.

---

## CF Proxy

WebSocket через домен за Cloudflare CDN: `wss://kws{N}.<домен>/apiws`. Последняя ступень перед blind relay и единственный путь DC1, DC3 и DC5 без своего Worker.

Свой домен: `CF_PROXY_DOMAIN="proxy.example.com"`, затем `reload`. Общий пул работает, пока своего домена нет: 20 доменов зашиты в бинарник, раз в час список обновляется из Flowseal/tg-ws-proxy. Домены чужие, и через них пойдёт трафик DC1, DC3 и DC5. `WRTG_CFPROXY_AUTO="0"` выключает пул, тогда эти DC уходят в blind relay на заблокированный адрес, и демон предупредит об этом при старте.

Как демон выбирает домен. На сессию уходит не больше трёх: первый последовательно, остальные параллельной гонкой. Сработавший домен закрепляется за DC и идёт первым дальше. Если провалилась вся тройка, курсор DC сдвигается по списку, и следующая сессия пробует другие домены. Часовое обновление списка курсор и закреплённый домен не сбрасывает. Здоровье пула меняется за минуты, 503 отдаёт сам Worker, когда у него не отвечает апстрим, поэтому демон ничего не выводит из ротации надолго. В `--stats` это секция `cf proxy` и счётчик `cf_proxy_dial_failed`, в LuCI карточка **CF Proxy** на странице статуса.

---

## Диагностика

```sh
/etc/init.d/wrtg status
wrtg --check                       # DNS и WSS по каждому DC его реальным путём
wrtg --stats                       # счётчики, пулы, закреплённые домены, живая куча
logread -e wrtg | tail
nft list table inet tg_tproxy      # правила DNAT и множество telegram_cidr
```

`--check` резолвит хосты Worker и первые три домена пула через `kws2.` (у базовых доменов пула A-записи нет), затем пробует WSS-рукопожатие для каждого DC его реальным путём: DC с фронтом напрямую, остальные через первый Worker или через до трёх доменов пула, первый ответивший засчитывается. Демон при этом не запускается, конфиг читается из `/etc/wrtg/config`. Провал фронта на закрытой сети говорит о сети, а не о поломке.

### Как читать `--stats`

| Строка | Значение |
|---|---|
| `active` близко к `capacity` | Семафор сессий заканчивается |
| `all_paths_failed` | Сессии, где не сработала ни одна ступень. Долю считайте от `ws_pool_hit + ws_direct + cf_proxy + tcp_fallback + all_paths_failed`; `blind_relay` растёт и на обычном не-MTProto трафике |
| `cf_proxy` = 0 при живом трафике | Ступень не используется; в строке старта смотрите `cf-proxies=N` |
| `cf_proxy_dial_failed` сравнимо с `cf_proxy` | Пул мигает; кто отвечает 503, покажет `logread -e 'CF proxy'` |
| `media_http_rejected` | Клиент тянет media по HTTP на :80, фронт отвечает редиректом, клиент повторяет по :443. Лишние круги, не отказы |
| `passthrough_no_data` | Worker поднял туннель, но до DC ничего не дошло |
| `cf proxy`, строки `DCn sticky` | Закреплённый домен пула по DC |
| `heap live` растёт вместе с RSS | Утечка в демоне, класс размера подскажет чья. RSS растёт, а `heap live` стоит: страницы держит аллокатор musl, это не утечка |

### Симптомы

| Симптом | Что проверить |
|---|---|
| Telegram не подключается, в `--stats` `accepted 0` | Пустое множество `telegram_cidr` или нет DNAT: `nft list table inet tg_tproxy`, `ROUTER_IP`, `LAN_IF`, `/etc/wrtg/update-cidr.sh` |
| DC1, DC3, DC5 в blind relay | Пул выключен и Worker не задан; в строке старта `cf-workers=0, cf-proxies=0` |
| `cf-workers=0` при заданном `CF_WORKER_DOMAIN` | Опечатка в домене, после правки `restart` |
| Worker отвечает 503 или 403 | Не задан `WRTG_TOKEN`; секреты не совпадают или адрес вне сетей Telegram |
| Сессия висит 10 с и уходит в blind relay | Провайдер закрыл адреса Telegram, а пул не ответил. Адрес попадает в `ip_fail`, следующие сессии ждут 2 с |
| `blind_relay` во много раз выше `cf_proxy` | Клиент со своим обходом DPI (zapret, byedpi) за роутером: его приманки доходят до Telegram как данные и остаются без ответа. Исключите его: **Settings → Перехват** или `WRTG_SKIP_SRC` |
| Media не грузятся | Нужен Worker с passthrough: `CF_WORKER_DOMAIN` задан, `WRTG_NO_WORKER_PASSTHROUGH` не выставлен |
| Обновление не скачивается | Провайдер режет Fastly: [офлайн-установка](#офлайн-установка) |

---

## Ограничения

- Звонки не проксируются: UDP и WebRTC идут мимо, wrtg берёт только TCP.
- Только IPv4: `SO_ORIGINAL_DST` для IPv6 не реализован.
- Worker обновляется руками: новый `cf-worker.js` вы выкладываете в Cloudflare сами.
- Общий пул CF Proxy держится на чужих доменах. По замерам рабочих 6–16 из 20 в зависимости от DC и часа. Свой Worker или домен надёжнее.
- Провайдер может закрыть Telegram целиком. Тогда всё держится на Worker и пуле; если молчат и они, сессия уходит в blind relay, и связи нет.

---

## Как это работает

nftables (таблица `inet tg_tproxy`, chain `prerouting`) перехватывает TCP на порты 80, 443 и 5222 к сетям Telegram из `/var/lib/wrtg/cidrs.txt` (официальный список плюс `/etc/wrtg/cidr-extra.txt`) и делает DNAT на `ROUTER_IP:8443`. Демон восстанавливает исходный адрес через `SO_ORIGINAL_DST`, разбирает рукопожатие MTProto (obfuscated2, 64 байта) и ведёт сессию по цепочке:

1. **Пул WebSocket** на фронт: готовое соединение, только DC с фронтом и только не-media.
2. **Новый WebSocket** на `FRONT_IP` или реальный адрес DC.
3. **TLS fronting**, только при `WRTG_FRONTING_SNI`.
4. **CF Worker**: пул, затем новое соединение; несколько Worker по кругу.
5. **CF Proxy**: до трёх доменов, первый последовательно, остальные гонкой.
6. **TCP fallback** на фронт или CDN media: 10 с на подключение, 2 с если адрес уже в `ip_fail`.
7. **Blind relay**: байты как есть, сначала через Worker, потом на фронт. Сюда же идёт всё, что не MTProto: TLS и HTTP.

Ступени 1–3 пропускаются, пока для DC действует `ws_blacklist` (HTTP 302 на всех хостах) или `ip_fail` (таймаут к адресу). Каждая ступень считается в `--stats`.

Файлы: `/usr/sbin/wrtg`, `/etc/wrtg/config`, `/etc/wrtg/*.sh`, `/etc/init.d/wrtg` (procd, START=95), `/var/lib/wrtg/cidrs.txt`, две строки wrtg в `/etc/crontabs/root`.

---

## Разработчикам

Гейты те же, что в CI:

```sh
cargo fmt --all -- --check
cargo clippy -p wrtg --all-targets -- -D warnings
cargo test -p wrtg
shellcheck -x -e SC1091,SC2029,SC3043 install.sh bootstrap.sh uninstall.sh build-rust.sh \
  openwrt/*.sh openwrt/wrtg.init openwrt/luci-app-wrtg/install-luci.sh
node --check openwrt/cf-worker.js
sh build-rust.sh arm64          # cargo-zigbuild; amd64, arm, mipsel так же
```

Изменения в цепочке fallback меряйте на живом роутере: `ROUTER=root@<ip> sh tools/ab-compare.sh <сборка A> <сборка B> 600` ставит обе сборки по очереди под тот же трафик и сравнивает счётчики. Запускайте днём, когда `all_paths_failed` шевелится, иначе вердикт INCONCLUSIVE.

CI: `.github/workflows/build.yml` гоняет гейты и собирает все архитектуры; `release.yml` делает релиз по тегу `v*`: бинарники, бандл, SHA256SUMS, версия в README.

---

## Лицензия

[MIT](LICENSE)
