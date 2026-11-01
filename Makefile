# Developer entry points. See docs/development.md.
# TV access: `make deploy-tv TV=root@<host>`; every ssh/scp runs under `timeout`.

-include local.mk
TV            ?= root@webos-tv
TV_DIR        ?= /tmp
SSH_CTL       ?= $(HOME)/.ssh/ctl-webos-%C
SSH_OPTS      ?= -o ControlMaster=auto -o ControlPersist=60 -o ControlPath=$(SSH_CTL) -o ConnectTimeout=10
TIMEOUT       ?= 30

BIN           := webos-satellite
TARGET_ARM    := armv7-unknown-linux-musleabi
TARGET_A64    := aarch64-unknown-linux-musl
OUT_ARM       := target/$(TARGET_ARM)/release/$(BIN)
OUT_A64       := target/$(TARGET_A64)/release/$(BIN)

.PHONY: build-tv build-tv64 test lint fmt deploy-tv run-tv-selftest clean-tv

# Static 32-bit ARM EABI5 softfp binary (flags come from .cargo/config.toml).
build-tv:
	cargo zigbuild --release -p webos-satellite --target $(TARGET_ARM)
	@file $(OUT_ARM)
	@sha256sum $(OUT_ARM)

# Static aarch64 binary, for comparison.
build-tv64:
	cargo zigbuild --release -p webos-satellite --target $(TARGET_A64)
	@file $(OUT_A64)
	@sha256sum $(OUT_A64)

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
