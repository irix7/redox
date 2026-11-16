# SGI Indy netboot: a file named `redox` in a TFTP directory, booted at the PROM
# with `boot -f bootp()redox`. Independent of ARCH and of the disk image path.
#
#   make indy            build build/indy/redox (ECOFF) from the current image
#   make indy-gate       boot it on a 32 MiB emulated Indy and assert the marker
#   make indy-netboot    copy it to INDY_TFTP_DIR (default /var/lib/sgi-netboot)

INDY_BUILD=build/indy
INDY_TARGET=targets/mips64-sgi-redox.json
INDY_TFTP_DIR?=/var/lib/sgi-netboot
INDY_MARKER?=REDOX-PROBE-OK

# The image the PROM loads. Today this is the serial probe; the kernel replaces it.
INDY_ELF=tools/indy-probe/target/mips64-sgi-redox/release/indy-probe
ECOFF=tools/ecoff/target/release/ecoff

$(ECOFF): $(wildcard tools/ecoff/src/*.rs) tools/ecoff/Cargo.toml
	cd tools/ecoff && cargo build --release

$(INDY_ELF): $(wildcard tools/indy-probe/src/*.rs) tools/indy-probe/Cargo.toml tools/indy-probe/linker.ld $(INDY_TARGET)
	cd tools/indy-probe && cargo build --release

$(INDY_BUILD)/redox: $(INDY_ELF) $(ECOFF)
	mkdir -p $(INDY_BUILD)
	$(ECOFF) convert $(INDY_ELF) $@

indy: $(INDY_BUILD)/redox

indy-gate: $(INDY_BUILD)/redox
	WORK=$(CURDIR)/$(INDY_BUILD)/gate tools/indy-gate.sh $< $(INDY_MARKER)

indy-netboot: $(INDY_BUILD)/redox
	install -m 0644 $< $(INDY_TFTP_DIR)/redox

.PHONY: indy indy-gate indy-netboot
