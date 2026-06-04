# Eldr — build / install / test. Zero external crates by policy.
PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN    := target/release/eldr
APPDIR := $(HOME)/Applications/Eldr.app
ICNS   := assets/eldr.icns

.PHONY: all build release test check clippy install install-cli uninstall app guard-install guard-uninstall clean fmt

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
	@codesign --force --deep --sign - "$(APPDIR)" && echo "ad-hoc signed ($(shell /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' packaging/Info.plist))"
	@touch "$(APPDIR)"
	@echo "built -> $(APPDIR)"

# Install just the CLI to $(BINDIR) — no Eldr.app bundle (the guard daemon needs the
# bundle; one-off CLI use does not). Warns if $(BINDIR) isn't on PATH.
install-cli: release
	@mkdir -p $(BINDIR)
	install -m 0755 $(BIN) $(BINDIR)/eldr
	@echo "installed -> $(BINDIR)/eldr"
	@case ":$$PATH:" in \
	  *":$(BINDIR):"*) ;; \
	  *) printf 'note: %s is not on your PATH. Add it, e.g.:\n      export PATH="%s:$$PATH"\n' "$(BINDIR)" "$(BINDIR)" ;; \
	esac

# Install the CLI and assemble Eldr.app (the bundle the guard daemon runs from).
install: install-cli app
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
