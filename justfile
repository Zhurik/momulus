# Development tasks for momulus.

default:
    @just --list

# Formatting
fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

# Lint: warnings are errors
lint:
    cargo clippy --all-targets --all-features -- -D warnings

# Fast tests (no Docker, no network)
test: fmt-check lint
    cargo test --all-features

# Tests that need Docker and a real pi
test-integration:
    cargo test --all-features -- --ignored --nocapture

# Build the images
build-images:
    docker build -f docker/Dockerfile.runner -t momulus-runner:latest .
    docker build -f docker/Dockerfile -t momulus:latest .

# Install the git hooks (pre-commit and pre-push)
hooks:
    pre-commit install --install-hooks
    pre-commit install --hook-type pre-push

# Run every hook over the whole tree
hooks-run:
    pre-commit run --all-files

# Validate the skill contracts
skills-validate:
    cargo run -q -p momulus -- skills validate

# Run a skill on a local directory: just try proofread ~/Projects/blog file.md
try skill repo files="":
    cargo run -q -p momulus -- run --repo-path {{repo}} --skill {{skill}}         {{ if files == "" { "" } else { "--files " + files } }} --out ./out

# Bring the service up with compose (reads .env from this directory)
up:
    @test -f .env || (echo "no .env yet: cp .env.example .env and fill it in" && exit 1)
    mkdir -p data
    docker compose --profile build build
    docker compose up -d
    @echo "started; follow the logs with: docker compose logs -f momulus"

down:
    docker compose down
