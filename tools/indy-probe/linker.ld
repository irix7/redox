/* Load image for the SGI Indy PROM: kseg0 0x8804_0000 (physical 0x0804_0000).
 * Text, rodata and data are contiguous so the ECOFF converter can make one
 * .text and one .data section; .bss follows and _start zeroes it. */
OUTPUT_FORMAT("elf32-ntradbigmips")
OUTPUT_ARCH(mips)
ENTRY(_start)

SECTIONS
{
  . = 0x88040000;

  .text : {
    KEEP(*(.text._start))
    *(.text .text.*)
  }

  .rodata : { *(.rodata .rodata.*) }

  .data : {
    *(.data .data.* .sdata .sdata.* .got .got.*)
    . = ALIGN(16);
  }

  .bss (NOLOAD) : {
    __bss_start = .;
    *(.bss .bss.* .sbss .sbss.* COMMON)
    . = ALIGN(16);
    __bss_end = .;
    /* Boot stack: 64 KiB above .bss, inside the image's memory footprint. */
    . += 0x10000;
    . = ALIGN(16);
    __stack_top = .;
  }

  /DISCARD/ : {
    *(.MIPS.abiflags .reginfo .pdr .comment .gnu.attributes .mdebug* .options* .eh_frame)
  }
}
