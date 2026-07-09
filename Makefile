# Eldr: build / install / test. Zero external crates by policy.
PREFIX ?= $(HOME)/.local
BINDIR := $(PREFIX)/bin
BIN    := target/release/eldr
MENU_BIN := target/release/EldrMenu
MENU_TEST_BIN := target/debug/eldr-menu-tests
MENU_SRC := $(wildcard menubar/*.swift)
SWIFT_TARGET := arm64-apple-macosx13.0
MACOS_SDK := $(shell xcrun --sdk macosx --show-sdk-path)
VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
APPDIR := $(HOME)/Applications/Eldr.app
ICNS   := assets/eldr.icns
MENU_MARK := assets/eldr-menubar-template.png
OPEN_MENU ?= 1
TAG_VERSION ?=

.PHONY: all build release test check clippy install install-cli uninstall app check-app menu menu-open menu-test verify-version verify-tag-version guard-install guard-uninstall clean fmt

all: release

build:
	cargo build --locked

release:
	cargo build --release --locked

test:
	cargo test --locked

clippy:
	cargo clippy --locked --all-targets -- -D warnings

fmt:
	cargo fmt

# Assert the zero-dependency invariant: Cargo.lock must contain only this crate.
check-zero-deps:
	@n=$$(grep -c '^name = ' Cargo.lock); \
	if [ "$$n" != "1" ]; then echo "FAIL: $$n crates in Cargo.lock (expected 1)"; exit 1; fi; \
	echo "OK: zero external crates"

verify-version:
	@short=$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' packaging/Info.plist); \
	build=$$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' packaging/Info.plist); \
	if [ "$(VERSION)" != "$$short" ] || [ "$(VERSION)" != "$$build" ]; then \
	  echo "FAIL: Cargo=$(VERSION) plist-short=$$short plist-build=$$build"; exit 1; \
	fi; \
	echo "OK: release version $(VERSION)"

verify-tag-version: verify-version
	@if [ -z "$(TAG_VERSION)" ]; then echo "FAIL: set TAG_VERSION to the tag without v"; exit 1; fi
	@if [ "$(TAG_VERSION)" != "$(VERSION)" ]; then echo "FAIL: tag=$(TAG_VERSION) Cargo=$(VERSION)"; exit 1; fi
	@echo "OK: tag version $(TAG_VERSION)"

# Build the native macOS menu bar executable with no package manager or third-party module.
menu: $(MENU_SRC)
	@mkdir -p "$(dir $(MENU_BIN))"
	xcrun swiftc -O -whole-module-optimization -parse-as-library \
		-target $(SWIFT_TARGET) -sdk "$(MACOS_SDK)" \
		-framework SwiftUI -framework AppKit -framework Foundation \
		-o "$(MENU_BIN)" $(MENU_SRC)

menu-test: $(MENU_SRC)
	@mkdir -p "$(dir $(MENU_TEST_BIN))"
	xcrun swiftc -D ELDR_MENU_TESTS -parse-as-library \
		-target $(SWIFT_TARGET) -sdk "$(MACOS_SDK)" \
		-framework SwiftUI -framework AppKit -framework Foundation \
		-o "$(MENU_TEST_BIN)" $(MENU_SRC)
	"$(MENU_TEST_BIN)"

# Assemble Eldr.app. The menu app is the bundle entry point; the Rust executable stays
# beside it because launchd deliberately runs Contents/MacOS/eldr as the guard.
app: release menu $(MENU_MARK)
	@rm -rf "$(APPDIR)"
	@mkdir -p "$(APPDIR)/Contents/MacOS" "$(APPDIR)/Contents/Resources"
	install -m 0644 packaging/Info.plist "$(APPDIR)/Contents/Info.plist"
	install -m 0755 $(BIN) "$(APPDIR)/Contents/MacOS/eldr"
	install -m 0755 $(MENU_BIN) "$(APPDIR)/Contents/MacOS/EldrMenu"
	install -m 0644 $(ICNS) "$(APPDIR)/Contents/Resources/eldr.icns"
	install -m 0644 $(MENU_MARK) "$(APPDIR)/Contents/Resources/eldr-menubar-template.png"
	@printf 'APPL????' > "$(APPDIR)/Contents/PkgInfo"
	@codesign --force --sign - "$(APPDIR)/Contents/MacOS/eldr"
	@codesign --force --sign - "$(APPDIR)/Contents/MacOS/EldrMenu"
	@codesign --force --sign - "$(APPDIR)" && echo "ad-hoc signed ($(shell /usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' packaging/Info.plist))"
	@codesign --verify --deep --strict "$(APPDIR)"
	@touch "$(APPDIR)"
	@echo "built -> $(APPDIR)"

check-app: app
	plutil -lint "$(APPDIR)/Contents/Info.plist"
	codesign --verify --deep --strict "$(APPDIR)"
	test -x "$(APPDIR)/Contents/MacOS/eldr"
	test -x "$(APPDIR)/Contents/MacOS/EldrMenu"
	test -f "$(APPDIR)/Contents/Resources/eldr-menubar-template.png"
	"$(APPDIR)/Contents/MacOS/eldr" version

menu-open: app
	@open -g "$(APPDIR)" || echo "built the menu app but could not open it in this session"

# Install just the CLI to $(BINDIR), with no Eldr.app bundle (the guard daemon needs the
# bundle; one-off CLI use does not). Warns if $(BINDIR) isn't on PATH.
install-cli: release
	@mkdir -p $(BINDIR)
	install -m 0755 $(BIN) $(BINDIR)/eldr
	@echo "installed -> $(BINDIR)/eldr"
	@case ":$$PATH:" in \
	  *":$(BINDIR):"*) ;; \
	  *) printf 'note: %s is not on your PATH. Add it, e.g.:\n      export PATH="%s:$$PATH"\n' "$(BINDIR)" "$(BINDIR)" ;; \
	esac

# Install the CLI, assemble Eldr.app and open the menu by default. Set OPEN_MENU=0 for a
# non-graphical installation.
install: install-cli app
	@echo "installed -> $(BINDIR)/eldr  and  $(APPDIR)"
	@if [ "$(OPEN_MENU)" = "1" ]; then \
	  open -g "$(APPDIR)" || echo "installed the menu app but could not open it in this session"; \
	fi

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
