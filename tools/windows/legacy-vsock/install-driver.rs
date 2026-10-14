#![no_std]
#![no_main]
use core::ffi::c_void;
#[cfg_attr(
    target_arch = "x86",
    link(name = "newdev", kind = "raw-dylib", import_name_type = "undecorated")
)]
#[cfg_attr(target_arch = "x86_64", link(name = "newdev", kind = "raw-dylib"))]
unsafe extern "system" {
    fn UpdateDriverForPlugAndPlayDevicesA(
        parent: *mut c_void,
        id: *const u8,
        inf: *const u8,
        flags: u32,
        reboot: *mut i32,
    ) -> i32;
}
#[cfg_attr(
    target_arch = "x86",
    link(
        name = "kernel32",
        kind = "raw-dylib",
        import_name_type = "undecorated"
    )
)]
#[cfg_attr(target_arch = "x86_64", link(name = "kernel32", kind = "raw-dylib"))]
unsafe extern "system" {
    fn GetLastError() -> u32;
    fn ExitProcess(code: u32) -> !;
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { ExitProcess(255) }
}
#[unsafe(no_mangle)]
pub extern "system" fn mainCRTStartup() -> ! {
    let mut reboot = 0;
    unsafe {
        let result = UpdateDriverForPlugAndPlayDevicesA(
            core::ptr::null_mut(),
            c"PCI\\VEN_1AF4&DEV_1053&SUBSYS_10531AF4&REV_01"
                .as_ptr()
                .cast(),
            c"C:\\Drivers\\VIOSock\\viosock.inf".as_ptr().cast(),
            1,
            &mut reboot,
        );
        ExitProcess(if result == 0 {
            GetLastError()
        } else if reboot != 0 {
            3010
        } else {
            0
        });
    }
}
