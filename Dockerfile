# Build stage
FROM rust:1.83-slim-bookworm AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests
COPY Cargo.toml Cargo.lock ./

# Create dummy source to cache dependencies
RUN mkdir -p src/bin && \
    echo "fn main() {}" > src/bin/catbus.rs && \
    echo "fn main() {}" > src/bin/catbusd.rs && \
    echo "pub fn dummy() {}" > src/lib.rs

# Build dependencies (this layer is cached)
RUN cargo build --release && \
    rm -rf src

# Copy actual source
COPY src ./src

# Touch sources to trigger rebuild with actual code
RUN touch src/bin/catbusd.rs src/bin/catbus.rs && \
    cargo build --release

# Runtime stage
FROM debian:bookworm-slim

WORKDIR /app

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Copy binaries from builder
COPY --from=builder /app/target/release/catbusd /usr/local/bin/catbusd
COPY --from=builder /app/target/release/catbus /usr/local/bin/catbus

# Create non-root user
RUN useradd -r -s /bin/false catbus
USER catbus

# Default environment variables
ENV RUST_LOG=info

# Expose WebTransport port (UDP for QUIC)
EXPOSE 4433/udp

# Run daemon in foreground (for Docker/k8s)
ENTRYPOINT ["catbusd"]
CMD ["--bind", "0.0.0.0:4433"]
