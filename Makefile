# Short commands for daily work. Run `make` to see them.

# rustup and cmake from Homebrew are not on PATH by default.
TOOLS := PATH="/opt/homebrew/opt/rustup/bin:/opt/homebrew/bin:$$PATH"

APP := target/release/bundle/macos/OpenLang.app
DMG_DIR := target/release/bundle/dmg
MOUNT := /tmp/openlang-dmg

.PHONY: help build open run install check test

help:
	@echo "make build          build OpenLang.app and the .dmg"
	@echo "make open           open the last build"
	@echo "make run            build, then open"
	@echo "make install        build, then install the .dmg into /Applications"
	@echo "make check          format check, lint, UI syntax"
	@echo "make test           all tests (speaks real audio, needs network once)"

build:
	cd app && $(TOOLS) npx @tauri-apps/cli@2 build
	@echo "App: $(APP)"
	@echo "DMG: $$(ls $(DMG_DIR)/*.dmg)"

open:
	open $(APP)

run: build open

install: build
	hdiutil attach -nobrowse -readonly -quiet -mountpoint $(MOUNT) $$(ls -t $(DMG_DIR)/*.dmg | head -1)
	rm -rf /Applications/OpenLang.app
	cp -R $(MOUNT)/OpenLang.app /Applications/; status=$$?; hdiutil detach -quiet $(MOUNT); exit $$status
	open /Applications/OpenLang.app

check:
	$(TOOLS) cargo fmt --all --check
	$(TOOLS) cargo clippy -q --workspace --all-targets -- -D warnings
	for f in ui/*/*.js; do node --check --input-type=module < $$f || exit 1; done

test:
	$(TOOLS) cargo test --workspace
