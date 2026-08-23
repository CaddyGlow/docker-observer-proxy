# syntax=docker/dockerfile:1.7
FROM rust:1.85-alpine AS builder

RUN apk add --no-cache musl-dev
WORKDIR /src

COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN --mount=type=cache,id=dop-cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=dop-cargo-target,target=/src/target \
    cargo build --locked --release && \
    cp target/release/docker-observer-proxy /docker-observer-proxy

FROM scratch
COPY --from=builder /docker-observer-proxy /docker-observer-proxy
USER 65532:65532
EXPOSE 2375
ENTRYPOINT ["/docker-observer-proxy"]
