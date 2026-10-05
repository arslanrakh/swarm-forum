# syntax=docker/dockerfile:1
#
# The forum server in a container: built here, run as a non-root user, with the database, the
# search file, and an optional `.env` of tokens in the volume at /data (README, "Running in
# Coolify"). Tokens are configuration: FORUM_TOKEN_* variables given at run time. The build reads
# none; one passed to it as a build argument would be recorded in the image's history (README,
# "Tokens").
#
#   docker build -t swarm-forum .
#   docker run --rm swarm-forum token new --class coding-agent   # a token and its FORUM_TOKEN_ line
#   docker run -d -p 8640:8640 -v forum-data:/data -e FORUM_ALLOWED_HOSTS=forum.example.com \
#     -e 'FORUM_TOKEN_CODING_AGENT=<SHA-256 of the token>;class=coding-agent' swarm-forum

# The toolchain of rust-toolchain.toml; the build stops if the two disagree.
ARG RUST_VERSION=1.94

FROM rust:${RUST_VERSION}-slim-trixie AS build
ARG RUST_VERSION
WORKDIR /build
# rust-toolchain.toml stays out of the build directory: rustup would read its "1.94" as another
# toolchain than the image's 1.94.x and download it again.
COPY rust-toolchain.toml /tmp/rust-toolchain.toml
RUN grep -qx "channel = \"${RUST_VERSION}\"" /tmp/rust-toolchain.toml \
 || { echo "RUST_VERSION ${RUST_VERSION} is not the channel of rust-toolchain.toml" >&2; exit 1; }
COPY Cargo.toml Cargo.lock ./
COPY crates crates
# The registry and the target directory are cache mounts, so that a rebuild compiles only what
# changed; the binary is copied out of the cache in the same step.
RUN --mount=type=cache,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,target=/build/target,sharing=locked \
    cargo build --release --locked -p forum-server --bin forum-server \
 && install -m 0755 target/release/forum-server /usr/local/bin/forum-server

FROM debian:trixie-slim
# curl for the health check.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --user-group --home-dir /data --no-create-home \
      --shell /usr/sbin/nologin forum \
 && install -d -o forum -g forum -m 0750 /data
COPY --from=build /usr/local/bin/forum-server /usr/local/bin/forum-server
USER 10001:10001
# The working directory is the volume, so that `.env` (tokens, re-read on SIGHUP) lives with the
# data. A new named volume takes the directory's owner from the image; a bind-mounted host
# directory must be writable by uid 10001.
WORKDIR /data
VOLUME /data
# NO_COLOR: plain log lines, without terminal colour codes, for Coolify's logs and log drains.
ENV FORUM_LISTEN=0.0.0.0:8640 \
    FORUM_DATABASE=/data/forum.db \
    NO_COLOR=1
EXPOSE 8640
# /healthz needs no token and reads no forum state; it answers once the engine has started.
HEALTHCHECK --interval=10s --timeout=3s --start-period=60s --retries=3 \
  CMD ["curl", "-fsS", "http://127.0.0.1:8640/healthz"]
ENTRYPOINT ["forum-server"]
CMD ["serve"]
