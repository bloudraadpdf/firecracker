#![no_std]
#![no_main]

use core::arch::{asm, global_asm};
use core::ptr::{read_unaligned, read_volatile, write_unaligned, write_volatile};

global_asm!(
    ".section .text.entry,\"ax\"",
    ".global _start",
    ".code32",
    "_start:",
    "cli",
    "cld",
    "mov eax, cr0",
    "and eax, 0xfffffffb",
    "or eax, 2",
    "mov cr0, eax",
    "mov eax, cr4",
    "or eax, 0x600",
    "mov cr4, eax",
    "fninit",
    "mov esp, 0x1ff000",
    "sub esp, 12",
    "push ebx",
    "call rust_main",
    ".section .note.Xen,\"a\",@note",
    ".balign 4",
    ".long 4, 4, 18",
    ".asciz \"Xen\"",
    ".long _start",
);

const BIOS: &[u8] = include_bytes!("bios.elf");
const TABLE: usize = 0x500;
const RSDP: usize = 0x180000;

fn stop() -> ! {
    loop {
        unsafe { asm!("cli", "hlt") }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    serial(b"PVH BIOS panic\r\n");
    stop()
}

fn serial(bytes: &[u8]) {
    for byte in bytes {
        unsafe {
            loop {
                let status: u8;
                asm!("in al, dx", in("dx") 0x3fdu16, out("al") status);
                if status & 0x20 != 0 {
                    break;
                }
            }
            asm!("out dx, al", in("dx") 0x3f8u16, in("al") *byte);
        }
    }
}

unsafe fn get32(address: usize) -> u32 {
    unsafe { read_unaligned(address as *const u32) }
}

unsafe fn get64(address: usize) -> u64 {
    unsafe { read_unaligned(address as *const u64) }
}

unsafe fn put32(address: usize, value: u32) {
    unsafe { write_unaligned(address as *mut u32, value) }
}

unsafe fn put64(address: usize, value: u64) {
    unsafe { write_unaligned(address as *mut u64, value) }
}

unsafe fn copy(source: usize, destination: usize, length: usize) {
    for i in 0..length {
        unsafe {
            write_volatile(
                (destination + i) as *mut u8,
                read_volatile((source + i) as *const u8),
            )
        }
    }
}

unsafe fn checksum(address: usize, length: usize) -> u16 {
    let mut sum = 0u32;
    for i in (0..length).step_by(2) {
        sum += unsafe { read_unaligned((address + i) as *const u16) } as u32;
    }
    sum = (sum & 0xffff) + (sum >> 16);
    !((sum & 0xffff) + (sum >> 16)) as u16
}

unsafe fn range(index: usize, start: u64, size: u64, kind: u32) {
    let address = TABLE + 32 + index * 20;
    unsafe {
        put64(address, start);
        put64(address + 8, size);
        put32(address + 16, kind);
    }
}

#[unsafe(no_mangle)]
unsafe extern "C" fn rust_main(info: usize) -> ! {
    serial(b"PVH BIOS handoff\r\n");
    unsafe {
        assert_eq!(get32(info), 0x336ec578);
        assert!(get32(info + 4) >= 1);
        let map = usize::try_from(get64(info + 40)).unwrap();
        let count = get32(info + 48) as usize;
        assert!(count <= 32);
        let rsdp = usize::try_from(get64(info + 32)).unwrap();
        assert_eq!(get64(rsdp), u64::from_le_bytes(*b"RSD PTR "));
        let rsdp_len = get32(rsdp + 20) as usize;
        assert!((36..=4096).contains(&rsdp_len));
        copy(rsdp, RSDP, rsdp_len);
        for i in 0..count {
            let source = map + i * 24;
            range(i, get64(source), get64(source + 8), get32(source + 16));
        }
        range(count, 0x100000, 0x100000, 2);
        range(count + 1, RSDP as u64, rsdp_len as u64, 16);
        range(count + 2, 0xa0000, 0x60000, 2);
        let body_len = 8 + (count + 3) * 20;
        put32(TABLE + 24, 1);
        put32(TABLE + 28, body_len as u32);
        put32(TABLE, 0x4f49424c);
        put32(TABLE + 4, 24);
        put32(TABLE + 8, 0);
        put32(TABLE + 12, body_len as u32);
        put32(TABLE + 16, checksum(TABLE + 24, body_len) as u32);
        put32(TABLE + 20, 1);
        put32(TABLE + 8, checksum(TABLE, 24) as u32);

        let elf = BIOS.as_ptr() as usize;
        assert_eq!(&BIOS[..5], b"\x7fELF\x01");
        let entry = get32(elf + 24);
        let phoff = get32(elf + 28) as usize;
        let phsize = read_unaligned((elf + 42) as *const u16) as usize;
        let phnum = read_unaligned((elf + 44) as *const u16) as usize;
        assert_eq!(phsize, 32);
        for i in 0..phnum {
            let header = elf + phoff + i * phsize;
            if get32(header) != 1 {
                continue;
            }
            let offset = get32(header + 4) as usize;
            let destination = get32(header + 12) as usize;
            let length = get32(header + 16) as usize;
            let memory = get32(header + 20) as usize;
            assert!(offset + length <= BIOS.len());
            assert!(destination >= 0xc0000 && destination + memory <= 0x100000);
            assert!(length <= memory);
            copy(elf + offset, destination, length);
            for address in destination + length..destination + memory {
                write_volatile(address as *mut u8, 0);
            }
        }
        serial(b"SeaBIOS entry\r\n");
        asm!("jmp {entry:e}", entry = in(reg) entry, options(noreturn));
    }
}
