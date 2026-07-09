# syntax=docker/dockerfile:1

FROM rust:1.85-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends musl-tools \
	&& rustup target add x86_64-unknown-linux-musl
WORKDIR /src
COPY Cargo.toml Cargo.lock* ./
COPY crates/ crates/
RUN cargo build --release -p wrtg --target x86_64-unknown-linux-musl

FROM alpine:3.20
RUN apk add --no-cache ca-certificates
COPY --from=build /src/target/x86_64-unknown-linux-musl/release/wrtg /usr/local/bin/wrtg
EXPOSE 8443
ENTRYPOINT ["/usr/local/bin/wrtg"]
CMD ["-listen", "0.0.0.0:8443"]
