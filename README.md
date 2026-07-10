# wrtg

Прозрачный TCP-прокси Telegram для OpenWrt. nftables перенаправляет трафик к IP Telegram на локальный демон, который мостит MTProto через WebSocket и Cloudflare fallback. Клиентам прокси настраивать не нужно.

## Установка

```bash
wget -qO- https://github.com/onebany/wrtg/raw/branch/main/bootstrap.sh | sh
```

## Дальше

Всё остальное — в **[docs/GUIDE.md](docs/GUIDE.md)**: архитектура, настройка, CF Worker, диагностика, ограничения.

Релизы: [Gitea](https://github.com/onebany/wrtg/releases) · [GitHub](https://github.com/onebany/wrtg/releases)
