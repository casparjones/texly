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
# Curated font base set for the Tectonic/XeTeX backend. XeTeX resolves
# \setmainfont{...} via system fontconfig (not the TeX tree), so these must be
# installed in the image and fc-cache must be primed at build time.
#   lmodern + TeX Gyre  → classic LaTeX look, Times/Helvetica/Palatino substitutes
#   Liberation          → metric-compatible MS replacements (Arial/Times/Courier)
#   Noto + CJK + Emoji  → full Unicode script coverage
#   EB Garamond         → nice serif bonus. NB: Debian ships the optical-size
#                         variant, so use \setmainfont{EB Garamond 12} (the bare
#                         "EB Garamond" superfamily has non-standard style names
#                         "12 Bold"/"08 Regular" that fontspec can't auto-resolve).
# All OFL/GUST/Apache — safe to bundle. (NB: 'lmodern' is the correct Debian
# package name; the old 'fonts-lmodern' does not exist.)
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    wget \
    fontconfig \
    lmodern \
    fonts-texgyre \
    fonts-noto \
    fonts-noto-cjk \
    fonts-noto-color-emoji \
    fonts-liberation \
    fonts-dejavu \
    fonts-ebgaramond \
    && fc-cache -f \
    && echo "fc-list reports $(fc-list | wc -l) font files" \
    && rm -rf /var/lib/apt/lists/*

# Font fallback safety net.
#  - 99-texly.conf: fontconfig alias families (__texly_serif/_sans/_mono).
#    NB: Tectonic/XeTeX does NOT honor fontconfig <alias> families for
#    \setmainfont — verified. The conf is kept for fc-match / non-Tectonic
#    consumers; the *effective* safety net for documents is texly-fonts.tex.
#  - texly-fonts.tex: \input-able preamble helpers (\texlySetMain/Sans/Mono)
#    that degrade to a real bundled family via \IfFontExistsTF instead of
#    failing the compile.
COPY deploy/fontconfig/99-texly.conf /etc/fonts/conf.d/99-texly.conf
COPY deploy/fontconfig/texly-fonts.tex /usr/share/texly/texly-fonts.tex
RUN fc-cache -f

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
RUN mkdir -p /data/users /data/home /data/share /data/fonts /root/.cache/tectonic

EXPOSE 8080
ENV TEXLY_DATA_DIR=/data \
    TEXLY_PORT=8080 \
    TEXLY_STATIC_DIR=/app/static

CMD ["./texly"]
