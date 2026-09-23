# Riflu build tasks. `make` on its own lists them.
#
# Windows binaries cannot be cross-compiled from macOS: Tauri needs the MSVC
# toolchain, the Windows SDK and WebView2, and even `cargo check` stops at the
# missing `llvm-rc`. Run `make windows` on Windows, or build it in CI.

export PATH := $(HOME)/.cargo/bin:$(PATH)

ifeq ($(OS),Windows_NT)
  HOST := windows
else
  HOST := $(shell uname -s | tr 'A-Z' 'a-z')
endif

TAURI    := npm run tauri --
APP      := src-tauri/target/release/bundle/macos/Riflu.app
INSTALL  := /Applications/Riflu.app

.DEFAULT_GOAL := help
.PHONY: help deps dev check test probe mac dmg install windows clean

help:
	@echo "Riflu — host: $(HOST)"
	@echo
	@echo "  make deps      install npm dependencies"
	@echo "  make dev       run the app with hot reload"
	@echo "  make check     typecheck the frontend and build the Rust core"
	@echo "  make test      cargo unit tests (URL signing)"
	@echo "  make probe     end-to-end API check, no UI"
	@echo
	@echo "  make mac       release .app            (macOS host)"
	@echo "  make dmg       release .app + .dmg     (macOS host)"
	@echo "  make install   build and replace $(INSTALL)"
	@echo "  make windows   release .exe installer  (Windows host)"
	@echo
	@echo "  make clean     drop build output"

deps:
	npm install

dev:
	$(TAURI) dev

check:
	npx tsc --noEmit
	cd src-tauri && cargo build

test:
	cd src-tauri && cargo test --lib

probe:
	cd src-tauri && cargo run --example probe

# ---- macOS ----

mac:
ifneq ($(HOST),darwin)
	@echo "make mac needs a macOS host (this is $(HOST))." >&2
	@exit 1
endif
	$(TAURI) build --bundles app
	@echo "built $(APP)"

# Separate from `mac` because bundle_dmg.sh needs Finder automation rights and
# fails without them; the .app alone is enough to install locally.
dmg:
ifneq ($(HOST),darwin)
	@echo "make dmg needs a macOS host (this is $(HOST))." >&2
	@exit 1
endif
	$(TAURI) build --bundles app,dmg

install: mac
	@echo "replacing $(INSTALL) — quitting it first if it is running"
	-pkill -f "$(INSTALL)/Contents/MacOS" 2>/dev/null || true
	@sleep 1
	rm -rf "$(INSTALL)"
	ditto "$(APP)" "$(INSTALL)"
	@echo "installed. open it with: open $(INSTALL)"

# ---- Windows ----

windows:
ifneq ($(HOST),windows)
	@echo "Windows binaries cannot be cross-compiled from $(HOST)." >&2
	@echo "Tauri needs the MSVC toolchain, the Windows SDK and WebView2." >&2
	@echo "Run 'make windows' on a Windows host, or build it in CI." >&2
	@exit 1
endif
	$(TAURI) build --bundles nsis

clean:
	rm -rf dist
	cd src-tauri && cargo clean
