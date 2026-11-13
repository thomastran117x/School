FROM rust:1.92-bookworm AS builder

WORKDIR /workspace
RUN apt-get update \
    && apt-get install --yes --no-install-recommends libssl-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*
COPY . .
RUN cargo build --locked --release -p backend-api

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --yes --no-install-recommends ca-certificates curl libssl3 \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 app
COPY --from=builder /workspace/target/release/backend-api /usr/local/bin/backend-api

USER app
EXPOSE 3000
ENTRYPOINT ["backend-api"]

