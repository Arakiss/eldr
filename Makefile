# Eldr — build / install / test. Zero external crates by policy.
PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN    := target/release/eldr
APPDIR := $(HOME)/Applications/Eldr.app
ICNS   := assets/eldr.icns

.PHONY: all build release test check clippy install uninstall app guard-install guard-uninstall clean fmt

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

# Assemble Eldr.app so the guard daemon shows its icon in Login Items.
app: release
	@rm -rf "$(APPDIR)"
	@mkdir -p "$(APPDIR)/Contents/MacOS" "$(APPDIR)/Contents/Resources"
	install -m 0644 packaging/Info.plist "$(APPDIR)/Contents/Info.plist"
	install -m 0755 $(BIN) "$(APPDIR)/Contents/MacOS/eldr"
	install -m 0644 $(ICNS) "$(APPDIR)/Contents/Resources/eldr.icns"
	@printf 'APPL????' > "$(APPDIR)/Contents/PkgInfo"
	@touch "$(APPDIR)"
	@echo "built -> $(APPDIR)"

install: release app
	@mkdir -p $(BINDIR)
	install -m 0755 $(BIN) $(BINDIR)/eldr
	@echo "installed -> $(BINDIR)/eldr  and  $(APPDIR)"

uninstall:
	rm -f $(BINDIR)/eldr
	rm -rf "$(APPDIR)"

# Run the guard 24/7 via launchd (delegates to the binary).
guard-install: install
	$(BINDIR)/eldr guard-install

guard-uninstall:
	$(BINDIR)/eldr guard-uninstall

clean:
	cargo clean
