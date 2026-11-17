# SGI Indy netboot: a file named `redox` in a TFTP directory, booted at the PROM
# with `boot -f bootp()redox`. Independent of ARCH and of the disk image path.
#
#   make indy            build build/indy/redox (ECOFF) from the current image
#   make indy-gate       boot it on a 32 MiB emulated Indy and assert the marker
#   make indy-netboot    copy it to INDY_TFTP_DIR (default /var/lib/sgi-netboot)

INDY_BUILD=build/indy
INDY_TARGET=targets/mips64-sgi-redox.json
INDY_TFTP_DIR?=/var/lib/sgi-netboot
INDY_MARKER?=REDOX-INDY-INIT: hello
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
INIT_ELF=tools/indy-init/target/mips64-sgi-redox/release/indy-init
RUST_SRC?=/mnt/europa/redox-rust-src/library

$(SYSROOT)/lib/libc.a: FORCE
	tools/relibc-indy.sh $(CURDIR)/$(SYSROOT)

$(INIT_ELF): $(SYSROOT)/lib/libc.a $(wildcard tools/indy-init/src/*.rs) tools/indy-init/Cargo.toml tools/indy-init/.cargo/config.toml
	cd tools/indy-init && RUSTUP_TOOLCHAIN=$(INDY_RUST_TOOLCHAIN) __CARGO_TESTS_ONLY_SRC_ROOT=$(RUST_SRC) cargo build --release

# The initfs holds what the bootstrap starts: bin/init and the drivers.
$(INITFS_IMG): $(BOOTSTRAP_ELF) $(INITFS_AR) $(INIT_ELF)
	rm -rf $(INITFS_DIR)
	mkdir -p $(INITFS_DIR)/bin $(INITFS_DIR)/lib/drivers $(INITFS_DIR)/lib/init.d
	install -m 0755 $(INIT_ELF) $(INITFS_DIR)/bin/init
	$(INITFS_AR) $(INITFS_DIR) $(BOOTSTRAP_ELF) -o $@
	ls -l $@

$(INDY_BUILD)/redox: $(INDY_ELF) $(ECOFF) $(INITFS_IMG)
	mkdir -p $(INDY_BUILD)
	$(ECOFF) convert $(INDY_ELF) $@ --append $(INITFS_IMG)
	llvm-size $(INDY_KERNEL_ALL)

indy: $(INDY_BUILD)/redox

indy-gate: $(INDY_BUILD)/redox
	WORK=$(CURDIR)/$(INDY_BUILD)/gate tools/indy-gate.sh $< "$(INDY_MARKER)"

indy-netboot: $(INDY_BUILD)/redox
	install -m 0644 $< $(INDY_TFTP_DIR)/redox

.PHONY: indy indy-gate indy-netboot indy-probe indy-probe-gate
