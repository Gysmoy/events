# Compilación: binario estático (musl) con la versión de Rust fijada
FROM rust:1.100.0-alpine3.24 AS builder

# El servidor compila con poca RAM: se limita el paralelismo (se puede cambiar con --build-arg)
ARG CARGO_BUILD_JOBS=2
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}

RUN apk add --no-cache musl-dev

WORKDIR /app

# Primero solo las dependencias: esta capa queda en caché mientras no cambien Cargo.toml ni Cargo.lock.
# Con el main vacío todas las dependencias salen "sin usar": esos avisos se callan solo en este paso.
COPY Cargo.toml Cargo.lock ./
RUN mkdir src \
    && echo "fn main() {}" > src/main.rs \
    && touch src/lib.rs \
    && CARGO_BUILD_WARNINGS=allow cargo build --release --locked \
    && rm -rf src

# Después el código: touch para que cargo no confunda los fuentes con el stub ya compilado
COPY src ./src
RUN touch src/main.rs src/lib.rs \
    && cargo build --release --locked

# Ejecución: alpine mínimo (busybox wget para el healthcheck), sin root
FROM alpine:3.24

RUN adduser -D -H -u 10001 events

COPY --from=builder /app/target/release/websocket-filter-service /usr/local/bin/websocket-filter-service

USER events

ENV PORT=3000
EXPOSE 3000

CMD ["websocket-filter-service"]
