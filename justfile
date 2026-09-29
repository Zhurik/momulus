# Задачи разработки llm-bot.

default:
    @just --list

# Форматирование
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

# Линт: предупреждения — ошибки
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Быстрые тесты (без Docker и сети)
test: fmt-check lint
    cargo test --all-features

# Тесты, которым нужен Docker и настоящий pi
test-integration:
    cargo test --all-features -- --ignored --nocapture

# Сборка образов
build-images:
    docker build -f docker/Dockerfile.runner -t llm-bot-runner:latest .
    docker build -f docker/Dockerfile -t llm-bot:latest .

# Проверка контрактов скиллов
skills-validate:
    cargo run -q -p llm-bot -- skills validate

# Прогон скилла на локальной папке: just try proofread ~/Projects/blog file.md
try skill repo files="":
    cargo run -q -p llm-bot -- run --repo-path {{repo}} --skill {{skill}}         {{ if files == "" { "" } else { "--files " + files } }} --out ./out

# Поднять сервис через compose (нужен .env с секретами)
up:
    docker compose --profile build build
    docker compose up -d

down:
    docker compose down
