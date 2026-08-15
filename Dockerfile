# syntax=docker/dockerfile:1
# a2a-switchboard — multi-stage build; the runtime image carries ONLY the
# binary + CA certs (rust-embed compiles templates/assets into the binary).

FROM rust:1.97 AS build
WORKDIR /src

# Dependency layer cache: build with a stub binary first, then copy real source.
COPY Cargo.toml Cargo.lock ./
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs \
    && cargo build --release
RUN rm -rf src

COPY src ./src
COPY templates ./templates
COPY assets ./assets
COPY config.toml.example ./
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/a2a-switchboard /usr/local/bin/a2a-switchboard

VOLUME ["/data"]
ENV AGW_DATA_DIR=/data
EXPOSE 9920

ENTRYPOINT ["/usr/local/bin/a2a-switchboard"]
