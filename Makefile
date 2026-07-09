VERSION := $(shell cat VERSION 2>/dev/null || echo dev)

.PHONY: all build install clean dist-amd64 dist-arm64 dist-arm rust rust-amd64 rust-arm64 rust-arm

all: rust

build: rust

rust: rust-amd64 rust-arm64

rust-amd64:
	sh build-rust.sh amd64

rust-arm64:
	sh build-rust.sh arm64

rust-arm:
	sh build-rust.sh arm

dist-amd64: rust-amd64
dist-arm64: rust-arm64
dist-arm: rust-arm

install: rust
	sh install.sh

clean:
	rm -rf dist target
