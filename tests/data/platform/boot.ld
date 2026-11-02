/* Copyright 2026 Bloudraad. */
/* SPDX-License-Identifier: Apache-2.0 */
ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); note PT_NOTE FLAGS(4); }
SECTIONS {
    . = 0x100000;
    .text : { *(.text.entry) *(.text .text.*) } :text
    .rodata : { *(.rodata .rodata.*) } :text
    .note.Xen : { *(.note.Xen) } :text :note
    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}
