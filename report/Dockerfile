# Alpine's Rust targets musl natively, so the release binary is fully static and
# the runtime image is nothing but that binary.
FROM rust:1-alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY report ./report
# The workspace root is the statusline itself; a stub keeps cargo from wanting
# its sources (and libgit2) for a build that only needs the report.
RUN mkdir -p src && echo 'fn main() {}' > src/main.rs
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p claude-statusline-report \
    && cp target/release/claude-statusline-report /claude-statusline-report

FROM scratch
COPY --from=build /claude-statusline-report /claude-statusline-report
ENV DB_PATH=/data/history.db PORT=8080
EXPOSE 8080
ENTRYPOINT ["/claude-statusline-report"]
