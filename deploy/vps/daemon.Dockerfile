# ONNX Runtime's prebuilt static library requires glibc's C23 symbols (>=2.38).
FROM rust:1-trixie AS build
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY packages/core-rust ./packages/core-rust
COPY hosts/rust-daemon ./hosts/rust-daemon
RUN cargo build --release --locked -p anima-daemon

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl libgomp1 libstdc++6 libssl3t64 \
    dbus gnome-keyring libsecret-tools \
    && rm -rf /var/lib/apt/lists/*
RUN groupadd --gid 10001 anima && useradd --uid 10001 --gid anima --create-home anima \
    && mkdir -p /state /workspace /home/anima/.local/share/keyrings \
    && chown -R anima:anima /state /workspace /home/anima
COPY --from=build /build/target/release/anima-daemon /usr/local/bin/anima-daemon
COPY --chmod=755 deploy/vps/daemon-entrypoint.sh /usr/local/bin/daemon-entrypoint
ENV HOME=/home/anima XDG_DATA_HOME=/home/anima/.local/share XDG_RUNTIME_DIR=/tmp/anima-runtime
USER 10001:10001
WORKDIR /workspace
ENTRYPOINT ["/usr/local/bin/daemon-entrypoint"]
