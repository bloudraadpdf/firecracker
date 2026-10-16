/* Copyright 2026 Bloudraad. */
/* SPDX-License-Identifier: Apache-2.0 */

ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); data PT_LOAD FLAGS(6); note PT_NOTE FLAGS(4); }
SECTIONS {
    . = 0x100000;
    .text : { *(.text.entry) *(.text .text.*) } :text
    .rodata : { *(.rodata .rodata.*) } :text
    .note.Xen : { *(.note.Xen) } :text :note
    . = ALIGN(4096);
    .data : { *(.data .data.*) } :data
    .bss : { *(.bss .bss.*) } :data
    ASSERT(. < 0x180000, "Firmware overlaps ACPI handoff")
    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}
