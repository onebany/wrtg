# Cloudflare Worker для wrtg

**Версия:** wrtg 0.5.0+  
**Исходник Worker:** [`openwrt/cf-worker.js`](../openwrt/cf-worker.js)

Worker обеспечивает WSS/TCP fallback к Telegram DC, когда direct WS недоступен
или возвращает HTTP 302. Для media passthrough он поддерживает `port=80|443|5222`.

## Безопасность

Worker 0.5.0:

- принимает только IPv4 из Telegram CIDR;
- разрешает только TCP 80, 443 и 5222;
- поддерживает secret `WRTG_TOKEN`;
- сериализует TCP-записи и корректно закрывает socket.

Старый Worker без этих ограничений является публичным TCP proxy. Его необходимо
заменить кодом из `openwrt/cf-worker.js`.

## Развёртывание

1. Cloudflare Dashboard → **Workers & Pages** → **Create Worker**.
2. Откройте **Edit code**, замените шаблон содержимым
   [`openwrt/cf-worker.js`](../openwrt/cf-worker.js) и нажмите **Deploy**.
3. В **Settings → Variables and Secrets** создайте encrypted secret:

   ```
   WRTG_TOKEN=<длинная случайная строка>
   ```

   Сгенерировать можно командой `openssl rand -hex 32`.
4. Скопируйте hostname вида `name.username.workers.dev`.
5. На роутере добавьте:

   ```sh
   CF_WORKER_DOMAIN="name.username.workers.dev"
   WRTG_CF_WORKER_TOKEN="<то же значение, что WRTG_TOKEN>"
   /etc/init.d/wrtg restart
   ```

Несколько Worker задаются через запятую. Порядок сохраняется:

```sh
CF_WORKER_DOMAIN="worker1.user.workers.dev,worker2.user.workers.dev"
```

## Проверка

DNS и HTTPS:

```sh
nslookup name.username.workers.dev
curl -i https://name.username.workers.dev/apiws
```

Ожидаемый HTTP-ответ без WebSocket Upgrade: `426 Expected websocket`.

После открытия Telegram:

```sh
logread -e wrtg | grep -E 'CF worker|worker passthrough'
```

Успешные варианты:

```text
DC1 -> WS connected via CF worker name.username.workers.dev
passthrough via CF worker name.username.workers.dev -> 149.154.x.x:80
```

## Troubleshooting

- `cf-workers=0`: проверьте `CF_WORKER_DOMAIN` и выполните restart.
- HTTP 403: Worker secret и `WRTG_CF_WORKER_TOKEN` не совпадают либо destination
  не входит в Telegram CIDR.
- TLS certificate error: проверьте hostname, DNS и время на роутере.
- Timeout: проверьте доступ роутера к `*.workers.dev`.
- После изменения config всегда используйте `/etc/init.d/wrtg restart`
  (`reload` является alias для restart).

wrtg не зависит от zapret; диагностика Worker начинается с DNS и HTTPS/WSS.
