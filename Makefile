.PHONY: all build release debug test clean static static-full static-freebsd static-freebsd-full install uninstall help

# Default target
all: build

# Build the project
build:
	cargo build

# Build release version
release:
	cargo build --release

# Build debug version
debug:
	cargo build

# Run tests
test:
	cargo test --lib

# Clean build artifacts
clean:
	cargo clean

# Build statically linked binary using musl
static:
	@UNAME_S=$$(uname -s); \
	if [ "$$UNAME_S" = "FreeBSD" ]; then \
		echo "musl target not supported on FreeBSD; skipping"; \
	else \
		rustup target add x86_64-unknown-linux-musl; \
		cargo build --release --target x86_64-unknown-linux-musl; \
	fi

# Build statically linked binary with all features
static-full:
	@UNAME_S=$$(uname -s); \
	if [ "$$UNAME_S" = "FreeBSD" ]; then \
		echo "musl target not supported on FreeBSD; skipping"; \
	else \
		rustup target add x86_64-unknown-linux-musl; \
		cargo build --release --target x86_64-unknown-linux-musl --features "static"; \
	fi

# Build statically linked binary for FreeBSD
static-freebsd:
	@if command -v rustup >/dev/null 2>&1; then \
		rustup target add x86_64-unknown-freebsd; \
	else \
		echo "rustup not found; assuming x86_64-unknown-freebsd toolchain is installed"; \
	fi
	cargo build --release --target x86_64-unknown-freebsd

# Build statically linked binary for FreeBSD with all features
static-freebsd-full:
	@if command -v rustup >/dev/null 2>&1; then \
		rustup target add x86_64-unknown-freebsd; \
	else \
		echo "rustup not found; assuming x86_64-unknown-freebsd toolchain is installed"; \
	fi
	cargo build --release --target x86_64-unknown-freebsd --features "static"

# Install the binary (requires root/sudo)
install:
	install -Dm755 target/release/zjudo /usr/local/bin/zjudo

# Uninstall the binary
uninstall:
	rm -f /usr/local/bin/zjudo

# Show help
help:
	@echo "Available targets:"
	@echo "  all          - Build the project (default)"
	@echo "  build        - Build the project"
	@echo "  release      - Build release version"
	@echo "  debug        - Build debug version"
	@echo "  test         - Run tests"
	@echo "  clean        - Clean build artifacts"
	@echo "  static       - Build statically linked binary (musl)"
	@echo "  static-full  - Build statically linked binary with all features"
	@echo "  static-freebsd       - Build statically linked binary for FreeBSD"
	@echo "  static-freebsd-full  - Build statically linked binary for FreeBSD with all features"
	@echo "  install      - Install binary to /usr/local/bin (requires root)"
	@echo "  uninstall    - Uninstall binary from /usr/local/bin"
	@echo "  help         - Show this help message"

# Alias for static target
musl: static
