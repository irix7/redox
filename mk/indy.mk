# SGI Indy netboot: a file named `redox` in a TFTP directory, booted at the PROM
# with `boot -f bootp()redox`. Independent of ARCH and of the disk image path.
#
#   make indy            build build/indy/redox (ECOFF): kernel and initfs
#   make indy-root       build the root tree and pack it into the webfs store
#   make indy-gate       boot it on a 32 MiB emulated Indy and assert the marker
#   make indy-netboot    copy it to INDY_TFTP_DIR (default /var/lib/sgi-netboot)

INDY_BUILD=build/indy
INDY_TARGET=targets/mips64-sgi-redox.json
INDY_TFTP_DIR?=/var/lib/sgi-netboot
# Names to install the image under. A BOOTP server that names its own boot file
# (dnsmasq dhcp-boot) makes the PROM fetch that name whatever was typed.
INDY_TFTP_NAMES?=redox
INDY_MARKER?=addr 192.168
# The toolchain pin of this fork; the kernel checkout carries upstream's own.
INDY_RUST_TOOLCHAIN?=nightly-2026-10-03

# The image the PROM loads: the kernel, built from the cookbook's kernel
# checkout for ARCH=mips64 with the fork's target JSON and linker script.
KERNEL_SOURCE=recipes/core/kernel/source
KERNEL_BUILD=$(CURDIR)/$(INDY_BUILD)/kernel
INDY_ELF=$(KERNEL_BUILD)/kernel
INDY_KERNEL_ALL=$(KERNEL_BUILD)/kernel.all
PROBE_ELF=tools/indy-probe/target/mips64-sgi-redox/release/indy-probe
ECOFF=tools/ecoff/target/release/ecoff

