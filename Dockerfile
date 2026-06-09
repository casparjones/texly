# JS bundle stage
FROM node:20-slim AS jsbuild
WORKDIR /jsbuild
RUN echo '{"name":"tb","private":true}' > package.json && \
    npm install codemirror @codemirror/state @codemirror/language @codemirror/legacy-modes esbuild --silent
COPY static/js/entry.js ./entry.js
RUN ./node_modules/.bin/esbuild entry.js --bundle --format=esm --minify --outfile=editor.bundle.js

# Rust build stage
FROM rust:1.95-bookworm AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
# Dummy build for layer caching
RUN mkdir src && echo 'fn main(){}' > src/main.rs && cargo build --release && rm -f target/release/texly
COPY src ./src
RUN touch src/main.rs && cargo build --release

# Runtime stage
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y \
    ca-certificates \
    wget \
    fontconfig \
    fonts-lmodern \
    fonts-dejavu \
    fonts-liberation \
    && rm -rf /var/lib/apt/lists/*

# Install Tectonic
RUN wget -qO /tmp/tectonic.tar.gz \
    "https://github.com/tectonic-typesetting/tectonic/releases/download/tectonic%400.16.9/tectonic-0.16.9-x86_64-unknown-linux-musl.tar.gz" \
    && tar -xzf /tmp/tectonic.tar.gz -C /usr/local/bin \
    && chmod +x /usr/local/bin/tectonic \
    && rm /tmp/tectonic.tar.gz

WORKDIR /app
COPY --from=builder /build/target/release/texly ./
COPY static ./static
COPY --from=jsbuild /jsbuild/editor.bundle.js ./static/js/editor.bundle.js

# Data volumes
RUN mkdir -p /data/users /data/home /data/share /root/.cache/tectonic

EXPOSE 8080
ENV TEXLY_DATA_DIR=/data \
    TEXLY_PORT=8080 \
    TEXLY_STATIC_DIR=/app/static

CMD ["./texly"]
