# Two stages on the same Debian the LXCs run (T8): a glibc binary that
# also works copied out of the image. The runtime stage has no shell
# tools, so the container HEALTHCHECK uses the binary's own --healthcheck.
FROM rust:1.97-slim-trixie AS build
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY . .
RUN cargo build --release --locked

FROM debian:trixie-slim
RUN apt-get update -qq && apt-get install -y -qq --no-install-recommends ca-certificates libssl3t64 && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home /var/lib/almanac --shell /usr/sbin/nologin almanac \
    && mkdir -p /var/lib/almanac && chown almanac:almanac /var/lib/almanac
COPY --from=build /src/target/release/almanac /usr/local/bin/almanac
USER almanac
ENV ALMANAC_LISTEN=0.0.0.0:8080 ALMANAC_STATE_DIR=/var/lib/almanac
EXPOSE 8080
VOLUME ["/var/lib/almanac"]
# Self-update is off inside an image by detection (AR8); updates are a new image.
HEALTHCHECK --interval=30s --timeout=5s --retries=3 CMD ["/usr/local/bin/almanac", "--healthcheck"]
ENTRYPOINT ["/usr/local/bin/almanac"]
