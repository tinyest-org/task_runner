# Start with a rust alpine image
FROM rust:1-alpine3.19
# This is important, see https://github.com/rust-lang/docker-rust/issues/85
ENV RUSTFLAGS="-C target-feature=-crt-static"
# if needed, add additional dependencies here
RUN apk add --no-cache musl-dev curl
# set the workdir and copy the source into it
WORKDIR /app
COPY ./Cargo.toml /app
COPY ./Cargo.lock /app
COPY rust-toolchain.toml /app
COPY ./src/cache_helper.rs /app/src/cache_helper.rs

RUN cargo build --release --bin cache

