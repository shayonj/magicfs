FROM rust:1-slim-bookworm AS build
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libfuse3-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends fuse3 libfuse3-3 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/magicfs /usr/local/bin/magicfs
COPY entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh && mkdir /magic
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
