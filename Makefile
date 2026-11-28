# Short commands for daily work on macOS, Windows and Linux. Run `make` to see them.
# Windows: run make from Git Bash. CI uses these same targets.

# What differs per system. Every target below is written once.
ifeq ($(OS),Windows_NT)
  # whisper.cpp's Vulkan shader build passes MSBuild's 260 character path limit under a deep
  # target directory.
  export CARGO_TARGET_DIR ?= C:/t
  SYSTEM := windows
else
  SYSTEM := $(shell uname -s)
endif

TARGET := $(or $(CARGO_TARGET_DIR),target)
# Set TRIPLE to build for another processor, e.g. TRIPLE=x86_64-apple-darwin.
RELEASE := $(TARGET)/$(if $(TRIPLE),$(TRIPLE)/)release
BUNDLE := $(RELEASE)/bundle
newest = $$(ls -t $(BUNDLE)/$(1) | head -1)

ifeq ($(SYSTEM),Darwin)
  # rustup and cmake from Homebrew are not on PATH by default.
  TOOLS := PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$$PATH"
  BUNDLES := macos/OpenLang.app dmg/*.dmg
  OPEN := open $(BUNDLE)/macos/OpenLang.app
  INSTALL := mount=$$(mktemp -d) && hdiutil attach -nobrowse -readonly -quiet -mountpoint $$mount "$(call newest,dmg/*.dmg)" \
    && rm -rf /Applications/OpenLang.app && cp -R $$mount/OpenLang.app /Applications/; \
    status=$$?; hdiutil detach -quiet $$mount; [ $$status = 0 ] && open /Applications/OpenLang.app
else ifeq ($(SYSTEM),windows)
  BUNDLES := nsis/*.exe
  # The app needs app/vulkan-1.dll beside it or on PATH.
  OPEN := PATH="$(CURDIR)/app:$$PATH" "$(RELEASE)/openlang.exe" &
  # The installer offers to open OpenLang when it is done.
  INSTALL := "$(call newest,nsis/*.exe)"
else
  BUNDLES := deb/*.deb appimage/*.AppImage
  OPEN := "$(call newest,appimage/*.AppImage)" &
  INSTALL := sudo apt install -y "$$(realpath $(call newest,deb/*.deb))" && (openlang &)
endif

.PHONY: help build open run install check test

help:
	@echo "make build          build OpenLang ($(BUNDLES) in $(BUNDLE))"
	@echo "make open           open the last build"
	@echo "make run            build, then open"
	@echo "make install        build, then install it on this computer and open it"
	@echo "make check          format check, lint, UI syntax"
	@echo "make test           all tests (speaks real audio on macOS, needs network once)"
	@echo "                    ARGS=... passes more arguments to cargo test"

build:
	cd app && $(TOOLS) npx --yes @tauri-apps/cli@2 build $(if $(TRIPLE),--target $(TRIPLE))
	@for b in $(BUNDLES); do ls -d $(BUNDLE)/$$b; done

open:
	$(OPEN)

run: build open

install: build
	$(INSTALL)

check:
	$(TOOLS) cargo fmt --all --check
	$(TOOLS) cargo clippy -q --workspace --all-targets -- -D warnings
	for f in ui/*/*.js; do node --check --input-type=module < "$$f" || exit 1; done

test:
	$(TOOLS) cargo test --workspace $(ARGS)
