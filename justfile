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
