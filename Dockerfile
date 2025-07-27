# (1) this stage will be run always on current arch
# zigbuild & Cargo targets added
FROM --platform=$BUILDPLATFORM rust:alpine AS chef
WORKDIR /app
COPY rust-toolchain.toml rust-toolchain.toml
ENV PKGCONFIG_SYSROOTDIR=/
RUN apk add --no-cache musl-dev openssl-dev zig
RUN cargo install --locked cargo-zigbuild cargo-chef
RUN rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl
 
# (2) nothing changed
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json
 
# (3) building project deps: need to specify all targets; zigbuild used
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --recipe-path recipe.json --release --zigbuild \
  --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl
 
# (4) actuall project build for all targets
# binary renamed to easier copy in runtime stage
COPY . .
RUN cargo zigbuild -r --target x86_64-unknown-linux-musl --target aarch64-unknown-linux-musl && \
  mkdir /app/linux && \
  cp target/aarch64-unknown-linux-musl/release/dyrics /app/linux/arm64 && \
  cp target/x86_64-unknown-linux-musl/release/dyrics /app/linux/amd64
 
# (5) this staged will be emulated as was before
# TARGETPLATFORM usage to copy right binary from builder stage
# ARG populated by docker itself
FROM alpine:latest AS runtime
WORKDIR /app
ARG TARGETPLATFORM
COPY --from=builder /app/${TARGETPLATFORM} /app/dyrics
ENTRYPOINT ["/app/dyrics"]
