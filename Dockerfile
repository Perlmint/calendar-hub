FROM node:alpine as web_builder

WORKDIR /src
ENV PRODUCTION 1
ADD \
    package.json package-lock.json \
    tsconfig.json webpack.config.js \
    ./

ADD src ./src

RUN npm i && npx webpack build

FROM rust:1.70-alpine as builder

RUN apk add --no-cache \
        musl-dev \
        ca-certificates \
        openssl-dev \
        openssl-libs-static && \
    update-ca-certificates

ENV CARGO_REGISTRIES_CRATES_IO_PROTOCOL=sparse
RUN cargo install sqlx-cli --no-default-features --features sqlite,sqlx/runtime-tokio-rustls

WORKDIR /src

ADD \
    Cargo.toml \
    .env \
    ./
ADD src/ ./src
ADD migrations/ ./migrations

ENV PKG_CONFIG_ALL_STATIC=1 \
    OPENSSL_STATIC=1 \
    OPENSSL_LIB_DIR=/usr/lib/ \
    OPENSSL_INCLUDE_DIR=/usr/include/
RUN sqlx database create && sqlx migrate run

COPY --from=web_builder /src/dist ./dist
RUN cargo build --release --features embed_web

FROM scratch

COPY --from=builder /src/target/release/calendar-hub /calendar-hub
COPY --from=builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/

EXPOSE 3000
ENTRYPOINT [ "/calendar-hub" ]
