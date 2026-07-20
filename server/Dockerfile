# Build context is the workspace root (see .dockerignore) so this stage can
# see Cargo.lock and the `protocol` path-dependency `server` needs — but we
# only ever build the `server` package, never `client` (wasm target, a much
# heavier and irrelevant dependency tree for this deploy).
FROM rust:1-slim-bookworm AS build

WORKDIR /workspace
COPY Cargo.toml Cargo.lock ./
COPY protocol ./protocol
COPY server ./server

# The workspace Cargo.toml lists `client` as a member; without a client/
# directory `cargo` would fail to resolve the workspace. A minimal stub
# satisfies the manifest without pulling in any of the client's real
# dependencies (bevy, osmpbf, etc.) or building it.
RUN mkdir -p client/src && \
    printf '[package]\nname = "browser-flight-sim"\nversion = "0.1.0"\nedition = "2024"\n' > client/Cargo.toml && \
    printf 'fn main() {}\n' > client/src/main.rs

RUN cargo build --release -p server

# ---

FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /workspace/target/release/server /usr/local/bin/server

# DigitalOcean App Platform sets $PORT; the server reads it at startup
# (falling back to 7777 for local `docker run`/dev parity).
ENV PORT=7777
EXPOSE 7777

CMD ["server"]
