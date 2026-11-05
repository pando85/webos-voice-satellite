# Developer entry points. See docs/development.md.
# TV access: `make deploy-tv TV=root@<host>`; every ssh/scp runs under `timeout`.

-include local.mk
TV            ?= root@webos-tv
TV_DIR        ?= /tmp
SSH_CTL       ?= $(HOME)/.ssh/ctl-webos-%C
SSH_OPTS      ?= -o ControlMaster=auto -o ControlPersist=60 -o ControlPath=$(SSH_CTL) -o ConnectTimeout=10
TIMEOUT       ?= 30
TV_PREFIX     ?= /tmp/webos-satellite
TV_STAGE      ?= /tmp/webos-satellite-stage
# Build install.sh picks on the TV: auto (uname -m), armv7 or aarch64.
TV_ARCH       ?= auto
PKG           := packaging/webos
PKG_OUT       := target/webos-package
PKG_TAR       := target/webos-satellite-package.tar.gz

BIN           := webos-satellite
TARGET_ARM    := armv7-unknown-linux-musleabi
TARGET_A64    := aarch64-unknown-linux-musl
OUT_ARM       := target/$(TARGET_ARM)/release/$(BIN)
OUT_A64       := target/$(TARGET_A64)/release/$(BIN)

# webOS app package (.ipk); docs/design/packaging.md. The version is the
# workspace version in Cargo.toml.
APP_ID        ?= io.github.pando85.voicesatellite
VERSION       := $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)
IPK_DIR       := target/ipk
IPK           := $(IPK_DIR)/$(APP_ID)_$(VERSION)_all.ipk
IPK_BUILD     := python3 $(PKG)/ipk/build_ipk.py
TV_APP_DIR    ?= /media/developer/apps/usr/palm/applications/$(APP_ID)

.PHONY: build-tv build-tv64 build-tv-all package-tv test lint fmt deploy-tv run-tv-selftest clean-tv tv-install-tmp tv-uninstall-tmp
.PHONY: ipk ipk-check test-packaging tv-install-ipk tv-remove-ipk

# Static 32-bit ARM EABI5 binary (flags come from .cargo/config.toml). Runs on
# every supported TV; the build verified on c3-a.
build-tv:
	cargo zigbuild --release -p webos-satellite --target $(TARGET_ARM)
	@file $(OUT_ARM)
	@sha256sum $(OUT_ARM)

# Static aarch64 binary, for TVs with a 64-bit kernel (install.sh selects it
# by `uname -m` and falls back to armv7). The daemon as aarch64 is not yet
# verified on a TV.
build-tv64:
	cargo zigbuild --release -p webos-satellite --target $(TARGET_A64)
	@file $(OUT_A64)
	@sha256sum $(OUT_A64)

build-tv-all: build-tv build-tv64

