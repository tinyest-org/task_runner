FROM task-builder

RUN apk add --no-cache libpq-dev

COPY ./migrations /app/migrations
COPY ./src /app/src
# do a release build
RUN cargo build --release --bin server

# use a plain alpine image, the alpine version needs to match the builder
FROM alpine:3.21
# if needed, install additional dependencies here
RUN apk add --no-cache libgcc libpq-dev
# copy the binary into the final image
COPY --from=0 /app/target/release/server server
# set the binary as entrypoint
ENTRYPOINT ["/server"]

EXPOSE 4000