use super::{CloseHandle, CreateFileW, WriteFile, number, output, ptr, require};

#[link(
    name = "kernel32",
    kind = "raw-dylib",
    import_name_type = "undecorated"
)]
unsafe extern "system" {
    fn ReadFile(handle: usize, data: *mut u8, len: u32, read: *mut u32, overlap: *mut u8) -> i32;
    fn FlushFileBuffers(handle: usize) -> i32;
    fn Sleep(milliseconds: u32);
}

struct File(usize);
impl File {
    fn open(access: u32, creation: u32) -> Self {
        let name = (*b"C:\\xp-legacy-data.bin\0").map(u16::from);
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                access,
                0,
                ptr::null(),
                creation,
                0x80000080,
                0,
            )
        };
        require(handle != usize::MAX, b"storage open");
        Self(handle)
    }
}
impl Drop for File {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn pattern(bytes: &mut [u8; 1024], chunk: usize, iteration: u8) {
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = (i.wrapping_mul(37) ^ chunk ^ (chunk >> 8)) as u8 ^ iteration;
    }
}

fn storage(iteration: u8) {
    let mut expected = [0u8; 1024];
    {
        let file = File::open(0x40000000, 2);
        for chunk in 0..2048 {
            pattern(&mut expected, chunk, iteration);
            let mut written = 0;
            require(
                unsafe {
                    WriteFile(
                        file.0,
                        expected.as_ptr(),
                        1024,
                        &mut written,
                        ptr::null_mut(),
                    )
                } != 0
                    && written == 1024,
                b"storage write",
            );
        }
        require(unsafe { FlushFileBuffers(file.0) } != 0, b"storage flush");
    }
    let file = File::open(0x80000000, 3);
    let mut actual = [0u8; 1024];
    for chunk in 0..2048 {
        pattern(&mut expected, chunk, iteration);
        let mut read = 0;
        require(
            unsafe {
                ReadFile(
                    file.0,
                    actual.as_mut_ptr(),
                    1024,
                    &mut read,
                    ptr::null_mut(),
                )
            } != 0
                && read == 1024
                && actual == expected,
            b"storage read equality",
        );
    }
    let mut read = 0;
    require(
        unsafe { ReadFile(file.0, actual.as_mut_ptr(), 1, &mut read, ptr::null_mut()) } != 0
            && read == 0,
        b"storage length",
    );
    output(b"XP_LEGACY_STORAGE bytes=2097152 iteration=");
    number(u32::from(iteration));
    output(b"\r\n");
}

pub fn run() {
    for iteration in 1..=3 {
        storage(iteration);
        super::run();
        output(b"XPLEGACY_PASS iteration=");
        number(u32::from(iteration));
        output(b"\r\n");
        if iteration < 3 {
            unsafe { Sleep(60000) };
        }
    }
}
