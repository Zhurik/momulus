# Образ сервиса momulus. Сборка кэширует зависимости через cargo-chef.
FROM rust:1.98-slim-trixie AS chef
WORKDIR /build
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config \
    && rm -rf /var/lib/apt/lists/* \
    && cargo install cargo-chef --locked --version ^0.1

# Считаем рецепт зависимостей: он меняется только при правке Cargo.toml.
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Собираем зависимости отдельным слоем, потом сам сервис.
FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --locked -p momulus \
    && strip target/release/momulus

FROM debian:trixie-slim AS runtime
# git нужен для рабочих копий, ca-certificates — для HTTPS к API.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        git \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 10001 momulus

COPY --from=builder /build/target/release/momulus /usr/local/bin/momulus

# Каталог данных монтируется извне; путь совпадает с хостовым (см. README).
USER momulus
WORKDIR /home/momulus
ENTRYPOINT ["/usr/local/bin/momulus"]
CMD ["serve"]