# mk/config.mk points RUSTUP_TOOLCHAIN at the cookbook prefix; every cargo
# call here uses the fork's pin instead.
$(ECOFF): $(wildcard tools/ecoff/src/*.rs) tools/ecoff/Cargo.toml
	cd tools/ecoff && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) cargo build --release

$(PROBE_ELF): $(wildcard tools/indy-probe/src/*.rs) tools/indy-probe/Cargo.toml tools/indy-probe/linker.ld $(INDY_TARGET)
	cd tools/indy-probe && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) cargo build --release

$(INDY_BUILD)/probe: $(PROBE_ELF) $(ECOFF)
	mkdir -p $(INDY_BUILD)
	$(ECOFF) convert $(PROBE_ELF) $@

indy-probe: $(INDY_BUILD)/probe

indy-probe-gate: $(INDY_BUILD)/probe
	WORK=$(CURDIR)/$(INDY_BUILD)/gate tools/indy-gate.sh $< REDOX-PROBE-OK

# The kernel Makefile tracks its own sources; always ask it.
$(INDY_ELF): FORCE
	RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) $(MAKE) -C $(KERNEL_SOURCE) ARCH=mips64 BUILD=$(KERNEL_BUILD)

# Userspace: the bootstrap program and the initfs it is archived into.
BASE_SOURCE=recipes/core/base/source
BOOTSTRAP_ELF=$(BASE_SOURCE)/bootstrap/target/mips64-sgi-redox/release/bootstrap
INITFS_AR=$(BASE_SOURCE)/target/release/redox-initfs-ar
INITFS_DIR=$(INDY_BUILD)/initfs
INITFS_IMG=$(INDY_BUILD)/initfs.img

$(BOOTSTRAP_ELF): FORCE
	cd $(BASE_SOURCE)/bootstrap && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) cargo rustc --release \
		--target $(CURDIR)/$(INDY_TARGET) \
		-Zbuild-std=core,alloc -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec

$(INITFS_AR): FORCE
	cd $(BASE_SOURCE)/initfs/tools && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) cargo build --release

# The static relibc sysroot (tools/relibc-indy.sh) and the first std program,
# which stands in for init until base's init builds for the target.
SYSROOT=$(INDY_BUILD)/sysroot
INIT_ELF=tools/indy-sh/target/mips64-sgi-redox/release/indy-sh
RUST_SRC?=/mnt/europa/redox-rust-src/library

$(SYSROOT)/lib/libc.a: FORCE
	tools/relibc-indy.sh $(CURDIR)/$(SYSROOT)

$(INIT_ELF): $(SYSROOT)/lib/libc.a $(wildcard tools/indy-sh/src/*.rs) tools/indy-sh/Cargo.toml tools/indy-sh/.cargo/config.toml
	touch tools/indy-sh/src/main.rs
	cd tools/indy-sh && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) __CARGO_TESTS_ONLY_SRC_ROOT=$(RUST_SRC) cargo build --release

# Base programs for the target (tools/base-indy.sh): init, the Seeq driver and
# the network stack go in the initfs; dhcpd and zerod go in the root store.
BASE_BINS=init randd seeqd netstack dhcpd zerod
ROOT_BINS=dhcpd zerod
BASE_OUT=$(BASE_SOURCE)/target/mips64-sgi-redox/release

$(BASE_OUT)/init: $(SYSROOT)/lib/libc.a FORCE
	tools/base-indy.sh $(BASE_BINS)

# webfs: the read-only filesystem served over HTTP or HTTPS (tools/webfs).
# The same crate builds for the host (pack, serve) and for the target (mount).
WEBFS_ELF=tools/webfs/target/mips64-sgi-redox/release/webfs
WEBFS_HOST=tools/webfs/target/release/webfs
WEBFS_SRC=$(wildcard tools/webfs/src/*.rs) tools/webfs/Cargo.toml tools/webfs/Cargo.lock

$(WEBFS_ELF): $(SYSROOT)/lib/libc.a $(WEBFS_SRC)
	tools/webfs-indy.sh

$(WEBFS_HOST): $(WEBFS_SRC)
	cd tools/webfs && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) cargo build --release

webfs: $(WEBFS_HOST)

# The root filesystem is a webfs store, mounted as scheme `file` by the last
# initfs unit (tools/indy-image.sh).
#   INDY_ROOT_URL     where the machine finds the store
#   INDY_ROOT_ARGS    `webfs mount` options, e.g. --connect ADDR, --root HASH
#   INDY_STORE        where `make indy-root` packs it; serve it with
#                     `webfs serve $(INDY_STORE) --listen 0.0.0.0:8734`
INDY_ROOT_URL?=http://10.1.1.3:8734/
INDY_ROOT_ARGS?=--connect 10.1.1.3
INDY_STORE?=$(INDY_BUILD)/store
ROOT_DIR=$(INDY_BUILD)/root
# The emulator's NAT gateway reaches this host's loopback.
GATE_ROOT_PORT?=8735
GATE_ROOT_URL=http://192.168.0.1:$(GATE_ROOT_PORT)/

IMAGE_DEPS=$(INDY_ELF) $(ECOFF) $(BOOTSTRAP_ELF) $(INITFS_AR) $(BASE_OUT)/init $(WEBFS_ELF) \
	tools/indy-image.sh $(wildcard config/indy/init.d/* config/indy/net/*)

$(INDY_BUILD)/redox: $(IMAGE_DEPS)
	tools/indy-image.sh $@ $(INITFS_DIR) "$(INDY_ROOT_URL)" $(INDY_ROOT_ARGS)
	llvm-size $(INDY_KERNEL_ALL)

$(INDY_BUILD)/redox-gate: $(IMAGE_DEPS)
	tools/indy-image.sh $@ $(INITFS_DIR)-gate "$(GATE_ROOT_URL)" --connect 192.168.0.1

indy: $(INDY_BUILD)/redox

# The root tree: config/indy/root plus the programs, packed into the store.
# Packing is incremental and only `root` changes for clients.
indy-root: $(WEBFS_HOST) $(WEBFS_ELF) $(INIT_ELF) $(BASE_OUT)/init
	rm -rf $(ROOT_DIR)
	mkdir -p $(ROOT_DIR)/usr/bin
	cp -r config/indy/root/. $(ROOT_DIR)/
	for bin in $(ROOT_BINS); do install -m 0755 $(BASE_OUT)/$$bin $(ROOT_DIR)/usr/bin/$$bin; done
	install -m 0755 $(INIT_ELF) $(ROOT_DIR)/usr/bin/sh
	install -m 0755 $(WEBFS_ELF) $(ROOT_DIR)/usr/bin/webfs
	ln -s usr/bin $(ROOT_DIR)/bin
	$(WEBFS_HOST) pack $(ROOT_DIR) $(INDY_STORE)
	du -sb $(ROOT_DIR) $(INDY_STORE)

indy-gate: $(INDY_BUILD)/redox-gate indy-root
	WORK=$(CURDIR)/$(INDY_BUILD)/gate SHELL_CHECK=1 ROOT_STORE=$(CURDIR)/$(INDY_STORE) ROOT_PORT=$(GATE_ROOT_PORT) \
		tools/indy-gate.sh $< "$(INDY_MARKER)"

indy-netboot: $(INDY_BUILD)/redox
	for name in $(INDY_TFTP_NAMES); do install -m 0644 $< $(INDY_TFTP_DIR)/$$name; done

# Pack a test tree, serve it over HTTPS on this host and read it from the
# emulated Indy through a second mount, /scheme/web.
indy-webfs-gate: $(WEBFS_HOST) $(INDY_BUILD)/redox-gate indy-root
	ROOT_STORE=$(CURDIR)/$(INDY_STORE) ROOT_PORT=$(GATE_ROOT_PORT) tools/webfs-gate.sh

.PHONY: webfs indy-webfs-gate indy indy-root indy-gate indy-netboot indy-probe indy-probe-gate
