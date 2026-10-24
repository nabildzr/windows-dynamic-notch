# Cross-compilation image: builds the Windows executable (x86_64-pc-windows-gnu)
# and its NSIS installer from Linux, so the host does not need a local Rust,
# MSVC or NSIS installation.
FROM rust:1-bookworm

LABEL org.opencontainers.image.source="https://github.com/nabildzr/windows-dynamic-notch" \
      org.opencontainers.image.description="Build environment for Windows Dynamic Notch: Rust + MinGW-w64 + NSIS (cross-compiles to x86_64-pc-windows-gnu)" \
      org.opencontainers.image.licenses="MIT"

RUN apt-get update \
    && apt-get install -y --no-install-recommends -o Acquire::Retries=5 \
        gcc-mingw-w64-x86-64 g++-mingw-w64-x86-64 binutils-mingw-w64-x86-64 \
    && rm -rf /var/lib/apt/lists/*

# Installer tooling: makensis builds the setup executable, 7z inspects it.
# Fetched over HTTPS: some networks intercept plain-HTTP package downloads.
RUN sed -i 's|http://deb.debian.org|https://deb.debian.org|g' /etc/apt/sources.list.d/debian.sources \
    && apt-get update \
    && apt-get install -y --no-install-recommends -o Acquire::Retries=5 nsis p7zip-full \
    && rm -rf /var/lib/apt/lists/*

RUN rustup target add x86_64-pc-windows-gnu \
    && rustup component add clippy rustfmt

ENV CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc \
    CARGO_TARGET_DIR=/target \
    CARGO_TERM_COLOR=never

WORKDIR /src
