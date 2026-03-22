FROM rust:1.94-slim AS builder

WORKDIR /app

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY gateway/Cargo.toml gateway/Cargo.toml
COPY mock-provider/Cargo.toml mock-provider/Cargo.toml
COPY loadtest/Cargo.toml loadtest/Cargo.toml

RUN mkdir -p gateway/src mock-provider/src loadtest/src \
    && echo "fn main() {}" > gateway/src/main.rs \
    && echo "fn main() {}" > mock-provider/src/main.rs \
    && echo "fn main() {}" > loadtest/src/main.rs \
    && cargo build --release --workspace \
    && rm -rf gateway/src mock-provider/src loadtest/src

COPY gateway/src gateway/src
COPY mock-provider/src mock-provider/src
COPY loadtest/src loadtest/src
COPY migrations migrations

RUN touch gateway/src/main.rs mock-provider/src/main.rs loadtest/src/main.rs \
    && cargo build --release --workspace

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y ca-certificates libssl3 && rm -rf /var/lib/apt/lists/*

FROM runtime AS gateway
COPY --from=builder /app/target/release/gateway /usr/local/bin/gateway
ENTRYPOINT ["gateway"]

FROM runtime AS mock-provider
COPY --from=builder /app/target/release/mock-provider /usr/local/bin/mock-provider
ENTRYPOINT ["mock-provider"]

FROM runtime AS loadtest
COPY --from=builder /app/target/release/loadtest /usr/local/bin/loadtest
ENTRYPOINT ["loadtest"]
