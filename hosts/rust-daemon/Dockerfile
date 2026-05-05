# Build with the repository root as context:
# docker build -f hosts/rust-daemon/Dockerfile -t anima-daemon:staging .

FROM cgr.dev/chainguard/rust:latest-dev AS builder

WORKDIR /workspace

COPY Cargo.toml Cargo.lock ./
COPY packages/core-rust ./packages/core-rust
COPY hosts/rust-daemon ./hosts/rust-daemon

RUN cargo build --release --locked -p anima-daemon

FROM cgr.dev/chainguard/wolfi-base:latest AS runtime

RUN apk add --no-cache ca-certificates libgomp libstdc++

RUN addgroup -S anima && adduser -S -D -H -s /sbin/nologin -u 10001 -G anima anima
WORKDIR /app

COPY --from=builder /workspace/target/release/anima-daemon /usr/local/bin/anima-daemon

ENV ANIMAOS_RS_HOST=0.0.0.0
ENV ANIMAOS_RS_PORT=8080

EXPOSE 8080

USER anima

CMD ["/usr/local/bin/anima-daemon"]