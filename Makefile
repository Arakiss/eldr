# Eldr — build / install / test. Zero external crates by policy.
PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN    := target/release/eldr

.PHONY: all build release test check clippy install uninstall guard-install guard-uninstall clean fmt

all: release

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

clippy:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt

# Assert the zero-dependency invariant: Cargo.lock must contain only this crate.
check-zero-deps:
	@n=$$(grep -c '^name = ' Cargo.lock); \
	if [ "$$n" != "1" ]; then echo "FAIL: $$n crates in Cargo.lock (expected 1)"; exit 1; fi; \
	echo "OK: zero external crates"

install: release
	@mkdir -p $(BINDIR)
	install -m 0755 $(BIN) $(BINDIR)/eldr
	@echo "installed -> $(BINDIR)/eldr"

uninstall:
	rm -f $(BINDIR)/eldr

# Run the guard 24/7 via launchd (delegates to the binary).
guard-install: install
	$(BINDIR)/eldr guard-install

guard-uninstall:
	$(BINDIR)/eldr guard-uninstall

clean:
	cargo clean
