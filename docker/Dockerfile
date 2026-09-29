# The momulus service image. Dependencies are cached through cargo-chef.
FROM rust:1.98-slim-trixie AS chef
WORKDIR /build
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && cargo install cargo-chef --locked --version ^0.1

# Compute the dependency recipe: it only changes when a Cargo.toml changes.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Build the dependencies as a separate layer, then the service itself.
FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p momulus \
    && strip target/release/momulus

FROM debian:trixie-slim AS runtime
# git is needed for working copies, ca-certificates for HTTPS to the API.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 momulus

COPY --from=builder /build/target/release/momulus /usr/local/bin/momulus

# The data directory is mounted from outside; its path matches the host (see README).
USER momulus
WORKDIR /home/momulus
ENTRYPOINT ["/usr/local/bin/momulus"]
CMD ["serve"]
