VERSION := $(shell cat VERSION 2>/dev/null || echo dev)

.PHONY: all build install install-amd64 bundle clean dist-amd64 dist-arm64 dist-arm dist-mipsel rust rust-amd64 rust-arm64 rust-arm rust-mipsel

all: rust

build: rust

rust: rust-amd64 rust-arm64 rust-arm rust-mipsel

rust-amd64:
	sh build-rust.sh amd64

rust-arm64:
	sh build-rust.sh arm64

rust-arm:
	sh build-rust.sh arm

rust-mipsel:
	sh build-rust.sh mipsel

dist-amd64: rust-amd64
dist-arm64: rust-arm64
dist-arm: rust-arm
dist-mipsel: rust-mipsel

install: rust
	sh install.sh

install-amd64: rust-amd64
	SKIP_BUILD=1 sh install.sh

bundle: rust
	mkdir -p bundle/wrtg/dist dist
	cp -r install.sh bootstrap.sh build-rust.sh VERSION README.md openwrt bundle/wrtg/
	cp dist/wrtg-linux-amd64 dist/wrtg-linux-arm64 dist/wrtg-linux-arm dist/wrtg-linux-mipsel bundle/wrtg/dist/
	tar -czf dist/wrtg-openwrt.tar.gz -C bundle wrtg
	cd dist && sha256sum wrtg-linux-* wrtg-openwrt.tar.gz > SHA256SUMS

clean:
	rm -rf dist target bundle
