# CF Proxy fallback

CF Proxy — дополнительный WSS fallback через собственный Cloudflare-proxied
домен. Предпочтительный вариант для wrtg — собственный
[CF Worker](CF_WORKER_SETUP.md).

## Собственный домен

Укажите один или несколько доменов:

```sh
CF_PROXY_DOMAIN="proxy.example.com"
/etc/init.d/wrtg restart
```

wrtg подключается к `wss://kws{N}[-1].proxy.example.com/apiws`.

## Публичный pool

Автозагрузка списка Flowseal выключена по умолчанию: публичные домены не
контролируются wrtg, часто устаревают и могут создавать длинные задержки.

Явное включение:

```sh
WRTG_CFPROXY_AUTO="1"
/etc/init.d/wrtg restart
```

На одно соединение проверяется не более трёх доменов. Список обновляется раз в
час и применяется только после валидации минимум трёх доменов.

## Диагностика

```sh
nslookup kws1.proxy.example.com
curl -i https://kws1.proxy.example.com/apiws
logread -e wrtg | grep -i 'CF proxy'
```

Сертификат TLS обязан быть действительным. При нестабильном публичном pool
выключите `WRTG_CFPROXY_AUTO` и используйте свой Worker или домен.

wrtg не требует zapret; сначала проверяйте DNS и HTTPS/WSS connectivity.