# Package directory and tarball: the ops scripts plus both builds, named
# webos-satellite-armv7 and webos-satellite-aarch64, and SHA256SUMS.
# On the TV: sh install.sh --prefix DIR --config FILE [--arch armv7|aarch64].
package-tv: build-tv-all
	rm -rf $(PKG_OUT)
	mkdir -p $(PKG_OUT)
	cp $(PKG)/*.sh $(PKG_OUT)/
	cp $(OUT_ARM) $(PKG_OUT)/$(BIN)-armv7
	cp $(OUT_A64) $(PKG_OUT)/$(BIN)-aarch64
	chmod 755 $(PKG_OUT)/$(BIN)-armv7 $(PKG_OUT)/$(BIN)-aarch64
	cd $(PKG_OUT) && sha256sum $(BIN)-armv7 $(BIN)-aarch64 *.sh >SHA256SUMS
	tar -C $(dir $(PKG_OUT)) -czf $(PKG_TAR) $(notdir $(PKG_OUT))
	@cat $(PKG_OUT)/SHA256SUMS
	@echo "package: $(PKG_OUT) and $(PKG_TAR)"

# Both TV builds plus the scripts and the status app in one reproducible
# ipk (target/ipk/<id>_<version>_all.ipk), its sha256 and an HBC manifest.
ipk: build-tv-all
	$(IPK_BUILD) build --app-id $(APP_ID) --version $(VERSION) --armv7 $(OUT_ARM) --aarch64 $(OUT_A64) --out $(IPK_DIR)
	$(IPK_BUILD) check --app-id $(APP_ID) --version $(VERSION) $(IPK)

# Structure check of an existing ipk (ar/tar layout, control, appinfo, payload).
ipk-check:
	$(IPK_BUILD) check --app-id $(APP_ID) --version $(VERSION) $(IPK)

# Host tests of the scripts, the package lifecycle and the status app.
test-packaging:
	sh $(PKG)/tests/run.sh
	sh $(PKG)/tests/ipk.sh
	sh $(PKG)/tests/tv_settings.sh
	node $(PKG)/tests/app_smoke.js
	node $(PKG)/tests/app_matrix.js

test:
	cargo test --workspace

fmt:
	cargo fmt --all

lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

# Copies the binaries to the TV's tmpfs. Nothing else is written on the TV.
deploy-tv: build-tv build-tv64
	timeout $(TIMEOUT) scp $(SSH_OPTS) $(OUT_ARM) $(TV):$(TV_DIR)/$(BIN)-armv7
	timeout $(TIMEOUT) scp $(SSH_OPTS) $(OUT_A64) $(TV):$(TV_DIR)/$(BIN)-aarch64

# Runs --self-test of each deployed binary (read-only on the TV).
run-tv-selftest:
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) '$(TV_DIR)/$(BIN)-armv7 --self-test'
	-timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) '$(TV_DIR)/$(BIN)-aarch64 --self-test'

# Removes the deployed binaries from the TV's /tmp.
clean-tv:
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'rm -f $(TV_DIR)/$(BIN)-armv7 $(TV_DIR)/$(BIN)-aarch64'

# Installs under TV_PREFIX on the TV's tmpfs (lost at reboot; nothing else is
# written, no autostart). Needs TV_CONFIG=<local toml> and, for an encrypted
# API, TV_PSK=<local key file>. Copies the package (both builds); install.sh
# picks one with TV_ARCH (default auto: aarch64 on a 64-bit kernel if it
# runs, else armv7; TV_ARCH=armv7 pins the build verified on c3-a). Starts
# nothing: run
#   ssh $(TV) 'sh $(TV_PREFIX)/bin/start.sh'
# Each TV session needs the owner's authorization (docs/development.md).
tv-install-tmp: package-tv
	@test -n "$(TV_CONFIG)" || { echo "set TV_CONFIG=<config.toml> (and TV_PSK=<key file>)"; exit 2; }
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'rm -rf $(TV_STAGE) && mkdir -m 700 $(TV_STAGE)'
	timeout $(TIMEOUT) scp $(SSH_OPTS) $(PKG_OUT)/*.sh $(PKG_OUT)/$(BIN)-armv7 $(PKG_OUT)/$(BIN)-aarch64 $(TV_CONFIG) $(TV_PSK) $(TV):$(TV_STAGE)/
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'cd $(TV_STAGE) && sh ./install.sh --prefix $(TV_PREFIX) --arch $(TV_ARCH) --config ./$(notdir $(TV_CONFIG)) $(if $(TV_PSK),--psk-file ./$(notdir $(TV_PSK))); rc=$$?; cd /; rm -rf $(TV_STAGE); exit $$rc'

# Stops the daemon and removes TV_PREFIX (and the staging directory) from the TV.
tv-uninstall-tmp:
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'sh $(TV_PREFIX)/bin/uninstall.sh; rm -rf $(TV_STAGE)'

# Installs (or updates) the ipk on the TV through appinstalld, as the Homebrew
# Channel does, then prints the package status. Starts nothing and writes
# nothing outside the app directory: set up and start from the app, or with
#   ssh $(TV) 'sh $(TV_APP_DIR)/payload/ctl.sh install'
# Persistent change on the TV: needs the owner's authorization.
tv-install-ipk: ipk
	timeout $(TIMEOUT) scp $(SSH_OPTS) $(IPK) $(TV):/tmp/$(notdir $(IPK))
	timeout 90 ssh $(SSH_OPTS) $(TV) sh -s -- install /tmp/$(notdir $(IPK)) <$(PKG)/tv/appinstall.sh; rc=$$?; \
	  timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'rm -f /tmp/$(notdir $(IPK))'; exit $$rc
	timeout $(TIMEOUT) ssh $(SSH_OPTS) $(TV) 'sh $(TV_APP_DIR)/payload/ctl.sh status'

# Removes the app (after `ctl.sh uninstall` or `purge`, which this does not run).
tv-remove-ipk:
	timeout 90 ssh $(SSH_OPTS) $(TV) sh -s -- remove $(APP_ID) <$(PKG)/tv/appinstall.sh
