default: build

build:
    cargo build --workspace

test:
    cargo test --workspace --all-targets

up:
    docker compose -f docker/docker-compose.yml up -d

down:
    docker compose -f docker/docker-compose.yml down

bootstrap:
    bash scripts/bootstrap.sh

logs:
    docker compose -f docker/docker-compose.yml logs -f falkordb

reset-db:
    docker compose -f docker/docker-compose.yml down -v
    docker compose -f docker/docker-compose.yml up -d

fmt:
    cargo fmt --all

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt-check:
    cargo fmt --all -- --check
