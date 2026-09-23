# syntax=docker/dockerfile:1
# Static musl build of core-api on a distroless base (no shell, runs as non-root).
FROM rust:1.97-alpine AS build
RUN apk add --no-cache build-base
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p core-api --bin core-api && cp target/release/core-api /core-api

FROM gcr.io/distroless/static-debian12:nonroot
COPY --from=build /core-api /core-api
ENV APP_ENV=production PORT=8080
EXPOSE 8080
ENTRYPOINT ["/core-api"]
CMD ["serve"]
