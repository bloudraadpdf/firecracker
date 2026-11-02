// Copyright 2026 Bloudraad.
// SPDX-License-Identifier: Apache-2.0

#![no_std]
#![no_main]

core::arch::global_asm!(
    ".section .text.entry,\"ax\"",
    ".global _start",
    ".code32",
    "_start:",
    "cli",
    ".macro vbe index, value",
    "mov dx, 0x1ce",
    "mov ax, \\index",
    "out dx, ax",
    "inc dx",
    "mov ax, \\value",
    "out dx, ax",
    ".endm",
    "vbe 1, 2",
    "vbe 2, 2",
    "vbe 3, 32",
    "vbe 4, 1",
    "mov dword ptr [0xa0000], 0x00112233",
    "mov dword ptr [0xa0004], 0x00aabbcc",
    "mov dx, 0x3f8",
    "mov al, 0x50",
    "out dx, al",
    "2: hlt",
    "jmp 2b",
    ".section .note.Xen,\"a\",@note",
    ".balign 4",
    ".long 4, 4, 18",
    ".asciz \"Xen\"",
    ".long _start",
);

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        // SAFETY: This test image owns the guest CPU and has no interrupt handlers.
        unsafe { core::arch::asm!("cli", "hlt") }
    }
}
